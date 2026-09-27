//! 应用服务：负责一次采样的可靠提交、窗口评分和告警状态转换。
//!
//! 这里仅依赖领域模型和仓储端口。gRPC、HTTP 与 PostgreSQL 均在外层适配。

use super::ports::{
    AlertRepository, AlertRule, MetricRepository, ProcessRepository, ProfileCompletion, ProfileJob,
    ProfileRepository,
};
use anyhow::{Result, ensure};
use linux_pilot_model::{Metric, MetricBatch, Scenario};
use linux_pilot_scoring::{RuleBasedScoreEngine, ScoreEngine};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{RwLock, broadcast};

/// 连接仓储端口、评分规则和短窗口状态的应用服务。
///
/// 数据库保存事实数据；`recent`、`last_score_bucket` 与 `alert_pending`
/// 仅加速实时派生和告警持续时间计算，进程重启后会自然重建。
pub struct Application {
    pub metrics: Arc<dyn MetricRepository>,
    pub profiles: Arc<dyn ProfileRepository>,
    pub processes: Arc<dyn ProcessRepository>,
    pub alerts: Arc<dyn AlertRepository>,
    pub events: broadcast::Sender<String>,
    scorer: RuleBasedScoreEngine,
    recent: RwLock<HashMap<String, Vec<Metric>>>,
    last_score_bucket: RwLock<HashMap<String, i64>>,
    alert_pending: RwLock<HashMap<(String, String), i64>>,
    persist_interval_ms: i64,
}

