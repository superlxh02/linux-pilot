//! 账号认证用例。密码、验证码和 OAuth 状态只在本模块编排；持久化与外部服务走端口。
//!
//! 认证材料在数据库中仅保存哈希或短期状态：Argon2id 密码哈希、带服务端
//! pepper 的验证码哈希、SHA-256 会话令牌哈希、一次性 OAuth state 哈希。
//! HTTP Cookie、SMTP 和第三方接口属于外层适配，应用用例只处理业务语义。

use crate::config::AuthSettings;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::{Arc, OnceLock};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;
use tokio::task;

#[derive(Debug, Clone)]
/// 已验证账号的领域视图；对外 JSON 不序列化 `password_hash`。
pub struct AuthUser {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub password_hash: Option<String>,
}

/// 管理员页面使用的公开账号资料，刻意不包含密码哈希和会话令牌。
#[derive(Debug, Clone, Serialize)]
pub struct ManagedUser {
    pub id: String,
    pub username: Option<String>,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub created_ms: i64,
    pub last_login_ms: Option<i64>,
}

/// 用户活动记录。`details` 只放可审计的非敏感参数，不记录密码、令牌或验证码。
#[derive(Debug, Clone, Serialize)]
pub struct UserActivity {
    pub id: String,
    pub action: String,
    pub target: String,
    pub actor_name: Option<String>,
    pub details: serde_json::Value,
    pub created_ms: i64,
}

#[derive(Debug, Clone)]
/// 第三方返回且邮箱已由适配器确认的稳定身份。
pub struct OAuthProfile {
    pub provider_user_id: String,
    pub email: String,
    pub display_name: String,
}

