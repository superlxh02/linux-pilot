//! 与 Spring Boot 相近的层级 YAML 配置：文件为基线，环境变量为部署覆盖层。
//!
//! 配置的所有环境差异都在此模块集中校验。`PO__DATABASE__URL` 一类变量
//! 覆盖 `appliaction.yaml` 的同名层级项；该前缀是现有部署接口，项目更名后
//! 保留以避免采集端与 Web 配置在滚动升级时失配。生产模式启动前拒绝明显
//! 的示例口令、明文邮箱和非 HTTPS 外部地址。

use anyhow::{Context, ensure};
use config::{Config, Environment, File};
use serde::Deserialize;
use std::{env, path::Path};

#[derive(Debug, Clone, Deserialize)]
/// 应用运行时的完整配置快照；启动成功后不在请求处理中读取环境变量。
pub struct Settings {
    pub app: AppSettings,
    pub server: ServerSettings,
    pub database: DatabaseSettings,
    pub auth: AuthSettings,
    pub worker: WorkerSettings,
    pub retention: RetentionSettings,
    pub scoring: ScoringSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppSettings {
    pub mode: String,
}

#[derive(Debug, Clone, Deserialize)]
/// HTTP/gRPC 监听地址及可选的 gRPC TLS 证书路径。
pub struct ServerSettings {
    pub http_addr: String,
    pub grpc_addr: String,
    pub tls_cert_path: Option<String>,
    pub tls_key_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseSettings {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Deserialize)]
/// 账号、邮件和第三方登录配置；Client Secret 只留在后端进程。
pub struct AuthSettings {
    pub viewer_token: String,
    pub operator_token: String,
    pub public_url: String,
    pub admin_password: String,
    pub code_pepper: String,
    pub session_days: i64,
    pub mail: MailSettings,
    pub github: OAuthSettings,
    pub google: OAuthSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MailSettings {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
    pub security: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OAuthSettings {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkerSettings {
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetentionSettings {
    pub days: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScoringSettings {
    pub persist_every_secs: i64,
}

impl Settings {
    /// 读取 YAML 后再覆盖环境变量，并在服务监听端口前完成完整校验。
    ///
    /// 默认路径以独立 `web/` Cargo 工作区为当前目录；Docker 镜像通过
    /// `PO_CONFIG` 指向容器内配置文件。错误会附上路径或字段语义。
    pub fn load() -> anyhow::Result<Self> {
        let path = env::var("PO_CONFIG").unwrap_or_else(|_| "backend/appliaction.yaml".into());
        let settings: Self = Config::builder()
            .add_source(File::from(Path::new(&path)))
            .add_source(Environment::with_prefix("PO").separator("__"))
            .build()
            .with_context(|| format!("读取配置失败: {path}"))?
            .try_deserialize()
            .context("配置字段类型不正确")?;
        settings.validate()?;
        Ok(settings)
    }

    /// 阻止无效配置进入请求路径，特别是生产环境误用开发凭据。
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            matches!(self.app.mode.as_str(), "local" | "production"),
            "app.mode 只能为 local 或 production"
        );
        ensure!(self.database.max_connections > 0, "数据库连接数必须大于 0");
        ensure!(
            (1..=365).contains(&self.retention.days),
            "保留天数必须在 1～365 之间"
        );
        ensure!(
            (1..=3600).contains(&self.scoring.persist_every_secs),
            "评分持久化间隔无效"
        );
        ensure!(
            !self.auth.viewer_token.is_empty() && !self.auth.operator_token.is_empty(),
            "访问令牌不能为空"
        );
        ensure!(
            self.auth.viewer_token != self.auth.operator_token,
            "只读与操作令牌必须不同"
        );
        ensure!(!self.worker.token.is_empty(), "Worker 令牌不能为空");
        ensure!(
            self.worker.token != self.auth.viewer_token
                && self.worker.token != self.auth.operator_token,
            "Worker 令牌不能与控制台令牌共用"
        );
        let public_url =
            reqwest::Url::parse(&self.auth.public_url).context("auth.public_url 必须为绝对 URL")?;
        ensure!(
            matches!(public_url.scheme(), "http" | "https")
                && public_url.host_str().is_some()
                && public_url.path() == "/"
                && public_url.query().is_none(),
            "auth.public_url 只允许站点根地址"
        );
        ensure!(
            (1..=30).contains(&self.auth.session_days),
            "会话有效期必须为 1～30 天"
        );
        ensure!(
            self.auth.admin_password.len() >= 20
                && self.auth.admin_password.len() <= 128
                && !self.auth.admin_password.starts_with("change-me")
                && !self.auth.admin_password.starts_with("replace-this"),
            "管理员密码需要 20～128 字节，不能使用示例占位值"
        );
        ensure!(
            matches!(
                self.auth.mail.security.as_str(),
                "none" | "starttls" | "tls"
            ) && !self.auth.mail.host.is_empty()
                && self.auth.mail.port > 0,
            "邮件服务配置无效"
        );
        for provider in [&self.auth.github, &self.auth.google] {
            ensure!(
                provider.client_id.is_empty() == provider.client_secret.is_empty(),
                "OAuth Client ID 与 Secret 必须同时配置"
            );
        }
        ensure!(
            self.server.tls_cert_path.is_some() == self.server.tls_key_path.is_some(),
            "gRPC TLS 证书与私钥必须成对配置"
        );
        if self.app.mode == "production" {
            // 示例文件里的占位符容易被误当成真实密钥，生产启动时统一拒绝。
            ensure!(
                self.auth.admin_password
                    != "admin-5a2c966a29f39ec1248ae6ba480ae765158d26fef745e139",
                "生产模式必须覆盖公开的初始管理员密码"
            );
            ensure!(
                [
                    &self.auth.viewer_token,
                    &self.auth.operator_token,
                    &self.worker.token
                ]
                .iter()
                .all(|value| {
                    value.len() >= 32
                        && !value.starts_with("change-me")
                        && !value.starts_with("replace-this")
                }),
                "生产模式必须为三种角色分别配置至少 32 字节的非示例令牌"
            );
            ensure!(
                self.server.tls_cert_path.is_some(),
                "生产模式必须开启 gRPC TLS"
            );
            ensure!(
                public_url.scheme() == "https"
                    && self.auth.code_pepper.len() >= 32
                    && !self.auth.code_pepper.starts_with("change-me")
                    && !self.auth.code_pepper.starts_with("replace-this")
                    && self.auth.mail.security != "none"
                    && self.auth.mail.host != "mailpit"
                    && !self.auth.mail.username.is_empty()
                    && !self.auth.mail.password.is_empty(),
                "生产模式需要 HTTPS、真实 SMTP 和至少 32 字节的验证码密钥"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_rejects_example_credentials_and_accepts_distinct_secrets() {
        let mut settings = Settings {
            app: AppSettings {
                mode: "production".into(),
            },
            server: ServerSettings {
                http_addr: "127.0.0.1:8080".into(),
                grpc_addr: "127.0.0.1:50051".into(),
                tls_cert_path: Some("cert.pem".into()),
                tls_key_path: Some("key.pem".into()),
            },
            database: DatabaseSettings {
                url: "postgres://localhost/po".into(),
                max_connections: 8,
            },
            auth: AuthSettings {
                viewer_token: "replace-this-viewer-token".into(),
                operator_token: "b".repeat(32),
                public_url: "https://po.example.com".into(),
                admin_password: "admin-0123456789abcdef0123456789abcdef".into(),
                code_pepper: "p".repeat(32),
                session_days: 7,
                mail: MailSettings {
                    host: "smtp.example.com".into(),
                    port: 587,
                    username: "admin".into(),
                    password: "secret".into(),
                    from: "no-reply@example.com".into(),
                    security: "starttls".into(),
                },
                github: OAuthSettings {
                    client_id: String::new(),
                    client_secret: String::new(),
                },
                google: OAuthSettings {
                    client_id: String::new(),
                    client_secret: String::new(),
                },
            },
            worker: WorkerSettings {
                token: "c".repeat(32),
            },
            retention: RetentionSettings { days: 7 },
            scoring: ScoringSettings {
                persist_every_secs: 10,
            },
        };
        assert!(settings.validate().is_err());

        settings.auth.viewer_token = "a".repeat(32);
        assert!(settings.validate().is_ok());
        settings.auth.admin_password =
            "admin-5a2c966a29f39ec1248ae6ba480ae765158d26fef745e139".into();
        assert!(settings.validate().is_err());
        settings.auth.admin_password = "admin-0123456789abcdef0123456789abcdef".into();
        settings.worker.token = settings.auth.viewer_token.clone();
        assert!(settings.validate().is_err());
    }
}
