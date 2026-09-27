//! 分布式 Linux 采集端入口。
//!
//! 生命周期由两个循环组成：采样循环每秒写入持久化缓冲，连接循环负责
//! gRPC 握手、发送、ACK 与断线重连。两者通过有界通道协作；中心端不可用时
//! 采样仍然继续，缓冲达到上限后的丢弃量会作为 Worker 自身指标上报。

#[cfg(target_os = "linux")]
mod collector;
#[cfg(target_os = "linux")]
mod ebpf;
#[cfg(target_os = "linux")]
mod perf;
#[cfg(target_os = "linux")]
mod spool;

#[cfg(target_os = "linux")]
use anyhow::Context;
#[cfg(target_os = "linux")]
use config::{Config, Environment};
#[cfg(target_os = "linux")]
use linux_pilot_model::{Metric, MetricBatch};
#[cfg(target_os = "linux")]
use linux_pilot_wire::agent::{
    self, AgentFrame, Hello, agent_frame::Body as AgentBody,
    agent_transport_client::AgentTransportClient, server_frame::Body as ServerBody,
};
#[cfg(target_os = "linux")]
use serde::Deserialize;
#[cfg(target_os = "linux")]
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};
#[cfg(target_os = "linux")]
use std::{path::PathBuf, time::Duration};
#[cfg(target_os = "linux")]
use tokio::sync::{Mutex, mpsc, watch};
#[cfg(target_os = "linux")]
use tokio_stream::wrappers::ReceiverStream;
#[cfg(target_os = "linux")]
use tracing::{info, warn};

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Deserialize)]
struct Settings {
    /// 中心端 gRPC 地址；跨主机部署应使用可达域名和 TLS。
    server_url: String,
    /// 只用于 Worker 握手，必须与 Web 后端 `worker.token` 相同。
    token: String,
    /// 可选的固定主机身份；未指定时从持久缓冲目录恢复。
    host_id: Option<String>,
    /// 未 ACK 批次和主机 ID 的持久目录，不能放在容器临时层。
    spool_dir: PathBuf,
    /// 自建 CA 证书路径；配置后由 tonic 校验中心端证书。
    ca_cert_path: Option<PathBuf>,
    /// 带标签明细上报频率；整机汇总仍每秒上报。
    detail_every_secs: u64,
}