impl Application {
    /// 注入仓储端口，创建有界广播通道并固定首版规则评分器。
    ///
    /// 将来新增 AI 分析服务时应通过独立端口读取已保存的评分证据，
    /// 不能在这个构造函数中让模型悄悄覆盖规则分。
    pub fn new(
        metrics: Arc<dyn MetricRepository>,
        profiles: Arc<dyn ProfileRepository>,
        processes: Arc<dyn ProcessRepository>,
        alerts: Arc<dyn AlertRepository>,
        persist_interval_secs: i64,
    ) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            metrics,
            profiles,
            processes,
            alerts,
            events,
            scorer: RuleBasedScoreEngine::default(),
            recent: RwLock::new(HashMap::new()),
            last_score_bucket: RwLock::new(HashMap::new()),
            alert_pending: RwLock::new(HashMap::new()),
            persist_interval_ms: persist_interval_secs * 1000,
        }
    }

    /// 处理一次 Worker 批次，并在事实数据落库后派生评分与告警。
    ///
    /// 返回 `Ok` 才允许 gRPC 层 ACK。重复的 `(host_id, boot_id, sequence)`
    /// 只更新主机在线时间，不再次广播、评分或触发告警。评分与告警属于
    /// 派生结果，失败会记录日志，但不能让已提交批次一直重放。
    pub async fn ingest(&self, batch: MetricBatch) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        Self::validate_batch(&batch, now)?;
        // 仓储先提交批次和去重键，再由 gRPC 适配器发送 ACK。
        let inserted = self
            .metrics
            .store_batch(
                &batch.host_id,
                &batch.boot_id,
                batch.sequence,
                &batch.metrics,
                now,
            )
            .await?;
        self.metrics.touch_host(&batch.host_id, now).await?;
        if !inserted {
            return Ok(());
        }

        // 只保留主机聚合值用于评分，避免进程/设备标签占用不受控的内存。
        let window = {
            let mut recent = self.recent.write().await;
            let history = recent.entry(batch.host_id.clone()).or_default();
            history.extend(
                batch
                    .metrics
                    .iter()
                    .filter(|metric| metric.labels.is_empty())
                    .cloned(),
            );
            history.retain(|metric| metric.time_ms >= now - 60_000);
            history.clone()
        };
        // 当前态清单可通过专用 HTTP 端点分页读取，不向每个 WebSocket
        // 客户端广播最多 1024 条进程记录，避免一台节点拖慢所有浏览器。
        let realtime: Vec<&Metric> = batch
            .metrics
            .iter()
            .filter(|metric| metric.name != "proc.present")
            .collect();
        let _ = self.events.send(
            serde_json::json!({
                "type": "metrics", "host_id": batch.host_id, "metrics": realtime
            })
            .to_string(),
        );

        let bucket = now / self.persist_interval_ms;
        let persist = {
            let mut last = self.last_score_bucket.write().await;
            if last.get(&batch.host_id).copied() == Some(bucket) {
                false
            } else {
                last.insert(batch.host_id.clone(), bucket);
                true
            }
        };
        let summarized = summarize_window(&window, now);
        let mut alert_values: HashMap<String, f64> = summarized
            .iter()
            .map(|m| (m.name.clone(), m.value))
            .collect();
        for scenario in Scenario::all() {
            let score = self
                .scorer
                .calculate(&batch.host_id, now, scenario, &summarized);
            if let Some(value) = score.value {
                alert_values.insert(format!("score.{}", scenario.as_str()), value);
            }
            if persist {
                // 派生结果失败不能使已经持久化的批次失去 ACK；失败会单独记录。
                if let Err(error) = self.metrics.store_score(&score).await {
                    tracing::warn!(%error, "评分持久化失败");
                }
            }
            let _ = self
                .events
                .send(serde_json::json!({"type":"score","score":score}).to_string());
        }
        if persist
            && let Err(error) = self
                .evaluate_alerts(&batch.host_id, &alert_values, now)
                .await
        {
            tracing::warn!(%error, "告警处理失败");
        }
        Ok(())
    }

    /// 校验会永久无效的输入并拒绝批次。
    ///
    /// 与数据库超时等暂时故障不同，时间戳越界、非有限浮点数或超限标签
    /// 重试也不会变好；gRPC 层会回 Reject，让 Worker 隔离问题批次。
    pub fn validate_batch(batch: &MetricBatch, now: i64) -> Result<()> {
        ensure!(
            !batch.host_id.is_empty() && !batch.boot_id.is_empty(),
            "缺少批次身份"
        );
        ensure!(batch.metrics.len() <= 20_000, "批次过大");
        for metric in &batch.metrics {
            ensure!(metric.value.is_finite(), "指标值不是有限数");
            ensure!(
                !metric.name.is_empty() && metric.name.len() <= 128,
                "指标名无效"
            );
            ensure!(
                metric.labels.len() <= 8
                    && metric
                        .labels
                        .iter()
                        .all(|(key, value)| key.len() <= 64 && value.len() <= 128),
                "指标标签超限"
            );
            ensure!(
                metric.time_ms <= now + 300_000 && metric.time_ms >= now - 7 * 86_400_000,
                "指标时间戳超限"
            );
            if metric.name == "proc.present" {
                // 清单字段最终进入带整数列的当前态表；提前拒绝永久无效
                // 的标签，避免数据库 CAST 失败后 Worker 无限重传该批次。
                let required = [
                    "pid",
                    "start_ticks",
                    "comm",
                    "uid",
                    "ppid",
                    "state",
                    "command",
                    "rss_bytes",
                ];
                ensure!(
                    required.iter().all(|key| metric.labels.contains_key(*key)),
                    "进程清单标签缺失"
                );
                ensure!(
                    metric.labels["pid"].parse::<i32>().is_ok_and(|pid| pid > 0)
                        && metric.labels["start_ticks"]
                            .parse::<i64>()
                            .is_ok_and(|ticks| ticks >= 0)
                        && metric.labels["uid"]
                            .parse::<i64>()
                            .is_ok_and(|uid| uid >= 0)
                        && metric.labels["ppid"].parse::<i32>().is_ok()
                        && metric.labels["rss_bytes"]
                            .parse::<i64>()
                            .is_ok_and(|bytes| bytes >= 0),
                    "进程清单数值无效"
                );
            }
        }
        Ok(())
    }

    /// 淘汰离线节点的短窗口缓存，避免长时间运行后驻留内存随历史节点数增长。
    pub async fn prune_inactive(&self, now: i64) {
        let active = {
            let mut recent = self.recent.write().await;
            recent.retain(|_, samples| {
                samples.retain(|sample| sample.time_ms >= now - 60_000);
                !samples.is_empty()
            });
            recent
                .keys()
                .cloned()
                .collect::<std::collections::HashSet<_>>()
        };
        self.last_score_bucket
            .write()
            .await
            .retain(|host, _| active.contains(host));
        self.alert_pending
            .write()
            .await
            .retain(|(_, host), _| active.contains(host));
    }

    pub async fn start_profile(
        &self,
        host_id: String,
        pid: i32,
        duration_s: u32,
        frequency_hz: u32,
    ) -> Result<ProfileJob> {
        ensure!(!host_id.is_empty() && pid > 0, "主机和 PID 必须有效");
        ensure!(
            (1..=60).contains(&duration_s) && (1..=199).contains(&frequency_hz),
            "perf 参数超出允许范围"
        );
        let job = ProfileJob {
            id: uuid::Uuid::new_v4().to_string(),
            host_id,
            pid,
            duration_s: duration_s as i32,
            frequency_hz: frequency_hz as i32,
            status: "running".into(),
            error: None,
            folded: None,
            sample_count: None,
            created_ms: chrono::Utc::now().timestamp_millis(),
            finished_ms: None,
        };
        self.profiles.start(&job).await?;
        Ok(job)
    }

    pub async fn finish_profile(&self, result: ProfileCompletion<'_>) -> Result<()> {
        let job_id = result.id.to_owned();
        let status = if result.success {
            "completed"
        } else {
            "failed"
        };
        self.profiles.complete(result).await?;
        let _ = self.events.send(
            serde_json::json!({"type":"profile","job_id":job_id,"status":status}).to_string(),
        );
        Ok(())
    }

    pub async fn create_alert(
        &self,
        host_id: String,
        metric: String,
        comparison: String,
        threshold: f64,
        duration_s: i32,
    ) -> Result<AlertRule> {
        ensure!(
            !host_id.is_empty() && !metric.is_empty() && metric.len() <= 128,
            "告警目标无效"
        );
        ensure!(
            ["above", "below"].contains(&comparison.as_str()) && threshold.is_finite(),
            "告警阈值无效"
        );
        ensure!((0..=3600).contains(&duration_s), "告警持续时间无效");
        let rule = AlertRule {
            id: uuid::Uuid::new_v4().to_string(),
            host_id,
            metric,
            comparison,
            threshold,
            duration_s,
            enabled: true,
        };
        self.alerts.create_rule(&rule).await?;
        Ok(rule)
    }

    async fn evaluate_alerts(
        &self,
        host_id: &str,
        values: &HashMap<String, f64>,
        now: i64,
    ) -> Result<()> {
        for rule in self.alerts.active_rules(host_id).await? {
            let Some(&value) = values.get(&rule.metric) else {
                continue;
            };
            let breached = match rule.comparison.as_str() {
                "above" => value > rule.threshold,
                "below" => value < rule.threshold,
                _ => continue,
            };
            let key = (rule.id.clone(), host_id.to_owned());
            let mut pending = self.alert_pending.write().await;
            if breached {
                let first = *pending.entry(key).or_insert(now);
                drop(pending);
                if now - first >= i64::from(rule.duration_s) * 1000 {
                    self.alerts.trigger(&rule, host_id, value, now).await?;
                }
            } else {
                pending.remove(&key);
                drop(pending);
                self.alerts.resolve(&rule.id, host_id, now).await?;
            }
        }
        Ok(())
    }
}

fn summarize_window(window: &[Metric], now: i64) -> Vec<Metric> {
    let mut grouped: HashMap<&str, Vec<f64>> = HashMap::new();
    for metric in window {
        grouped.entry(&metric.name).or_default().push(metric.value);
    }
    grouped
        .into_iter()
        .map(|(name, mut values)| {
            let value = if name.ends_with("_p95_ms") {
                // 仅有秒级 p95 时，窗口分位数采用保守近似；后续保存原始直方图可精确合并。
                values.sort_by(f64::total_cmp);
                values[((values.len() - 1) as f64 * 0.95).round() as usize]
            } else {
                values.iter().sum::<f64>() / values.len() as f64
            };
            Metric {
                name: name.into(),
                value,
                time_ms: now,
                labels: Default::default(),
                source: "window".into(),
            }
        })
        .collect()
}
