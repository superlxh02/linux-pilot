//! REST/WebSocket 适配器：参数校验、权限判断和应用服务调用。

use crate::{
    AppState,
    application::ports::{MetricRange, ProfileCompletion},
};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
};
use linux_pilot_model::Scenario;
use linux_pilot_scoring::topology::{PodObservation, TopologyManifest};
use linux_pilot_wire::agent::{ProfileCommand, ServerFrame, server_frame::Body};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{str::FromStr, sync::Arc};
use subtle::ConstantTimeEq;

type ApiResult<T> = Result<T, (StatusCode, String)>;

pub fn router(state: Arc<AppState>) -> Router {
    let read_routes = Router::new()
        .route("/api/v1/session", get(session))
        .route("/api/v1/hosts", get(hosts))
        .route("/api/v1/hosts/{id}/overview", get(overview))
        .route("/api/v1/metrics", get(metrics))
        .route("/api/v1/processes", get(processes))
        .route("/api/v1/process-watches", get(process_watches))
        .route("/api/v1/scores", get(scores))
        .route("/api/v1/profiles/{id}", get(profile))
        .route("/api/v1/profiles", get(profiles))
        .route("/api/v1/alerts", get(alert_rules))
        .route("/api/v1/alert-events", get(alert_events))
        .route("/api/v1/topology/manifest", get(topology_manifest))
        .route("/api/v1/topology/snapshot", get(topology_snapshot))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            authorize_reader,
        ));
    let write_routes = Router::new()
        .route("/api/v1/profiles", post(create_profile))
        .route("/api/v1/process-watches", post(add_process_watch))
        .route(
            "/api/v1/process-watches/{host_id}/{pid}/{start_ticks}",
            delete(remove_process_watch),
        )
        .route("/api/v1/alerts", post(create_alert))
        .route("/api/v1/alerts/{id}", patch(update_alert))
        .route("/api/v1/topology/validate", post(validate_topology))
        .route("/api/v1/topology/manifest", put(apply_topology))
        .route("/api/v1/topology/pods", post(observe_pods))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            authorize_operator,
        ));
    let admin_routes = Router::new()
        .route("/api/v1/admin/users", get(admin_users))
        .route("/api/v1/admin/users/{id}/activity", get(admin_activity))
        .route("/api/v1/admin/users/{id}/role", patch(admin_set_role))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            authorize_admin,
        ));
    Router::new()
        .route("/health/live", get(|| async { "ok" }))
        .route("/health/ready", get(ready))
        .route("/api/v1/stream", get(stream))
        .merge(super::auth_http::router())
        .merge(read_routes)
        .merge(write_routes)
        .merge(admin_routes)
        .with_state(state)
}

/// 读取当前配置供文件化管理和 AI 自动化导出，运行时 Pod 不混进配置文件。
async fn topology_manifest(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    let value = state.topology.manifest().await.map_err(server_error)?;
    Ok(Json(json!(value)))
}

async fn topology_snapshot(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    Ok(Json(state.topology.snapshot().await.map_err(server_error)?))
}

/// dry-run 只执行领域校验，不要求当前集群在线，也不写数据库。
async fn validate_topology(Json(manifest): Json<TopologyManifest>) -> ApiResult<Json<Value>> {
    manifest
        .validate()
        .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    Ok(Json(
        json!({"valid":true,"revision":manifest.metadata.revision}),
    ))
}

async fn apply_topology(
    State(state): State<Arc<AppState>>,
    Json(manifest): Json<TopologyManifest>,
) -> ApiResult<Json<Value>> {
    manifest
        .validate()
        .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    state.topology.apply(&manifest).await.map_err(|error| {
        let message = error.to_string();
        if message.contains("版本冲突") {
            (StatusCode::CONFLICT, message)
        } else {
            server_error(error)
        }
    })?;
    Ok(Json(
        json!({"applied":true,"revision":manifest.metadata.revision}),
    ))
}

/// Kubernetes 发现器每轮按 Pod UID 更新事实；旧观察超过 90 秒即不参与快照。
async fn observe_pods(
    State(state): State<Arc<AppState>>,
    Json(pods): Json<Vec<PodObservation>>,
) -> ApiResult<Json<Value>> {
    let count = pods.len();
    state
        .topology
        .observe(&pods)
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    Ok(Json(json!({"accepted":count})))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Viewer,
    Operator,
    Admin,
}

