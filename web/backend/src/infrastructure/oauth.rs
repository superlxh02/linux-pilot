//! GitHub 与 Google 的固定 OAuth 端点适配器；从服务端交换授权码，不向浏览器暴露 Client Secret。
//!
//! 授权 URL 和令牌端点都是代码内的固定常量，不接受浏览器传入地址，
//! 避免认证流程变成任意 URL 请求。适配器只返回已验证邮箱的资料。

use crate::{
    application::auth::{OAuthGateway, OAuthProfile},
    config::{AuthSettings, OAuthSettings},
};
use anyhow::{Context, ensure};
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde::Deserialize;
use std::time::Duration;

/// 通过同一个带超时的 HTTP 客户端访问受支持的身份提供方。
pub struct RemoteOAuthGateway {
    client: Client,
    settings: AuthSettings,
}

impl RemoteOAuthGateway {
    /// 创建带 10 秒总请求超时的客户端，防止提供方故障占满服务端任务。
    pub fn new(settings: AuthSettings) -> anyhow::Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("Linux-Pilot/0.1")
            .build()?;
        Ok(Self { client, settings })
    }

    fn config(&self, provider: &str) -> Option<&OAuthSettings> {
        match provider {
            "github" => Some(&self.settings.github),
            "google" => Some(&self.settings.google),
            _ => None,
        }
    }

    fn redirect_uri(&self, provider: &str) -> String {
        // 回调必须由配置的公开站点地址构造，和提供方控制台登记值一致。
        format!(
            "{}/api/v1/auth/oauth/{provider}/callback",
            self.settings.public_url.trim_end_matches('/')
        )
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GithubUser {
    id: u64,
    login: String,
    name: Option<String>,
}

#[derive(Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

#[derive(Deserialize)]
struct GoogleUser {
    sub: String,
    email: String,
    email_verified: bool,
    name: Option<String>,
}

#[async_trait]
impl OAuthGateway for RemoteOAuthGateway {
    fn enabled(&self, provider: &str) -> bool {
        self.config(provider)
            .is_some_and(|value| !value.client_id.is_empty() && !value.client_secret.is_empty())
    }

    fn authorization_url(
        &self,
        provider: &str,
        state: &str,
        challenge: &str,
    ) -> anyhow::Result<String> {
        // PKCE challenge 与 state 由应用层生成；这里仅使用固定端点拼接
        // 标准参数，不让未验证的 provider 值进入 URL 主机部分。
        let config = self.config(provider).context("不支持的 OAuth 提供方")?;
        ensure!(self.enabled(provider), "OAuth 提供方未配置");
        let (endpoint, scope) = match provider {
            "github" => (
                "https://github.com/login/oauth/authorize",
                "read:user user:email",
            ),
            "google" => (
                "https://accounts.google.com/o/oauth2/v2/auth",
                "openid email profile",
            ),
            _ => anyhow::bail!("不支持的 OAuth 提供方"),
        };
        let mut url = Url::parse(endpoint)?;
        url.query_pairs_mut()
            .append_pair("client_id", &config.client_id)
            .append_pair("redirect_uri", &self.redirect_uri(provider))
            .append_pair("response_type", "code")
            .append_pair("scope", scope)
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(url.into())
    }

    async fn profile(
        &self,
        provider: &str,
        code: &str,
        verifier: &str,
    ) -> anyhow::Result<OAuthProfile> {
        // 授权码只在后端交换；客户端密钥不会出现在浏览器地址或响应中。
        let config = self.config(provider).context("不支持的 OAuth 提供方")?;
        let endpoint = match provider {
            "github" => "https://github.com/login/oauth/access_token",
            "google" => "https://oauth2.googleapis.com/token",
            _ => anyhow::bail!("不支持的 OAuth 提供方"),
        };
        let token: TokenResponse = self
            .client
            .post(endpoint)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", config.client_id.as_str()),
                ("client_secret", config.client_secret.as_str()),
                ("code", code),
                ("redirect_uri", &self.redirect_uri(provider)),
                ("code_verifier", verifier),
                ("grant_type", "authorization_code"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("交换 OAuth 授权码失败")?;
        ensure!(
            !token.access_token.is_empty(),
            "OAuth 没有返回 access_token"
        );
        if provider == "github" {
            let person: GithubUser = self
                .client
                .get("https://api.github.com/user")
                .bearer_auth(&token.access_token)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let emails: Vec<GithubEmail> = self
                .client
                .get("https://api.github.com/user/emails")
                .bearer_auth(&token.access_token)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            // GitHub /user 的 public email 可能为空或未验证，必须再查
            // /user/emails 并优先选择已验证的主邮箱。
            let verified = emails
                .iter()
                .find(|item| item.primary && item.verified)
                .or_else(|| emails.iter().find(|item| item.verified))
                .context("GitHub 账号没有可用的已验证邮箱")?;
            Ok(OAuthProfile {
                provider_user_id: person.id.to_string(),
                email: verified.email.clone(),
                display_name: person
                    .name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(person.login),
            })
        } else {
            let person: GoogleUser = self
                .client
                .get("https://openidconnect.googleapis.com/v1/userinfo")
                .bearer_auth(&token.access_token)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            // Google userinfo 的 sub 是稳定身份；email_verified 是创建
            // 本地账号的前置条件，不能只相信 email 字符串存在。
            ensure!(person.email_verified, "Google 邮箱未验证");
            Ok(OAuthProfile {
                provider_user_id: person.sub,
                display_name: person.name.unwrap_or_else(|| person.email.clone()),
                email: person.email,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_urls_include_callback_state_and_pkce() {
        let settings: AuthSettings = serde_json::from_value(serde_json::json!({
            "viewer_token":"viewer", "operator_token":"operator",
            "public_url":"https://po.example.com", "admin_password":"admin-0123456789abcdef0123456789abcdef",
            "code_pepper":"pepper", "session_days":7,
            "mail":{"host":"smtp.example.com","port":587,"username":"a","password":"b","from":"a@example.com","security":"starttls"},
            "github":{"client_id":"github-id","client_secret":"secret"},
            "google":{"client_id":"google-id","client_secret":"secret"}
        })).unwrap();
        let gateway = RemoteOAuthGateway::new(settings).unwrap();
        for provider in ["github", "google"] {
            let url = Url::parse(
                &gateway
                    .authorization_url(provider, "state123", "challenge123")
                    .unwrap(),
            )
            .unwrap();
            let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(params.get("state").map(String::as_str), Some("state123"));
            assert_eq!(
                params.get("code_challenge_method").map(String::as_str),
                Some("S256")
            );
            assert_eq!(
                params.get("redirect_uri").map(String::as_str),
                Some(
                    format!("https://po.example.com/api/v1/auth/oauth/{provider}/callback")
                        .as_str()
                )
            );
        }
    }
}
