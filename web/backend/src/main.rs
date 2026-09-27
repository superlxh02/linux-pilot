//! Linux-Pilot 中心服务入口。
//!
//! 单进程同时提供 Axum HTTP/WebSocket 与 tonic gRPC；两者共享应用服务和
//! PostgreSQL 仓储，但协议握手、授权和序列化分别留在接口层。后台任务只
//! 清理过期状态与失联任务，不参与请求事务边界。

mod application;
mod config;
mod infrastructure;
mod interfaces;

use anyhow::Context;
use application::{
    auth::{AuthService, AuthStore, MailSender, OAuthGateway},
    ports::{AlertRepository, MetricRepository, ProfileRepository},
    service::Application,
};
use axum::Router;
use config::Settings;
use sea_orm::{ConnectOptions, Database};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{RwLock, mpsc};
use tonic::transport::Server;
use tracing::info;

/// HTTP 与 gRPC 共享的进程状态。
///
/// `streams` 仅记录当前 Worker 连接，断线后可重建；它不是事实来源。
/// 指标、评分、认证会话及任务状态由数据库保存，重启后仍可恢复。
pub(crate) struct AppState {
    pub app: Arc<Application>,
    pub auth: Arc<AuthService>,
    pub settings: Settings,
    pub streams: RwLock<HashMap<String, mpsc::Sender<linux_pilot_wire::agent::ServerFrame>>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "linux_pilot_server=info,tower_http=info".into()),
        )
        .json()
        .init();
    let settings = Settings::load()?;
    let mut database_options = ConnectOptions::new(settings.database.url.clone());
    database_options.max_connections(settings.database.max_connections);
    let db = Database::connect(database_options)
        .await
        .context("连接 PostgreSQL 失败")?;
    // 启动时迁移在事务和 PostgreSQL advisory lock 下串行，避免两个副本
    // 同时创建表；迁移失败则拒绝对外提供部分可用的服务。
    infrastructure::postgres::migrate(&db)
        .await
        .context("数据库迁移失败")?;
    let auth_store: Arc<dyn AuthStore> = Arc::new(
        infrastructure::auth_postgres::PostgresAuthStore::new(db.clone()),
    );
    let mail: Arc<dyn MailSender> = Arc::new(infrastructure::mail::SmtpMailSender::new(
        &settings.auth.mail,
    )?);
    let oauth: Arc<dyn OAuthGateway> = Arc::new(infrastructure::oauth::RemoteOAuthGateway::new(
        settings.auth.clone(),
    )?);
    let auth = Arc::new(AuthService::new(
        auth_store,
        mail,
        oauth,
        settings.auth.clone(),
    ));
    // 数据库迁移完成后才初始化管理员；重复启动不会覆盖已有账号。
    auth.bootstrap_admin()
        .await
        .context("初始化管理员账号失败")?;
    let repository = Arc::new(infrastructure::postgres::PostgresRepository::new(db));
    let metrics: Arc<dyn MetricRepository> = repository.clone();
    let profiles: Arc<dyn ProfileRepository> = repository.clone();
    let alerts: Arc<dyn AlertRepository> = repository;
    let app = Arc::new(Application::new(
        metrics,
        profiles,
        alerts,
        settings.scoring.persist_every_secs,
    ));
    let state = Arc::new(AppState {
        app,
        auth,
        settings: settings.clone(),
        streams: RwLock::new(HashMap::new()),
    });
    let grpc_addr: std::net::SocketAddr = settings.server.grpc_addr.parse()?;
    let grpc_state = state.clone();
    let grpc_settings = settings.clone();
    // gRPC 与 HTTP 同时服务；HTTP 就绪检查以数据库可访问为条件。
    tokio::spawn(async move {
        if let Err(error) = run_grpc(grpc_state, grpc_addr, grpc_settings).await {
            tracing::error!(%error, "gRPC 服务退出");
        }
    });
    let retention_state = state.clone();
    // 指标与批次键用同一保留期清理，历史查询不能无限占用磁盘。
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let cutoff = chrono::Utc::now().timestamp_millis()
                - retention_state.settings.retention.days * 86_400_000;
            if let Err(error) = retention_state.app.metrics.retention(cutoff).await {
                tracing::warn!(%error, "历史指标清理失败");
            }
        }
    });
    let auth_state = state.clone();
    // 会话、邮箱验证码、OAuth 状态和登录限流记录各有独立过期时间。
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            if let Err(error) = auth_state.auth.cleanup().await {
                tracing::warn!(%error, "清理过期认证状态失败");
            }
        }
    });
    let profile_state = state.clone();
    // perf 命令可能在 Worker 断线时失去结果，过期任务最终必须转为失败。
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            let now = chrono::Utc::now().timestamp_millis();
            profile_state.app.prune_inactive(now).await;
            // perf 最长 60 秒，额外留出传输/符号化时间；异常断线的任务最终会失败。
            match profile_state
                .app
                .profiles
                .expire_stale(now - 120_000, now)
                .await
            {
                Ok(count) if count > 0 => tracing::warn!(count, "已终止超时的性能剖析任务"),
                Err(error) => tracing::warn!(%error, "清理超时采样任务失败"),
                _ => {}
            }
        }
    });
    let http_addr: std::net::SocketAddr = settings.server.http_addr.parse()?;
    let app: Router = interfaces::http::router(state);
    let listener = tokio::net::TcpListener::bind(http_addr).await?;
    info!(%http_addr, %grpc_addr, "中心服务已启动");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn run_grpc(
    state: Arc<AppState>,
    addr: std::net::SocketAddr,
    settings: Settings,
) -> anyhow::Result<()> {
    let service = linux_pilot_wire::agent::agent_transport_server::AgentTransportServer::new(
        interfaces::grpc::AgentService { state },
    );
    let mut builder = Server::builder();
    if let (Some(cert_path), Some(key_path)) =
        (settings.server.tls_cert_path, settings.server.tls_key_path)
    {
        let cert = tokio::fs::read(cert_path).await?;
        let key = tokio::fs::read(key_path).await?;
        builder = builder.tls_config(
            tonic::transport::ServerTlsConfig::new()
                .identity(tonic::transport::Identity::from_pem(cert, key)),
        )?;
        info!("Agent gRPC TLS 已启用");
    } else {
        tracing::warn!("Agent gRPC 未启用 TLS；仅适合本机开发或可信网络");
    }
    builder.add_service(service).serve(addr).await?;
    Ok(())
}