#[derive(Debug)]
/// 认证用例的可分类失败；HTTP 层据此映射状态码和安全的用户提示。
pub enum AuthFailure {
    InvalidInput(&'static str),
    InvalidCredentials,
    InvalidCode,
    RateLimited,
    Conflict(&'static str),
    ProviderDisabled,
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for AuthFailure {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

#[async_trait]
/// 账号持久化端口；并发限制和一次性消费必须由实现端保证原子性。
///
/// `reserve_code`、`login_allowed` 使用数据库原子 UPSERT，避免多副本
/// 同时请求绕过频率限制。`register` 在同一事务中消耗验证码并创建账号。
pub trait AuthStore: Send + Sync {
    async fn find_user(&self, email: &str) -> anyhow::Result<Option<AuthUser>>;
    async fn find_username(&self, username: &str) -> anyhow::Result<Option<AuthUser>>;
    async fn bootstrap_admin(&self, password_hash: &str, now: i64) -> anyhow::Result<()>;
    async fn list_users(&self, limit: i64, offset: i64) -> anyhow::Result<Vec<ManagedUser>>;
    async fn user_activity(&self, user_id: &str, limit: i64) -> anyhow::Result<Vec<UserActivity>>;
    async fn set_role(
        &self,
        actor_id: &str,
        user_id: &str,
        role: &str,
        now: i64,
    ) -> anyhow::Result<bool>;
    async fn record_action(
        &self,
        actor_id: &str,
        action: &str,
        target: &str,
        details: serde_json::Value,
        now: i64,
    ) -> anyhow::Result<()>;
    async fn reserve_code(&self, email: &str, hash: &str, now: i64) -> anyhow::Result<bool>;
    async fn remove_code(&self, email: &str, hash: &str) -> anyhow::Result<()>;
    async fn attempt_code(&self, email: &str, now: i64) -> anyhow::Result<Option<String>>;
    async fn register(&self, user: &AuthUser, code_hash: &str, now: i64) -> anyhow::Result<bool>;
    async fn login_allowed(&self, email: &str, now: i64) -> anyhow::Result<bool>;
    async fn clear_login_failures(&self, email: &str) -> anyhow::Result<()>;
    async fn create_session(
        &self,
        hash: &str,
        user_id: &str,
        now: i64,
        expires: i64,
    ) -> anyhow::Result<()>;
    async fn session_user(&self, hash: &str, now: i64) -> anyhow::Result<Option<AuthUser>>;
    async fn revoke_session(&self, hash: &str) -> anyhow::Result<()>;
    async fn save_oauth_state(
        &self,
        hash: &str,
        provider: &str,
        verifier: &str,
        expires: i64,
    ) -> anyhow::Result<()>;
    async fn consume_oauth_state(
        &self,
        hash: &str,
        provider: &str,
        now: i64,
    ) -> anyhow::Result<Option<String>>;
    async fn oauth_user(
        &self,
        provider: &str,
        profile: &OAuthProfile,
        role: &str,
        now: i64,
    ) -> anyhow::Result<Option<AuthUser>>;
    async fn cleanup(&self, now: i64) -> anyhow::Result<()>;
}

#[async_trait]
/// 邮件发送端口，业务层不依赖 SMTP 库或具体测试收件箱。
pub trait MailSender: Send + Sync {
    async fn send_registration_code(&self, email: &str, code: &str) -> anyhow::Result<()>;
}

#[async_trait]
/// GitHub/Google 网关端口；适配器负责固定端点、授权码交换和邮箱验证。
pub trait OAuthGateway: Send + Sync {
    fn enabled(&self, provider: &str) -> bool;
    fn authorization_url(
        &self,
        provider: &str,
        state: &str,
        challenge: &str,
    ) -> anyhow::Result<String>;
    async fn profile(
        &self,
        provider: &str,
        code: &str,
        verifier: &str,
    ) -> anyhow::Result<OAuthProfile>;
}

/// 认证应用服务，协调账户状态、密码计算和外部提供方。
///
/// 密码哈希在线程池运行，同时用信号量限制最多 8 个并发任务，防止
/// 匿名请求耗尽 CPU/内存，拖慢指标接入和实时查询。
pub struct AuthService {
    store: Arc<dyn AuthStore>,
    mail: Arc<dyn MailSender>,
    oauth: Arc<dyn OAuthGateway>,
    settings: AuthSettings,
    password_slots: Semaphore,
}

impl AuthService {
    /// 组装持久化、邮件与 OAuth 端口；设置值在启动时完成校验。
    pub fn new(
        store: Arc<dyn AuthStore>,
        mail: Arc<dyn MailSender>,
        oauth: Arc<dyn OAuthGateway>,
        settings: AuthSettings,
    ) -> Self {
        Self {
            store,
            mail,
            oauth,
            settings,
            password_slots: Semaphore::new(8),
        }
    }

    /// 返回已完整配置的第三方提供方，用于前端启用或禁用入口。
    pub fn providers(&self) -> (bool, bool) {
        (self.oauth.enabled("github"), self.oauth.enabled("google"))
    }

    /// 首次启动创建管理员，或在配置的密码发生变化时更新哈希。
    ///
    /// 每次启动先验证现有 Argon2id 哈希，只有密码确实变化时才写库。
    /// 仓储在同一事务中撤销旧会话，避免已签发的 Cookie 继续使用旧身份。
    pub async fn bootstrap_admin(&self) -> anyhow::Result<()> {
        if let Some(current) = self.store.find_username("admin").await?
            && current.role == "admin"
            && verify_password(self.settings.admin_password.clone(), current.password_hash)
                .await
                .map_err(|error| anyhow::anyhow!("验证管理员初始密码失败: {error:?}"))?
        {
            return Ok(());
        }
        let hash = hash_password(self.settings.admin_password.clone())
            .await
            .map_err(|error| anyhow::anyhow!("管理员密码哈希失败: {error:?}"))?;
        self.store.bootstrap_admin(&hash, now_ms()).await?;
        Ok(())
    }

    pub async fn list_users(&self, limit: i64, offset: i64) -> anyhow::Result<Vec<ManagedUser>> {
        self.store.list_users(limit, offset).await
    }

    pub async fn user_activity(
        &self,
        user_id: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<UserActivity>> {
        self.store.user_activity(user_id, limit).await
    }

    pub async fn set_role(
        &self,
        actor_id: &str,
        user_id: &str,
        role: &str,
    ) -> anyhow::Result<bool> {
        self.store.set_role(actor_id, user_id, role, now_ms()).await
    }

    pub async fn record_action(
        &self,
        actor_id: &str,
        action: &str,
        target: &str,
        details: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.store
            .record_action(actor_id, action, target, details, now_ms())
            .await
    }

    /// 只有 HTTPS 站点才能设置 Secure Cookie；本地 HTTP 保持可联调。
    pub fn secure_cookie(&self) -> bool {
        self.settings.public_url.starts_with("https://")
    }

    /// 返回会话 Cookie 有效期，和数据库 `expires_ms` 使用同一配置。
    pub fn session_max_age_secs(&self) -> i64 {
        self.settings.session_days * 86_400
    }

    /// 为尚未注册的邮箱发送一次性验证码。
    ///
    /// 已存在账号返回相同成功提示，避免通过响应内容枚举用户。发送频率
    /// 由仓储端原子限制；邮件发送失败时撤销刚写入的验证码记录。
    pub async fn send_code(&self, raw_email: &str) -> Result<(), AuthFailure> {
        let email = normalize_email(raw_email)?;
        if self.store.find_user(&email).await?.is_some() {
            // 对已注册邮箱返回相同结果，避免把账号存在性暴露给匿名请求。
            return Ok(());
        }
        let code = format!("{:06}", random_u32()? % 1_000_000);
        let hash = code_hash(&self.settings.code_pepper, &email, &code);
        let now = now_ms();
        if !self.store.reserve_code(&email, &hash, now).await? {
            return Err(AuthFailure::RateLimited);
        }
        if let Err(error) = self.mail.send_registration_code(&email, &code).await {
            self.store.remove_code(&email, &hash).await?;
            return Err(AuthFailure::Internal(error.context("发送注册邮件失败")));
        }
        Ok(())
    }

    /// 验证邮箱和密码规则后注册账号，并立即签发会话。
    ///
    /// 邮箱注册账号默认只读；管理员随后可在用户管理页面分配角色。
    /// 验证码尝试次数由仓储端限制，创建账号与消耗验证码在同一事务完成。
    pub async fn register(
        &self,
        raw_email: &str,
        display_name: &str,
        password: &str,
        code: &str,
    ) -> Result<(AuthUser, String), AuthFailure> {
        let email = normalize_email(raw_email)?;
        let display_name = display_name.trim();
        if !(2..=64).contains(&display_name.chars().count())
            || display_name.chars().any(char::is_control)
        {
            return Err(AuthFailure::InvalidInput("昵称需要 2～64 个字符"));
        }
        if !(12..=128).contains(&password.len()) {
            return Err(AuthFailure::InvalidInput("密码需要 12～128 字节"));
        }
        if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(AuthFailure::InvalidCode);
        }
        let now = now_ms();
        let expected = self.store.attempt_code(&email, now).await?;
        let hash = code_hash(&self.settings.code_pepper, &email, code);
        if !expected
            .as_deref()
            .is_some_and(|value| bool::from(value.as_bytes().ct_eq(hash.as_bytes())))
        {
            return Err(AuthFailure::InvalidCode);
        }
        let _slot = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            self.password_slots.acquire(),
        )
        .await
        .map_err(|_| AuthFailure::RateLimited)?
        .map_err(|error| AuthFailure::Internal(error.into()))?;
        let password_hash = hash_password(password.to_owned()).await?;
        let role = "viewer".to_owned();
        let user = AuthUser {
            id: uuid::Uuid::new_v4().to_string(),
            email,
            display_name: display_name.into(),
            role,
            password_hash: Some(password_hash),
        };
        if !self.store.register(&user, &hash, now).await? {
            return Err(AuthFailure::Conflict("邮箱已注册或验证码已失效"));
        }
        let session = self.issue_session(&user.id).await?;
        Ok((user, session))
    }

    /// 验证邮箱密码并签发随机会话令牌。
    ///
    /// 未知邮箱和仅有 OAuth 身份的账号也执行一次 Argon2 校验以缩小
    /// 时间差，但占位哈希绝不允许它们通过密码登录。
    pub async fn login(
        &self,
        raw_email: &str,
        password: &str,
    ) -> Result<(AuthUser, String), AuthFailure> {
        let login = raw_email.trim().to_ascii_lowercase();
        let email = if login == "admin" {
            login
        } else {
            normalize_email(raw_email)?
        };
        let now = now_ms();
        if !self.store.login_allowed(&email, now).await? {
            return Err(AuthFailure::RateLimited);
        }
        let user = if email == "admin" {
            self.store.find_username("admin").await?
        } else {
            self.store.find_user(&email).await?
        };
        let hash = user.as_ref().and_then(|item| item.password_hash.as_deref());
        // 缺失账号也运行一次 Argon2，减小根据响应时间枚举邮箱的机会。
        let _slot = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            self.password_slots.acquire(),
        )
        .await
        .map_err(|_| AuthFailure::RateLimited)?
        .map_err(|error| AuthFailure::Internal(error.into()))?;
        let valid = verify_password(password.to_owned(), hash.map(str::to_owned)).await?;
        let Some(user) = user.filter(|_| valid) else {
            return Err(AuthFailure::InvalidCredentials);
        };
        self.store.clear_login_failures(&email).await?;
        self.store
            .record_action(&user.id, "auth.login", &user.id, serde_json::json!({}), now)
            .await?;
        let session = self.issue_session(&user.id).await?;
        Ok((user, session))
    }

    /// 通过令牌哈希查找有效会话；无效长度先本地拒绝，避免无谓数据库查询。
    pub async fn session(&self, token: &str) -> anyhow::Result<Option<AuthUser>> {
        if token.len() != 43 {
            return Ok(None);
        }
        self.store
            .session_user(&digest(token.as_bytes()), now_ms())
            .await
    }

    /// 立即撤销数据库会话，Cookie 清理由 HTTP 适配器负责。
    pub async fn logout(&self, token: &str) -> anyhow::Result<()> {
        if token.len() == 43 {
            if let Some(user) = self.session(token).await? {
                self.store
                    .record_action(
                        &user.id,
                        "auth.logout",
                        &user.id,
                        serde_json::json!({}),
                        now_ms(),
                    )
                    .await?;
            }
            self.store.revoke_session(&digest(token.as_bytes())).await?;
        }
        Ok(())
    }

    /// 生成一次性 state 和 PKCE verifier，并返回固定提供方授权地址。
    ///
    /// 数据库只持久化 state 哈希和短期 verifier；浏览器收到的 state
    /// 同时保存在专用 HttpOnly Cookie 中，回调必须匹配两者。
    pub async fn start_oauth(&self, provider: &str) -> Result<(String, String), AuthFailure> {
        if !self.oauth.enabled(provider) {
            return Err(AuthFailure::ProviderDisabled);
        }
        let state = random_token()?;
        let verifier = random_token()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let url = self.oauth.authorization_url(provider, &state, &challenge)?;
        self.store
            .save_oauth_state(
                &digest(state.as_bytes()),
                provider,
                &verifier,
                now_ms() + 600_000,
            )
            .await?;
        Ok((url, state))
    }

    /// 一次性消费 state，交换用户资料并建立本地会话。
    ///
    /// 已有同邮箱账号不会自动绑定到新提供方，避免邮箱变更或错误映射
    /// 造成账户接管；只按提供方稳定用户 ID 识别已有 OAuth 账号。
    pub async fn finish_oauth(
        &self,
        provider: &str,
        state: &str,
        code: &str,
    ) -> Result<(AuthUser, String), AuthFailure> {
        if !self.oauth.enabled(provider) {
            return Err(AuthFailure::ProviderDisabled);
        }
        let verifier = self
            .store
            .consume_oauth_state(&digest(state.as_bytes()), provider, now_ms())
            .await?
            .ok_or(AuthFailure::InvalidCredentials)?;
        let profile = self.oauth.profile(provider, code, &verifier).await?;
        let email = normalize_email(&profile.email)?;
        let role = "viewer";
        let user = self
            .store
            .oauth_user(provider, &OAuthProfile { email, ..profile }, role, now_ms())
            .await?
            .ok_or(AuthFailure::Conflict("此邮箱已注册，请先使用原方式登录"))?;
        self.store
            .record_action(
                &user.id,
                "auth.login.oauth",
                &user.id,
                serde_json::json!({"provider":provider}),
                now_ms(),
            )
            .await?;
        let session = self.issue_session(&user.id).await?;
        Ok((user, session))
    }

    pub async fn cleanup(&self) -> anyhow::Result<()> {
        self.store.cleanup(now_ms()).await
    }

    async fn issue_session(&self, user_id: &str) -> Result<String, AuthFailure> {
        let token = random_token()?;
        let now = now_ms();
        let expires = now + self.settings.session_days * 86_400_000;
        self.store
            .create_session(&digest(token.as_bytes()), user_id, now, expires)
            .await?;
        Ok(token)
    }
}

fn normalize_email(value: &str) -> Result<String, AuthFailure> {
    let email = value.trim().to_ascii_lowercase();
    if email.len() > 254
        || email.len() < 5
        || email.contains(char::is_whitespace)
        || email
            .split_once('@')
            .is_none_or(|(local, domain)| local.is_empty() || !domain.contains('.'))
    {
        return Err(AuthFailure::InvalidInput("邮箱格式无效"));
    }
    Ok(email)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn code_hash(pepper: &str, email: &str, code: &str) -> String {
    digest(format!("{pepper}:{email}:{code}").as_bytes())
}

fn random_u32() -> Result<u32, AuthFailure> {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes)
        .map_err(|error| AuthFailure::Internal(anyhow::anyhow!(error.to_string())))?;
    Ok(u32::from_le_bytes(bytes))
}

fn random_token() -> Result<String, AuthFailure> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| AuthFailure::Internal(anyhow::anyhow!(error.to_string())))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

async fn hash_password(password: String) -> Result<String, AuthFailure> {
    task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|value| value.to_string())
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    })
    .await
    .map_err(|error| AuthFailure::Internal(error.into()))?
    .map_err(AuthFailure::Internal)
}

