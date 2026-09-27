//! 认证 HTTP 适配器：将账号用例映射为 JSON、HttpOnly Cookie 与 OAuth 重定向。
//!
//! 本模块不读取密码哈希、不交换第三方令牌；只负责请求校验边界、
//! Cookie 属性、错误状态码和跳转。生产站点使用 HTTPS 时 Cookie 带 Secure。

use crate::{
    AppState,
    application::auth::{AuthFailure, AuthUser},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use subtle::ConstantTimeEq;

type ApiResult<T> = Result<T, (StatusCode, String)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/auth/providers", get(providers))
        .route("/api/v1/auth/email-code", post(send_code))
        .route("/api/v1/auth/register", post(register))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/oauth/{provider}/start", get(oauth_start))
        .route(
            "/api/v1/auth/oauth/{provider}/callback",
            get(oauth_callback),
        )
        .layer(DefaultBodyLimit::max(8 * 1024))
}

/// 只提取平台自己的会话 Cookie，避免将其他站点 Cookie 当作认证依据。
///
/// 固定的 43 字符长度对应 32 字节随机令牌的 URL-safe Base64 编码；
/// 长度不对直接拒绝，业务层再查询数据库中的令牌哈希与有效期。
pub fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(name, value)| {
            (name == "po_session" && value.len() == 43).then(|| value.to_owned())
        })
}

fn oauth_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(name, value)| {
            (name == "po_oauth_state" && value.len() == 43).then(|| value.to_owned())
        })
}

fn cookie(name: &str, value: &str, max_age: i64, secure: bool, path: &str) -> String {
    // SameSite=Lax 允许 OAuth 提供方回跳时带上 state Cookie，仍减少
    // 跨站表单自动带会话的风险。写接口另做同源校验。
    format!(
        "{name}={value}; Path={path}; Max-Age={max_age}; HttpOnly; SameSite=Lax{}",
        if secure { "; Secure" } else { "" }
    )
}

fn with_cookie(mut response: Response, value: String) -> Response {
    if let Ok(value) = HeaderValue::from_str(&value) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

fn user_json(user: &AuthUser) -> Value {
    json!({"id":user.id,"email":user.email,"display_name":user.display_name,"role":user.role})
}

fn map_error(error: AuthFailure) -> (StatusCode, String) {
    match error {
        AuthFailure::InvalidInput(message) => (StatusCode::BAD_REQUEST, message.into()),
        AuthFailure::InvalidCredentials => (StatusCode::UNAUTHORIZED, "邮箱或密码错误".into()),
        AuthFailure::InvalidCode => (StatusCode::BAD_REQUEST, "验证码无效或已过期".into()),
        AuthFailure::RateLimited => (
            StatusCode::TOO_MANY_REQUESTS,
            "请求过于频繁，请稍后重试".into(),
        ),
        AuthFailure::Conflict(message) => (StatusCode::CONFLICT, message.into()),
        AuthFailure::ProviderDisabled => (StatusCode::NOT_FOUND, "此登录方式尚未配置".into()),
        AuthFailure::Internal(error) => {
            tracing::error!(%error, "认证流程失败");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "认证服务暂时不可用".into(),
            )
        }
    }
}

async fn providers(State(state): State<Arc<AppState>>) -> Json<Value> {
    let (github, google) = state.auth.providers();
    Json(json!({"email":true,"github":github,"google":google,
        "local_mailbox": if state.settings.app.mode == "local" && state.settings.auth.mail.host == "mailpit" { Some("http://localhost:8025") } else { None }}))
}

#[derive(Deserialize)]
struct EmailInput {
    email: String,
}

async fn send_code(
    State(state): State<Arc<AppState>>,
    Json(input): Json<EmailInput>,
) -> ApiResult<Json<Value>> {
    state
        .auth
        .send_code(&input.email)
        .await
        .map_err(map_error)?;
    Ok(Json(json!({"message":"如果邮箱可注册，验证码已发送"})))
}

#[derive(Deserialize)]
struct RegisterInput {
    email: String,
    display_name: String,
    password: String,
    code: String,
}