fn token_role(token: &str, state: &AppState) -> Option<Role> {
    if bool::from(
        token
            .as_bytes()
            .ct_eq(state.settings.auth.operator_token.as_bytes()),
    ) {
        Some(Role::Operator)
    } else if bool::from(
        token
            .as_bytes()
            .ct_eq(state.settings.auth.viewer_token.as_bytes()),
    ) {
        Some(Role::Viewer)
    } else {
        None
    }
}

async fn header_role(headers: &HeaderMap, state: &AppState) -> Option<Role> {
    let legacy = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .and_then(|value| token_role(value, state));
    if legacy.is_some() {
        return legacy;
    }
    let cookie = super::auth_http::session_cookie(headers)?;
    match state.auth.session(&cookie).await {
        Ok(Some(user)) if user.role == "admin" => Some(Role::Admin),
        Ok(Some(user)) if user.role == "operator" => Some(Role::Operator),
        Ok(Some(_)) => Some(Role::Viewer),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(%error, "读取用户会话失败");
            None
        }
    }
}

async fn authorize_reader(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if header_role(request.headers(), &state).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

async fn authorize_operator(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if request.headers().get("authorization").is_none() && !same_origin(request.headers(), &state) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !matches!(
        header_role(request.headers(), &state).await,
        Some(Role::Operator | Role::Admin)
    ) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

async fn authorize_admin(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // 管理操作必须来自当前站点的账号会话；旧版静态 operator token
    // 永远不能进入用户管理模块，避免无账号身份的权限变更。
    if request.method() != axum::http::Method::GET && !same_origin(request.headers(), &state) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if current_user(request.headers(), &state)
        .await
        .as_ref()
        .is_none_or(|user| user.role != "admin")
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

async fn current_user(
    headers: &HeaderMap,
    state: &AppState,
) -> Option<crate::application::auth::AuthUser> {
    let cookie = super::auth_http::session_cookie(headers)?;
    state.auth.session(&cookie).await.ok().flatten()
}

/// Cookie 会自动附带，因此对写请求和 WebSocket 握手验证浏览器来源。
pub(crate) fn same_origin(headers: &HeaderMap, state: &AppState) -> bool {
    let expected = state.settings.auth.public_url.trim_end_matches('/');
    if let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) {
        return origin == expected;
    }
    headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_none_or(|site| matches!(site, "same-origin" | "none"))
}

async fn session(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
) -> Json<Value> {
    let role = match header_role(request.headers(), &state).await {
        Some(Role::Admin) => "admin",
        Some(Role::Operator) => "operator",
        _ => "viewer",
    };
    let user = if let Some(cookie) = super::auth_http::session_cookie(request.headers()) {
        state.auth.session(&cookie).await.ok().flatten()
    } else {
        None
    };
    Json(
        json!({"role":role,"email":user.as_ref().map(|u| &u.email),"display_name":user.as_ref().map(|u| &u.display_name)}),
    )
}

#[derive(Deserialize)]
struct ProcessQuery {
    host_id: String,
}

async fn processes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProcessQuery>,
) -> ApiResult<Json<Value>> {
    if query.host_id.is_empty() || query.host_id.len() > 128 {
        return Err((StatusCode::BAD_REQUEST, "主机 ID 无效".into()));
    }
    let rows = state
        .app
        .processes
        .list_processes(
            &query.host_id,
            chrono::Utc::now().timestamp_millis() - 30_000,
        )
        .await
        .map_err(server_error)?;
    Ok(Json(json!(rows)))
}

async fn process_watches(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProcessQuery>,
) -> ApiResult<Json<Value>> {
    if query.host_id.is_empty() || query.host_id.len() > 128 {
        return Err((StatusCode::BAD_REQUEST, "主机 ID 无效".into()));
    }
    Ok(Json(json!(
        state
            .app
            .processes
            .list_watches(&query.host_id)
            .await
            .map_err(server_error)?
    )))
}

#[derive(Deserialize)]
struct NewProcessWatch {
    host_id: String,
    pid: i32,
    start_ticks: i64,
}

async fn add_process_watch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<NewProcessWatch>,
) -> ApiResult<Json<Value>> {
    if input.host_id.is_empty()
        || input.host_id.len() > 128
        || input.pid <= 0
        || input.start_ticks < 0
    {
        return Err((StatusCode::BAD_REQUEST, "进程监控参数无效".into()));
    }
    let watch = state
        .app
        .processes
        .add_watch(
            &input.host_id,
            input.pid,
            input.start_ticks,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .map_err(|error| (StatusCode::CONFLICT, error.to_string()))?;
    if let Err(error) = state.sync_process_watches(&input.host_id).await {
        tracing::warn!(%error, "进程监控配置已保存，等待 Worker 重连同步");
    }
    if let Some(actor) = current_user(&headers, &state).await {
        state
            .auth
            .record_action(
                &actor.id,
                "process.watch",
                &format!("{}:{}", input.host_id, input.pid),
                json!({"start_ticks":input.start_ticks}),
            )
            .await
            .map_err(server_error)?;
    }
    Ok(Json(json!(watch)))
}

async fn remove_process_watch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((host_id, pid, start_ticks)): Path<(String, i32, i64)>,
) -> ApiResult<Json<Value>> {
    if host_id.is_empty() || host_id.len() > 128 || pid <= 0 || start_ticks < 0 {
        return Err((StatusCode::BAD_REQUEST, "进程监控参数无效".into()));
    }
    let removed = state
        .app
        .processes
        .remove_watch(&host_id, pid, start_ticks)
        .await
        .map_err(server_error)?;
    if removed {
        if let Err(error) = state.sync_process_watches(&host_id).await {
            tracing::warn!(%error, "进程监控配置已删除，等待 Worker 重连同步");
        }
        if let Some(actor) = current_user(&headers, &state).await {
            state
                .auth
                .record_action(
                    &actor.id,
                    "process.unwatch",
                    &format!("{host_id}:{pid}"),
                    json!({"start_ticks":start_ticks}),
                )
                .await
                .map_err(server_error)?;
        }
    }
    Ok(Json(json!({"removed":removed})))
}