#[cfg(target_os = "linux")]
impl Settings {
    /// 从稳定的 `PO_AGENT__*` 环境变量读取部署配置。
    ///
    /// 此前缀属于 Worker 配置接口，品牌更名后保留以兼容现有主机部署。
    fn load() -> anyhow::Result<Self> {
        Config::builder()
            .set_default("server_url", "http://127.0.0.1:50051")?
            .set_default("token", "change-me-agent")?
            .set_default("spool_dir", "/var/lib/linux-pilot-worker/spool")?
            .set_default("detail_every_secs", 5)?
            .add_source(Environment::with_prefix("PO_AGENT").separator("__"))
            .build()?
            .try_deserialize()
            .map_err(Into::into)
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("linux-pilot-worker 仅支持 Linux；请在目标 Linux 主机或 Docker Linux 容器中运行");
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "linux_pilot_worker=info".into()),
        )
        .json()
        .init();
    let settings = Settings::load()?;
    anyhow::ensure!(
        (1..=60).contains(&settings.detail_every_secs),
        "明细采样间隔必须在 1～60 秒之间"
    );
    let host_id = stable_host_id(&settings)?;
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .context("读取 Linux boot_id 失败")?
        .trim()
        .to_owned();
    let hostname = std::fs::read_to_string("/etc/hostname")
        .unwrap_or_else(|_| host_id.clone())
        .trim()
        .to_owned();
    let spool = Arc::new(Mutex::new(spool::Spool::new(settings.spool_dir.clone())?));
    let collector = collector::Collector::new();
    let ebpf_available = collector.ebpf_available();
    let (live_sender, live_receiver) = watch::channel(None);
    // 采集与网络重连分离：中心端中断时仍每秒采样并写入本地缓冲。
    // 采样任务与连接任务独立：网络失败不应让下一秒的观测窗口消失。
    tokio::spawn(sample_loop(
        host_id.clone(),
        boot_id.clone(),
        spool.clone(),
        collector,
        live_receiver,
        settings.detail_every_secs,
    ));
    info!(%host_id, "Agent 启动");

    loop {
        match connect_once(
            &settings,
            &host_id,
            &boot_id,
            &hostname,
            &spool,
            &live_sender,
            ebpf_available,
        )
        .await
        {
            Ok(()) => warn!("gRPC 流已关闭，准备重连"),
            Err(error) => warn!(%error, "连接中心端失败，准备重连"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[cfg(target_os = "linux")]
/// 确定稳定主机 ID，以免容器重新创建后被服务端识别成一台新主机。
///
/// 优先级：显式配置、缓冲目录已有 ID、首次运行的 `/etc/machine-id`。
/// 后两者都不可用时生成 UUID，并立即写入持久目录。
fn stable_host_id(settings: &Settings) -> anyhow::Result<String> {
    if let Some(id) = settings.host_id.as_ref().filter(|id| !id.trim().is_empty()) {
        return Ok(id.clone());
    }
    std::fs::create_dir_all(&settings.spool_dir)?;
    let path = settings.spool_dir.join("host-id");
    if let Ok(saved) = std::fs::read_to_string(&path) {
        if !saved.trim().is_empty() {
            return Ok(saved.trim().to_owned());
        }
    }
    // 容器重新创建时 /etc/machine-id 可能变化，首次确定后持久化到数据卷。
    let id = std::fs::read_to_string("/etc/machine-id")
        .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string())
        .trim()
        .to_owned();
    std::fs::write(path, &id)?;
    Ok(id)
}

#[cfg(target_os = "linux")]
/// 每秒采集一次并先写磁盘，再尝试送入实时通道。
///
/// 通道写入用 `try_send`：网络背压时不能阻塞采样时钟。未能实时发送的
/// 批次仍在磁盘里，连接循环按序补发。明细降频只影响发送内容，采集器的
/// 计数器基线每秒仍会更新，所以速率分母始终是本轮的实际间隔。
async fn sample_loop(
    host_id: String,
    boot_id: String,
    spool: Arc<Mutex<spool::Spool>>,
    mut collector: collector::Collector,
    live: watch::Receiver<Option<mpsc::Sender<MetricBatch>>>,
    detail_every_secs: u64,
) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    let mut tick = 0_u64;
    loop {
        ticker.tick().await;
        let mut metrics = collector.sample().await;
        // 主机汇总每秒保留；设备、网卡、CPU 核、进程和 cgroup 明细降频写库。
        // 采集器仍每秒读取计数器，速率计算不会因降频而使用过期基线。
        if tick % detail_every_secs != 0 {
            metrics.retain(|metric| metric.labels.is_empty());
        }
        tick = tick.wrapping_add(1);
        let now = chrono::Utc::now().timestamp_millis();
        let mut local = spool.lock().await;
        if let Ok((pending, bytes, dropped)) = local.stats() {
            for (name, value) in [
                ("agent.buffer_batches", pending as f64),
                ("agent.buffer_bytes", bytes as f64),
                ("agent.dropped_batches_total", dropped as f64),
            ] {
                metrics.push(Metric {
                    name: name.to_owned(),
                    value,
                    time_ms: now,
                    labels: Default::default(),
                    source: "agent".to_owned(),
                });
            }
        }
        let batch = MetricBatch {
            host_id: host_id.clone(),
            boot_id: boot_id.clone(),
            sequence: local.next_sequence(),
            metrics,
        };
        let stored = local.store(&batch);
        drop(local);
        if let Err(error) = stored {
            warn!(%error, "本地缓冲写入失败，当前批次不发送");
            continue;
        }
        // 实时队列满时保留磁盘批次；连接任务每 10 秒补发未确认数据。
        if let Some(sender) = live.borrow().as_ref() {
            let _ = sender.try_send(batch);
        }
    }
}

#[cfg(target_os = "linux")]
/// 建立一次 gRPC 双向流并处理 ACK、拒绝、perf 命令和心跳式重放。
///
/// 函数退出意味着本次连接失效，外层循环会在短暂退避后重建连接。
/// 只有服务器 ACK 的批次才从磁盘删除；Reject 则转入隔离目录。
async fn connect_once(
    settings: &Settings,
    host_id: &str,
    boot_id: &str,
    hostname: &str,
    spool: &Arc<Mutex<spool::Spool>>,
    live: &watch::Sender<Option<mpsc::Sender<MetricBatch>>>,
    ebpf_available: bool,
) -> anyhow::Result<()> {
    let mut endpoint = tonic::transport::Endpoint::from_shared(settings.server_url.clone())?
        .connect_timeout(Duration::from_secs(10))
        .tcp_keepalive(Some(Duration::from_secs(30)));
    if let Some(ca_path) = &settings.ca_cert_path {
        let certificate = std::fs::read(ca_path)?;
        endpoint = endpoint.tls_config(
            tonic::transport::ClientTlsConfig::new()
                .ca_certificate(tonic::transport::Certificate::from_pem(certificate)),
        )?;
    }
    let mut client = AgentTransportClient::new(endpoint.connect().await?);
    let (sender, receiver) = mpsc::channel::<AgentFrame>(256);
    // 首帧必须在 RPC 握手前入队，否则服务端等待 Hello、客户端等待响应会相互阻塞。
    sender
        .send(AgentFrame {
            body: Some(AgentBody::Hello(Hello {
                host_id: host_id.to_owned(),
                boot_id: boot_id.to_owned(),
                token: settings.token.clone(),
                hostname: hostname.to_owned(),
                ebpf: ebpf_available,
                perf: perf::available(),
                cgroup_v2: std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists(),
            })),
        })
        .await?;
    let response = client.exchange(ReceiverStream::new(receiver)).await?;
    let mut incoming = response.into_inner();
    let (live_sender, mut live_receiver) = mpsc::channel::<MetricBatch>(256);
    let mut sent = HashSet::new();
    let mut replay = VecDeque::from(spool.lock().await.pending_paths()?);
    fill_window(&mut replay, spool, host_id, &sender, &mut sent).await?;
    live.send_replace(Some(live_sender));
    let mut replay_ticker = tokio::time::interval(Duration::from_secs(10));
    replay_ticker.tick().await;
    let mut last_response = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = replay_ticker.tick() => {
                if !sent.is_empty() && last_response.elapsed() > Duration::from_secs(60) {
                    anyhow::bail!("超过 60 秒未收到 Worker 批次确认，重新建立连接");
                }
                if replay.is_empty() { replay = VecDeque::from(spool.lock().await.pending_paths()?); }
                fill_window(&mut replay, spool, host_id, &sender, &mut sent).await?;
            }
            Some(batch) = live_receiver.recv() => {
                if sent.len() < 64 && sent.insert(batch.sequence) {
                    sender.send(batch_frame(batch)).await?;
                }
            }
            incoming_message = incoming.message() => {
                let Some(frame) = incoming_message? else { break };
                last_response = tokio::time::Instant::now();
                match frame.body {
                    Some(ServerBody::Ack(ack)) => {
                        spool.lock().await.ack(ack.sequence)?;
                        sent.remove(&ack.sequence);
                        fill_window(&mut replay, spool, host_id, &sender, &mut sent).await?;
                    }
                    Some(ServerBody::Reject(reject)) => {
                        warn!(sequence = reject.sequence, reason = %reject.reason, "服务端拒绝批次，已隔离");
                        spool.lock().await.reject(reject.sequence)?;
                        sent.remove(&reject.sequence);
                        fill_window(&mut replay, spool, host_id, &sender, &mut sent).await?;
                    }
                    Some(ServerBody::ProfileCommand(command)) => {
                        let sender = sender.clone();
                        let host_id = host_id.to_owned();
                        tokio::spawn(async move {
                            let result = perf::run(command, host_id).await;
                            let _ = sender.send(AgentFrame {
                                body: Some(AgentBody::ProfileResult(result))
                            }).await;
                        });
                    }
                    None => {}
                }
            }
        }
    }
    live.send_replace(None);
    Ok(())
}

