//! 邮件适配器。开发环境可接入 Mailpit，生产环境使用 SMTP TLS。
//!
//! 这里不生成验证码、不判定邮箱是否可注册；它们属于应用层。
//! 邮件正文显式声明 UTF-8 文本，避免中文验证码提示在收件箱中乱码。

use crate::{application::auth::MailSender, config::MailSettings};
use anyhow::Context;
use async_trait::async_trait;
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor, message::header::ContentType,
    transport::smtp::authentication::Credentials,
};

/// 单个可复用异步 SMTP 传输实例。
pub struct SmtpMailSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: String,
}

impl SmtpMailSender {
    /// 按 `none/starttls/tls` 建立传输配置。
    ///
    /// `none` 仅供本地测试邮箱使用，生产模式在配置校验阶段已经拒绝。
    pub fn new(settings: &MailSettings) -> anyhow::Result<Self> {
        let builder = match settings.security.as_str() {
            "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&settings.host)?,
            "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.host)?,
            // 仅供绑定本机的 Mailpit 开发实例使用；生产配置校验禁止 none。
            "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.host),
            _ => anyhow::bail!("未知 SMTP 安全模式"),
        };
        let builder = builder.port(settings.port);
        let builder = if settings.username.is_empty() {
            builder
        } else {
            builder.credentials(Credentials::new(
                settings.username.clone(),
                settings.password.clone(),
            ))
        };
        Ok(Self {
            transport: builder.build(),
            from: settings.from.clone(),
        })
    }
}

#[async_trait]
impl MailSender for SmtpMailSender {
    async fn send_registration_code(&self, email: &str, code: &str) -> anyhow::Result<()> {
        let message = Message::builder()
            .from(self.from.parse().context("发件人地址无效")?)
            .to(email.parse().context("收件人地址无效")?)
            .subject("Linux-Pilot 注册验证码")
            .header(ContentType::TEXT_PLAIN)
            .body(format!(
                "你的 Linux-Pilot 注册验证码是 {code}。10 分钟内有效，请勿转发给任何人。\n\n如果不是你发起的注册，请忽略此邮件。"
            ))?;
        self.transport
            .send(message)
            .await
            .context("SMTP 发送失败")?;
        Ok(())
    }
}