#[derive(Deserialize)]
struct AdminUsersQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn admin_users(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AdminUsersQuery>,
) -> ApiResult<Json<Value>> {
    let limit = query.limit.unwrap_or(50);
    let offset = query.offset.unwrap_or(0);
    if !(1..=100).contains(&limit) || !(0..=1_000_000).contains(&offset) {
        return Err((StatusCode::BAD_REQUEST, "分页参数无效".into()));
    }
    Ok(Json(json!(
        state
            .auth
            .list_users(limit, offset)
            .await
            .map_err(server_error)?
    )))
}

async fn admin_activity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    if id.len() > 64 {
        return Err((StatusCode::BAD_REQUEST, "用户 ID 无效".into()));
    }
    Ok(Json(json!(
        state
            .auth
            .user_activity(&id, 100)
            .await
            .map_err(server_error)?
    )))
}

#[derive(Deserialize)]
struct ChangeRole {
    role: String,
}

async fn admin_set_role(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<ChangeRole>,
) -> ApiResult<Json<Value>> {
    if !matches!(input.role.as_str(), "viewer" | "operator" | "admin") {
        return Err((StatusCode::BAD_REQUEST, "未知角色".into()));
    }
    let actor = current_user(&headers, &state)
        .await
        .ok_or((StatusCode::FORBIDDEN, "需要管理员会话".into()))?;
    let changed = state
        .auth
        .set_role(&actor.id, &id, &input.role)
        .await
        .map_err(server_error)?;
    if !changed {
        return Err((
            StatusCode::NOT_FOUND,
            "用户不存在或内置管理员不能修改".into(),
        ));
    }
    Ok(Json(json!({"id":id,"role":input.role})))
}

async fn ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if state.app.metrics.ping().await.is_ok() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

fn server_error(error: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!(%error, "API 处理失败");
    (StatusCode::INTERNAL_SERVER_ERROR, "服务端处理失败".into())
}

async fn hosts(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    let rows = state
        .app
        .metrics
        .list_hosts(chrono::Utc::now().timestamp_millis())
        .await
        .map_err(server_error)?;
    Ok(Json(json!(rows)))
}

async fn overview(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let latest = state
        .app
        .metrics
        .overview(&id, chrono::Utc::now().timestamp_millis() - 60_000)
        .await
        .map_err(server_error)?;
    Ok(Json(json!({"host_id":id,"latest":latest})))
}

#[derive(Deserialize)]
struct MetricQuery {
    host_id: String,
    category: Option<String>,
    from: Option<i64>,
    to: Option<i64>,
    limit: Option<u64>,
    aggregate_only: Option<bool>,
    step_ms: Option<i64>,
    labels: Option<String>,
}