#[cfg(target_os = "linux")]
/// 从磁盘补满最多 64 个在途批次，保留接收 ACK 的空间。
///
/// 若无限制地向双向流写入历史批次，双方可能都等待对端读取而停滞。
/// `sent` 同时防止实时队列与历史重放重复发送同一序号。
async fn fill_window(
    replay: &mut VecDeque<PathBuf>,
    spool: &Arc<Mutex<spool::Spool>>,
    host_id: &str,
    sender: &mpsc::Sender<AgentFrame>,
    sent: &mut HashSet<u64>,
) -> anyhow::Result<()> {
    // 最多 64 个在途批次；客户端必须有机会持续读取 ACK，避免双向流双方互相等待。
    while sent.len() < 64 {
        let Some(path) = replay.pop_front() else {
            break;
        };
        let Some(batch) = spool.lock().await.read_batch(&path)? else {
            continue;
        };
        if batch.host_id != host_id {
            tracing::warn!(
                sequence = batch.sequence,
                "缓冲批次主机 ID 与当前配置不符，已隔离"
            );
            spool.lock().await.reject(batch.sequence)?;
            continue;
        }
        if sent.insert(batch.sequence) {
            sender.send(batch_frame(batch)).await?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
/// 将领域批次显式映射到 v1 Protobuf，避免协议对象侵入采集器与缓冲。
fn batch_frame(batch: MetricBatch) -> AgentFrame {
    AgentFrame {
        body: Some(AgentBody::Batch(agent::MetricBatch {
            host_id: batch.host_id,
            boot_id: batch.boot_id,
            sequence: batch.sequence,
            metrics: batch
                .metrics
                .into_iter()
                .map(|metric: Metric| agent::Metric {
                    name: metric.name,
                    value: metric.value,
                    time_ms: metric.time_ms,
                    labels: metric.labels.into_iter().collect(),
                    source: metric.source,
                })
                .collect(),
        })),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replay_window_stays_bounded_while_acks_arrive() {
        let (sender, mut receiver) = mpsc::channel(256);
        let dir = std::env::temp_dir().join(format!("po-replay-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for sequence in 0..130 {
            let batch = MetricBatch {
                host_id: "h".into(),
                boot_id: "b".into(),
                sequence,
                metrics: vec![],
            };
            std::fs::write(
                dir.join(format!("{sequence:020}.json")),
                serde_json::to_vec(&batch).unwrap(),
            )
            .unwrap();
        }
        let spool = Arc::new(Mutex::new(spool::Spool::new(dir.clone()).unwrap()));
        let mut replay = VecDeque::from(spool.lock().await.pending_paths().unwrap());
        let mut sent = HashSet::new();
        fill_window(&mut replay, &spool, "h", &sender, &mut sent)
            .await
            .unwrap();
        assert_eq!(sent.len(), 64);
        assert_eq!(replay.len(), 66);
        let first = receiver.recv().await.unwrap();
        let sequence = match first.body.unwrap() {
            AgentBody::Batch(batch) => batch.sequence,
            _ => panic!("首帧应为批次"),
        };
        sent.remove(&sequence);
        fill_window(&mut replay, &spool, "h", &sender, &mut sent)
            .await
            .unwrap();
        assert_eq!(sent.len(), 64);
        assert_eq!(replay.len(), 65);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