async fn register(
    State(state): State<Arc<AppState>>,
    Json(input): Json<RegisterInput>,
) -> ApiResult<Response> {
    let (user, token) = state
        .auth
        .register(
            &input.email,
            &input.display_name,
            &input.password,
            &input.code,
        )
        .await
        .map_err(map_error)?;
    let response = Json(user_json(&user)).into_response();
    Ok(with_cookie(
        response,
        cookie(
            "po_session",
            &token,
            state.auth.session_max_age_secs(),
            state.auth.secure_cookie(),
            "/",
        ),
    ))
}

#[derive(Deserialize)]
struct LoginInput {
    email: String,
    password: String,
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(input): Json<LoginInput>,
) -> ApiResult<Response> {
    let (user, token) = state
        .auth
        .login(&input.email, &input.password)
        .await
        .map_err(map_error)?;
    let response = Json(user_json(&user)).into_response();
    Ok(with_cookie(
        response,
        cookie(
            "po_session",
            &token,
            state.auth.session_max_age_secs(),
            state.auth.secure_cookie(),
            "/",
        ),
    ))
}

async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Response> {
    if !super::http::same_origin(&headers, &state) {
        return Err((StatusCode::FORBIDDEN, "请求来源无效".into()));
    }
    if let Some(token) = session_cookie(&headers) {
        state
            .auth
            .logout(&token)
            .await
            .map_err(|error| map_error(AuthFailure::Internal(error)))?;
    }
    Ok(with_cookie(
        StatusCode::NO_CONTENT.into_response(),
        cookie("po_session", "", 0, state.auth.secure_cookie(), "/"),
    ))
}

async fn oauth_start(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
) -> ApiResult<Response> {
    let (url, state_value) = state.auth.start_oauth(&provider).await.map_err(map_error)?;
    let response = Redirect::to(&url).into_response();
    Ok(with_cookie(
        response,
        cookie(
            "po_oauth_state",
            &state_value,
            600,
            state.auth.secure_cookie(),
            "/api/v1/auth/oauth",
        ),
    ))
}

#[derive(Deserialize)]
struct OAuthCallback {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}

async fn oauth_callback(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
    Query(query): Query<OAuthCallback>,
    headers: HeaderMap,
) -> Response {
    let mut response = if query.error.is_some() {
        Redirect::to("/login?auth_error=cancelled").into_response()
    } else {
        // 查询参数中的 state 必须与同一浏览器保存的 HttpOnly Cookie 匹配；
        // 后续应用层还会原子消费数据库记录，抵御伪造回调和重放。
        let valid_state = query
            .state
            .as_deref()
            .zip(oauth_cookie(&headers))
            .is_some_and(|(given, saved)| bool::from(given.as_bytes().ct_eq(saved.as_bytes())));
        if !valid_state {
            Redirect::to("/login?auth_error=state").into_response()
        } else {
            match state
                .auth
                .finish_oauth(
                    &provider,
                    query.state.as_deref().unwrap_or_default(),
                    query.code.as_deref().unwrap_or_default(),
                )
                .await
            {
                Ok((_user, token)) => with_cookie(
                    Redirect::to("/overview").into_response(),
                    cookie(
                        "po_session",
                        &token,
                        state.auth.session_max_age_secs(),
                        state.auth.secure_cookie(),
                        "/",
                    ),
                ),
                Err(error) => {
                    let code = if matches!(error, AuthFailure::Conflict(_)) {
                        "existing"
                    } else {
                        "failed"
                    };
                    if let AuthFailure::Internal(error) = error {
                        tracing::warn!(%error, provider, "OAuth 登录失败");
                    }
                    Redirect::to(&format!("/login?auth_error={code}")).into_response()
                }
            }
        }
    };
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie(
            "po_oauth_state",
            "",
            0,
            state.auth.secure_cookie(),
            "/api/v1/auth/oauth",
        ))
        .unwrap_or_else(|_| {
            HeaderValue::from_static("po_oauth_state=; Max-Age=0; Path=/api/v1/auth/oauth")
        }),
    );
    response
}
