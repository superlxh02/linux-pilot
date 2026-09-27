//! 应用层端口。领域用语描述所需能力，不暴露 SeaORM、SQL 或 HTTP 类型。

use anyhow::Result;
use async_trait::async_trait;
use linux_pilot_model::{Metric, Scenario, Score};
use linux_pilot_scoring::topology::{PodObservation, TopologyManifest};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
/// 节点列表的只读投影视图；在线状态由最近心跳时间计算，不持久化布尔值。
pub struct HostView {
    pub id: String,
    pub hostname: String,
    pub last_seen_ms: i64,
    pub online: bool,
    pub capabilities: Value,
    pub cpu_pct: Option<f64>,
    pub mem_pct: Option<f64>,
    pub health_score: Option<f64>,
}

/// 历史指标查询条件；`aggregate_only` 限制为无标签主机汇总样本。
///
/// `step_ms` 为数据库端时间桶宽度，不在前端对大量原始点重复聚合。
pub struct MetricRange<'a> {
    pub host_id: &'a str,
    pub category: Option<&'a str>,
    pub from: i64,
    pub to: i64,
    pub limit: u64,
    pub aggregate_only: bool,
    pub step_ms: Option<i64>,
    /// 可选的维度子集匹配，供挂载点、cgroup 与进程实例画历史曲线。
    pub labels: Option<&'a BTreeMap<String, String>>,
}

#[async_trait]
/// 指标仓储端口，定义中心端对时序数据的最小依赖。
///
/// `store_batch` 必须把去重键和指标放进同一个事务；返回 `false`
/// 表示该序号之前已提交。只有提交成功后接口层才能 ACK Worker。
pub trait MetricRepository: Send + Sync {
    async fn ping(&self) -> Result<()>;
    async fn register_host(
        &self,
        id: &str,
        hostname: &str,
        capabilities: Value,
        now: i64,
    ) -> Result<()>;
    async fn touch_host(&self, id: &str, now: i64) -> Result<()>;
    async fn store_batch(
        &self,
        host_id: &str,
        boot_id: &str,
        sequence: u64,
        metrics: &[Metric],
        now: i64,
    ) -> Result<bool>;
    async fn store_score(&self, score: &Score) -> Result<()>;
    async fn list_hosts(&self, now: i64) -> Result<Vec<HostView>>;
    async fn overview(&self, host_id: &str, since: i64) -> Result<BTreeMap<String, f64>>;
    async fn metrics(&self, range: MetricRange<'_>) -> Result<Vec<Metric>>;
    async fn scores(
        &self,
        host_id: &str,
        scenario: Scenario,
        from: i64,
        to: i64,
    ) -> Result<Vec<Score>>;
    async fn retention(&self, cutoff_ms: i64) -> Result<()>;
}

/// 当前进程清单只保留最新快照；PID 与 start_ticks 一起构成进程身份，
/// 防止服务重启或 PID 复用后把监控配置应用到另一个程序。
#[derive(Debug, Clone, Serialize)]
pub struct ProcessEntry {
    pub pid: i32,
    pub start_ticks: i64,
    pub comm: String,
    pub uid: i64,
    pub ppid: i32,
    pub state: String,
    pub command: String,
    pub cpu_pct: f64,
    pub rss_bytes: i64,
    pub last_seen_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessWatch {
    pub host_id: String,
    pub pid: i32,
    pub start_ticks: i64,
    pub name: String,
    pub created_ms: i64,
}

#[async_trait]
pub trait ProcessRepository: Send + Sync {
    async fn list_processes(&self, host_id: &str, since_ms: i64) -> Result<Vec<ProcessEntry>>;
    async fn list_watches(&self, host_id: &str) -> Result<Vec<ProcessWatch>>;
    async fn add_watch(
        &self,
        host_id: &str,
        pid: i32,
        start_ticks: i64,
        now_ms: i64,
    ) -> Result<ProcessWatch>;
    async fn remove_watch(&self, host_id: &str, pid: i32, start_ticks: i64) -> Result<bool>;
}

#[derive(Debug, Clone, Serialize)]
/// 按需 perf 任务的持久状态；详情页可重放历史结果。
pub struct ProfileJob {
    pub id: String,
    pub host_id: String,
    pub pid: i32,
    pub duration_s: i32,
    pub frequency_hz: i32,
    pub status: String,
    pub error: Option<String>,
    pub folded: Option<String>,
    pub sample_count: Option<i64>,
    pub created_ms: i64,
    pub finished_ms: Option<i64>,
}

#[derive(Debug)]
/// Worker 上传 perf 结果时的借用视图，避免复制大块折叠栈文本。
pub struct ProfileCompletion<'a> {
    pub id: &'a str,
    pub host_id: &'a str,
    pub success: bool,
    pub error: &'a str,
    pub folded: &'a str,
    pub sample_count: u64,
    pub finished_ms: i64,
}

#[async_trait]
/// perf 任务仓储端口；超时任务由后台清理变为失败状态。
pub trait ProfileRepository: Send + Sync {
    async fn start(&self, job: &ProfileJob) -> Result<()>;
    async fn get(&self, id: &str) -> Result<Option<ProfileJob>>;
    async fn list_recent(&self, host_id: &str) -> Result<Vec<ProfileJob>>;
    async fn complete(&self, completion: ProfileCompletion<'_>) -> Result<()>;
    async fn expire_stale(&self, cutoff_ms: i64, now_ms: i64) -> Result<u64>;
}

#[derive(Debug, Clone, Serialize)]
/// 阈值告警配置。持续时间用于过滤短促尖峰，启停与事件分开保存。
pub struct AlertRule {
    pub id: String,
    pub host_id: String,
    pub metric: String,
    pub comparison: String,
    pub threshold: f64,
    pub duration_s: i32,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
/// 一次告警生命周期；触发和恢复共用同一事件记录。
pub struct AlertEvent {
    pub id: String,
    pub rule_id: String,
    pub host_id: String,
    pub metric: String,
    pub value: f64,
    pub triggered_ms: i64,
    pub resolved_ms: Option<i64>,
}

#[async_trait]
/// 告警规则与事件端口。数据库唯一索引保证同规则同节点只有一个未恢复事件。
pub trait AlertRepository: Send + Sync {
    async fn create_rule(&self, rule: &AlertRule) -> Result<()>;
    async fn set_enabled(&self, id: &str, enabled: bool, now: i64) -> Result<bool>;
    async fn list_rules(&self) -> Result<Vec<AlertRule>>;
    async fn active_rules(&self, host_id: &str) -> Result<Vec<AlertRule>>;
    async fn list_events(&self) -> Result<Vec<AlertEvent>>;
    async fn trigger(&self, rule: &AlertRule, host_id: &str, value: f64, now: i64) -> Result<()>;
    async fn resolve(&self, rule_id: &str, host_id: &str, now: i64) -> Result<()>;
}

/// 拓扑配置和 Kubernetes 发现事实分开持久化。
/// 配置由用户版本控制；Pod UID 是随调度变化的运行时数据，不回写 Manifest。
#[async_trait]
pub trait TopologyRepository: Send + Sync {
    async fn manifest(&self) -> Result<Option<TopologyManifest>>;
    async fn apply_manifest(&self, manifest: &TopologyManifest) -> Result<()>;
    async fn upsert_pods(&self, observations: &[PodObservation]) -> Result<()>;
    async fn pods(&self, cluster_id: &str, since_ms: i64) -> Result<Vec<PodObservation>>;
    async fn pod_metrics(
        &self,
        host_id: &str,
        pod_uid: &str,
        since_ms: i64,
    ) -> Result<BTreeMap<String, f64>>;
}