async fn metrics(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MetricQuery>,
) -> ApiResult<Json<Value>> {
    let now = chrono::Utc::now().timestamp_millis();
    let from = query.from.unwrap_or(now - 3_600_000);
    let to = query.to.unwrap_or(now);
    if query.host_id.is_empty()
        || from >= to
        || to - from > state.settings.retention.days * 86_400_000
    {
        return Err((StatusCode::BAD_REQUEST, "主机或时间范围无效".into()));
    }
    if query.category.as_ref().is_some_and(|value| {
        ![
            "cpu", "mem", "disk", "fs", "net", "tcp", "proc", "thread", "pod", "cgroup", "ebpf",
            "agent", "host", "io",
        ]
        .contains(&value.as_str())
    }) {
        return Err((StatusCode::BAD_REQUEST, "未知指标分类".into()));
    }
    let limit = query.limit.unwrap_or(5000);
    if !(1..=10_000).contains(&limit) {
        return Err((StatusCode::BAD_REQUEST, "查询条数超限".into()));
    }
    if query
        .step_ms
        .is_some_and(|step| !(1000..=3_600_000).contains(&step))
    {
        return Err((StatusCode::BAD_REQUEST, "聚合粒度无效".into()));
    }
    let labels: Option<std::collections::BTreeMap<String, String>> = query
        .labels
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| (StatusCode::BAD_REQUEST, "维度过滤格式无效".into()))?;
    if labels.as_ref().is_some_and(|labels| {
        labels.is_empty()
            || labels.len() > 2
            || labels.iter().any(|(key, value)| {
                // 服务详情按动态 Pod UID 查询；该字段来自只读发现器，
                // 与父 Pod cgroup 一起过滤可避免同节点其他工作负载混入。
                ![
                    "mount",
                    "cgroup",
                    "pod_uid",
                    "pid",
                    "start_ticks",
                    "device",
                    "iface",
                ]
                .contains(&key.as_str())
                    || value.len() > 128
            })
    }) {
        return Err((StatusCode::BAD_REQUEST, "维度过滤无效".into()));
    }
    let rows = state
        .app
        .metrics
        .metrics(MetricRange {
            host_id: &query.host_id,
            category: query.category.as_deref(),
            from,
            to,
            limit,
            aggregate_only: query.aggregate_only.unwrap_or(false),
            step_ms: query.step_ms,
            labels: labels.as_ref(),
        })
        .await
        .map_err(server_error)?;
    Ok(Json(json!(rows)))
}

#[derive(Deserialize)]
struct ScoreQuery {
    host_id: String,
    scenario: String,
    from: Option<i64>,
    to: Option<i64>,
}

async fn scores(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ScoreQuery>,
) -> ApiResult<Json<Value>> {
    let scenario = Scenario::from_str(&query.scenario)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.into()))?;
    let now = chrono::Utc::now().timestamp_millis();
    let from = query.from.unwrap_or(now - 3_600_000);
    let to = query.to.unwrap_or(now);
    if query.host_id.is_empty()
        || from >= to
        || to - from > state.settings.retention.days * 86_400_000
    {
        return Err((StatusCode::BAD_REQUEST, "主机或时间范围无效".into()));
    }
    let rows = state
        .app
        .metrics
        .scores(&query.host_id, scenario, from, to)
        .await
        .map_err(server_error)?;
    Ok(Json(json!(rows)))
}

#[derive(Deserialize)]
struct NewProfile {
    host_id: String,
    pid: i32,
    duration_s: u32,
    frequency_hz: u32,
}

async fn create_profile(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<NewProfile>,
) -> ApiResult<Json<Value>> {
    let sender = state
        .streams
        .read()
        .await
        .get(&input.host_id)
        .cloned()
        .ok_or((StatusCode::CONFLICT, "Worker 不在线".into()))?;
    if input.pid <= 0
        || !(1..=60).contains(&input.duration_s)
        || !(1..=199).contains(&input.frequency_hz)
    {
        return Err((StatusCode::BAD_REQUEST, "perf 参数超出允许范围".into()));
    }
    let job = state
        .app
        .start_profile(
            input.host_id,
            input.pid,
            input.duration_s,
            input.frequency_hz,
        )
        .await
        .map_err(|error| {
            if error.to_string().contains("已有运行中的采样任务") {
                (StatusCode::CONFLICT, error.to_string())
            } else {
                server_error(error)
            }
        })?;
    let sent = sender
        .send(ServerFrame {
            body: Some(Body::ProfileCommand(ProfileCommand {
                job_id: job.id.clone(),
                pid: job.pid,
                duration_s: job.duration_s as u32,
                frequency_hz: job.frequency_hz as u32,
            })),
        })
        .await;
    if sent.is_err() {
        state
            .app
            .finish_profile(ProfileCompletion {
                id: &job.id,
                host_id: &job.host_id,
                success: false,
                error: "Worker 连接已断开",
                folded: "",
                sample_count: 0,
                finished_ms: chrono::Utc::now().timestamp_millis(),
            })
            .await
            .map_err(server_error)?;
        return Err((StatusCode::CONFLICT, "Worker 连接已断开".into()));
    }
    if let Some(actor) = current_user(&headers, &state).await {
        state.auth.record_action(&actor.id, "profile.start", &job.id,
            json!({"host_id":job.host_id,"pid":job.pid,"duration_s":job.duration_s,"frequency_hz":job.frequency_hz}))
            .await.map_err(server_error)?;
    }
    Ok(Json(json!({"id":job.id,"status":job.status})))
}