async fn verify_password(password: String, hash: Option<String>) -> Result<bool, AuthFailure> {
    static DUMMY_HASH: OnceLock<Result<String, String>> = OnceLock::new();
    task::spawn_blocking(move || {
        let has_local_password = hash.is_some();
        let stored = match hash.as_deref() {
            Some(stored) => stored,
            None => DUMMY_HASH
                .get_or_init(|| {
                    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
                    Argon2::default()
                        .hash_password(b"dummy-account-password", &salt)
                        .map(|value| value.to_string())
                        .map_err(|error| error.to_string())
                })
                .as_ref()
                .map_err(|error| anyhow::anyhow!(error.clone()))?,
        };
        let valid = PasswordHash::new(stored).ok().is_some_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        });
        // 占位哈希只用于平衡响应耗时；OAuth 账号绝不能靠它通过密码登录。
        Ok::<bool, anyhow::Error>(has_local_password && valid)
    })
    .await
    .map_err(|error| AuthFailure::Internal(error.into()))?
    .map_err(AuthFailure::Internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn argon2_hash_verifies_only_original_password() {
        let hash = hash_password("StrongLocalPass!2026".into()).await.unwrap();
        assert!(
            verify_password("StrongLocalPass!2026".into(), Some(hash.clone()))
                .await
                .unwrap()
        );
        assert!(
            !verify_password("wrong-password".into(), Some(hash))
                .await
                .unwrap()
        );
        assert!(!verify_password("anything".into(), None).await.unwrap());
        assert!(
            !verify_password("dummy-account-password".into(), None)
                .await
                .unwrap()
        );
    }

    #[test]
    fn email_normalization_and_code_pepper_are_stable() {
        let email = normalize_email(" Admin@Example.COM ").unwrap();
        assert_eq!(email, "admin@example.com");
        assert_ne!(
            code_hash("pepper-one", &email, "123456"),
            code_hash("pepper-two", &email, "123456")
        );
    }
}
