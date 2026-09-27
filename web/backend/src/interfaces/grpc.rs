//! gRPC 适配器只做鉴权、协议转换与 ACK；业务由应用服务负责。
//!
//! 首帧必须是带令牌与主机身份的 Hello。收到批次时先区分永久无效输入
//! 与可重试故障：前者 Reject 并要求 Worker 隔离，后者断流让 Worker 重放。
//! ACK 只能在应用服务完成数据库事务之后发送，这是传输可靠性边界。

use crate::{
    AppState,
    application::{ports::ProfileCompletion, service::Application},
};
use linux_pilot_model::{Metric, MetricBatch};
use linux_pilot_wire::agent::{
    Ack, AgentFrame, Reject, ServerFrame, agent_frame::Body as AgentBody,
    agent_transport_server::AgentTransport, server_frame::Body as ServerBody,
};
use std::{pin::Pin, sync::Arc};
use subtle::ConstantTimeEq;
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt, wrappers::ReceiverStream};
use tonic::{Request, Response, Status, Streaming};

/// tonic 服务实现；每个连接持有独立入站流和有界出站队列。
pub struct AgentService {
    pub state: Arc<AppState>,
}

#[tonic::async_trait]
impl AgentTransport for AgentService {
    type ExchangeStream = Pin<Box<dyn Stream<Item = Result<ServerFrame, Status>> + Send>>;

    async fn exchange(
        &self,
        request: Request<Streaming<AgentFrame>>,
    ) -> Result<Response<Self::ExchangeStream>, Status> {
        // 先完成握手鉴权和主机注册，再把该连接放入可派发 perf 命令的表。
        // 未认证连接不能抢占同一 host_id 的任务路由。
        let mut incoming = request.into_inner();
        let Some(first) = incoming.message().await? else {
            return Err(Status::unauthenticated("缺少 Hello"));
        };
        let Some(AgentBody::Hello(hello)) = first.body else {
            return Err(Status::unauthenticated("首帧必须是 Hello"));
        };
        if !bool::from(
            hello
                .token
                .as_bytes()
                .ct_eq(self.state.settings.worker.token.as_bytes()),
        ) || hello.host_id.is_empty()
            || hello.host_id.len() > 128
        {
            return Err(Status::unauthenticated("Worker 凭证或主机 ID 无效"));
        }
        let capabilities = serde_json::json!({
            "ebpf": hello.ebpf, "perf": hello.perf, "cgroup_v2": hello.cgroup_v2
        });
        self.state
            .app
            .metrics
            .register_host(
                &hello.host_id,
                &hello.hostname,
                capabilities,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(internal)?;

        let (sender, receiver) = mpsc::channel(128);
        self.state
            .streams
            .write()
            .await
            .insert(hello.host_id.clone(), sender.clone());
        let state = self.state.clone();
        let host_id = hello.host_id;
        tokio::spawn(async move {
            while let Ok(Some(frame)) = incoming.message().await {
                match frame.body {
                    Some(AgentBody::Batch(wire_batch)) => {
                        if wire_batch.host_id != host_id {
                            break;
                        }
                        let batch = MetricBatch {
                            host_id: wire_batch.host_id,
                            boot_id: wire_batch.boot_id,
                            sequence: wire_batch.sequence,
                            metrics: wire_batch
                                .metrics
                                .into_iter()
                                .map(|item| Metric {
                                    name: item.name,
                                    value: item.value,
                                    time_ms: item.time_ms,
                                    labels: item.labels.into_iter().collect(),
                                    source: item.source,
                                })
                                .collect(),
                        };
                        let sequence = batch.sequence;
                        // 数据格式永久错误：明确 Reject，避免它无限重试并堵住后续批次。
                        if let Err(error) = Application::validate_batch(
                            &batch,
                            chrono::Utc::now().timestamp_millis(),
                        ) {
                            tracing::warn!(%error, host_id, sequence, "已拒绝永久无效的批次");
                            if sender
                                .send(ServerFrame {
                                    body: Some(ServerBody::Reject(Reject {
                                        sequence,
                                        reason: error.to_string(),
                                    })),
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                            continue;
                        }
                        // 数据库暂时失败：不 ACK，不 Reject，断开后由 Worker 磁盘重放。
                        if let Err(error) = state.app.ingest(batch).await {
                            tracing::warn!(%error, host_id, sequence, "批次处理失败，等待 Worker 重传");
                            break;
                        }
                        // 已提交批次才发送 ACK。若此时连接中断，重复重放由数据库
                        // 的 (host_id, boot_id, sequence) 主键安全去重。
                        if sender
                            .send(ServerFrame {
                                body: Some(ServerBody::Ack(Ack { sequence })),
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(AgentBody::Heartbeat(heartbeat)) if heartbeat.host_id == host_id => {
                        if let Err(error) = state
                            .app
                            .metrics
                            .touch_host(&host_id, chrono::Utc::now().timestamp_millis())
                            .await
                        {
                            tracing::warn!(%error, "更新主机心跳失败");
                        }
                    }
                    Some(AgentBody::ProfileResult(result)) if result.host_id == host_id => {
                        if let Err(error) = state
                            .app
                            .finish_profile(ProfileCompletion {
                                id: &result.job_id,
                                host_id: &result.host_id,
                                success: result.success,
                                error: &result.error,
                                folded: &result.folded_stacks,
                                sample_count: result.sample_count,
                                finished_ms: chrono::Utc::now().timestamp_millis(),
                            })
                            .await
                        {
                            tracing::warn!(%error, "保存性能剖析结果失败");
                        }
                    }
                    _ => break,
                }
            }
            // 旧连接的清理不能误删同一主机刚建立的新连接。
            let mut streams = state.streams.write().await;
            if streams
                .get(&host_id)
                .is_some_and(|current| current.same_channel(&sender))
            {
                streams.remove(&host_id);
            }
            tracing::info!(host_id, "Worker 已断开");
        });
        Ok(Response::new(Box::pin(
            ReceiverStream::new(receiver).map(Ok),
        )))
    }
}

fn internal(error: impl std::fmt::Display) -> Status {
    tracing::error!(%error, "gRPC 接入失败");
    Status::internal("接入服务暂不可用")
}