async fn profile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let job = state
        .app
        .profiles
        .get(&id)
        .await
        .map_err(server_error)?
        .ok_or((StatusCode::NOT_FOUND, "任务不存在".into()))?;
    Ok(Json(json!(job)))
}

#[derive(Deserialize)]
struct ProfileQuery {
    host_id: String,
}

async fn profiles(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProfileQuery>,
) -> ApiResult<Json<Value>> {
    if query.host_id.is_empty() || query.host_id.len() > 128 {
        return Err((StatusCode::BAD_REQUEST, "主机 ID 无效".into()));
    }
    Ok(Json(json!(
        state
            .app
            .profiles
            .list_recent(&query.host_id)
            .await
            .map_err(server_error)?
    )))
}

#[derive(Deserialize)]
struct NewAlert {
    host_id: String,
    metric: String,
    comparison: String,
    threshold: f64,
    duration_s: i32,
}

async fn create_alert(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<NewAlert>,
) -> ApiResult<Json<Value>> {
    if input.host_id.is_empty()
        || input.metric.is_empty()
        || input.metric.len() > 128
        || !["above", "below"].contains(&input.comparison.as_str())
        || !input.threshold.is_finite()
        || !(0..=3600).contains(&input.duration_s)
    {
        return Err((StatusCode::BAD_REQUEST, "告警规则无效".into()));
    }
    let rule = state
        .app
        .create_alert(
            input.host_id,
            input.metric,
            input.comparison,
            input.threshold,
            input.duration_s,
        )
        .await
        .map_err(server_error)?;
    if let Some(actor) = current_user(&headers, &state).await {
        state
            .auth
            .record_action(
                &actor.id,
                "alert.create",
                &rule.id,
                json!({"host_id":rule.host_id,"metric":rule.metric}),
            )
            .await
            .map_err(server_error)?;
    }
    Ok(Json(json!({"id":rule.id})))
}

#[derive(Deserialize)]
struct AlertUpdate {
    enabled: bool,
}

async fn update_alert(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<AlertUpdate>,
) -> ApiResult<Json<Value>> {
    let updated = state
        .app
        .alerts
        .set_enabled(&id, input.enabled, chrono::Utc::now().timestamp_millis())
        .await
        .map_err(server_error)?;
    if !updated {
        return Err((StatusCode::NOT_FOUND, "规则不存在".into()));
    }
    if let Some(actor) = current_user(&headers, &state).await {
        state
            .auth
            .record_action(
                &actor.id,
                "alert.update",
                &id,
                json!({"enabled":input.enabled}),
            )
            .await
            .map_err(server_error)?;
    }
    Ok(Json(json!({"id":id,"enabled":input.enabled})))
}

async fn alert_rules(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(
        state.app.alerts.list_rules().await.map_err(server_error)?
    )))
}

async fn alert_events(State(state): State<Arc<AppState>>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(
        state.app.alerts.list_events().await.map_err(server_error)?
    )))
}

async fn stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let encoded = headers
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .split(',')
                .map(str::trim)
                .find_map(|item| item.strip_prefix("auth."))
        });
    let token = encoded
        .and_then(decode_hex)
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let protocol_role = token.as_deref().and_then(|value| token_role(value, &state));
    let cookie_role = if same_origin(&headers, &state) {
        header_role(&headers, &state).await
    } else {
        None
    };
    if protocol_role.or(cookie_role).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.protocols(["po-v1"])
        .on_upgrade(move |socket| stream_loop(socket, state))
        .into_response()
}

/// 握手头只接受十六进制编码的令牌；无效字符或奇数长度一律拒绝。
fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() > 512 || !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            Some(((hi << 4) | lo) as u8)
        })
        .collect()
}

async fn stream_loop(mut socket: WebSocket, state: Arc<AppState>) {
    let mut receiver = state.app.events.subscribe();
    loop {
        match receiver.recv().await {
            Ok(text) => {
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}
