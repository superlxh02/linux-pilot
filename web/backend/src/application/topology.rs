//! 拓扑用例：校验版本化配置、接收 Pod 发现事实并计算跨节点资源视图。
//!
//! 每次查询只用最近窗口数据；缺失应用 SLI 时整体业务分保持 null。
//! 节点分和服务基础设施分仍可解释地返回，避免伪造业务健康度。

use super::ports::{MetricRepository, TopologyRepository};
use anyhow::{Result, ensure};
use linux_pilot_model::Metric;
use linux_pilot_scoring::{
    RuleBasedScoreEngine,
    topology::{PodObservation, ServiceSelector, TopologyManifest, infrastructure_score},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

pub struct TopologyApplication {
    repository: Arc<dyn TopologyRepository>,
    metrics: Arc<dyn MetricRepository>,
    scorer: RuleBasedScoreEngine,
}

impl TopologyApplication {
    pub fn new(
        repository: Arc<dyn TopologyRepository>,
        metrics: Arc<dyn MetricRepository>,
    ) -> Self {
        Self {
            repository,
            metrics,
            scorer: RuleBasedScoreEngine::default(),
        }
    }

    pub async fn manifest(&self) -> Result<Option<TopologyManifest>> {
        self.repository.manifest().await
    }

    pub async fn apply(&self, manifest: &TopologyManifest) -> Result<()> {
        manifest.validate().map_err(anyhow::Error::msg)?;
        self.repository.apply_manifest(manifest).await
    }

    pub async fn observe(&self, pods: &[PodObservation]) -> Result<()> {
        ensure!(pods.len() <= 2000, "Pod 快照最多 2000 项");
        let now = chrono::Utc::now().timestamp_millis();
        for pod in pods {
            ensure!(uuid::Uuid::parse_str(&pod.pod_uid).is_ok(), "Pod UID 无效");
            ensure!(
                pod.observed_ms >= now - 300_000 && pod.observed_ms <= now + 30_000,
                "Pod 发现时间越界"
            );
            ensure!(
                [
                    &pod.cluster_id,
                    &pod.pod_name,
                    &pod.node_id,
                    &pod.namespace,
                    &pod.workload_kind,
                    &pod.workload_name
                ]
                .iter()
                .all(|part| !part.is_empty() && part.len() <= 128),
                "Pod 发现字段无效"
            );
        }
        self.repository.upsert_pods(pods).await
    }

    pub async fn snapshot(&self) -> Result<Value> {
        let Some(manifest) = self.repository.manifest().await? else {
            return Ok(
                json!({"configured":false,"nodes":[],"services":[],"overall_score":null,"infrastructure_score":null}),
            );
        };
        let now = chrono::Utc::now().timestamp_millis();
        let mut nodes = Vec::new();
        for node in &manifest.spec.nodes {
            let profile = manifest
                .spec
                .score_profiles
                .iter()
                .find(|profile| profile.id == node.score_profile_ref);
            let Some(profile) = profile else { continue };
            let overview = self.metrics.overview(&node.id, now - 30_000).await?;
            let measurements: Vec<Metric> = overview
                .iter()
                .map(|(name, value)| Metric {
                    name: name.clone(),
                    value: *value,
                    time_ms: now,
                    labels: BTreeMap::new(),
                    source: "snapshot".into(),
                })
                .collect();
            let score = self.scorer.calculate_with_weights(
                &node.id,
                now,
                profile.scenario,
                &measurements,
                &profile.weights,
            );
            nodes.push(json!({"id":node.id,"cluster_id":node.cluster_id,"role":node.role,"description":node.description,"capacity":node.capacity,"score":score.value,"coverage":score.coverage,"scenario":profile.scenario,"profile":profile.id,"factors":score.factors,"missing":score.missing}));
        }

        let mut discovered = HashMap::new();
        for cluster in &manifest.spec.clusters {
            discovered.insert(
                cluster.id.clone(),
                self.repository.pods(&cluster.id, now - 90_000).await?,
            );
        }
        let mut services = Vec::new();
        let mut service_scores = Vec::new();
        for service in &manifest.spec.services {
            let matched: Vec<&PodObservation> = discovered
                .get(&service.cluster_id)
                .into_iter()
                .flatten()
                .filter(|pod| matches_pod(&service.selector, pod))
                .collect();
            let mut pods = Vec::new();
            let mut score_total = 0.0;
            let mut scored = 0;
            let mut cpu_cores = 0.0;
            let mut memory_bytes = 0.0;
            let mut read_bytes_per_s = 0.0;
            let mut write_bytes_per_s = 0.0;
            let mut network_rx_bytes_per_s = 0.0;
            let mut network_tx_bytes_per_s = 0.0;
            let mut network_samples = 0;
            for pod in matched {
                let values = self
                    .repository
                    .pod_metrics(&pod.node_id, &pod.pod_uid, now - 30_000)
                    .await?;
                let pod_score = pod_resource_score(&values, pod.ready);
                if let Some(value) = pod_score {
                    score_total += value;
                    scored += 1;
                }
                cpu_cores += values.get("cgroup.cpu_usage_pct").copied().unwrap_or(0.0) / 100.0;
                memory_bytes += values
                    .get("cgroup.mem_current_bytes")
                    .copied()
                    .unwrap_or(0.0);
                read_bytes_per_s += values
                    .get("cgroup.io_read_bytes_per_s")
                    .copied()
                    .unwrap_or(0.0);
                write_bytes_per_s += values
                    .get("cgroup.io_write_bytes_per_s")
                    .copied()
                    .unwrap_or(0.0);
                if let (Some(rx), Some(tx)) = (
                    values.get("pod.net_rx_bytes_per_s"),
                    values.get("pod.net_tx_bytes_per_s"),
                ) {
                    network_rx_bytes_per_s += rx;
                    network_tx_bytes_per_s += tx;
                    network_samples += 1;
                }
                pods.push(json!({"uid":pod.pod_uid,"name":pod.pod_name,"node_id":pod.node_id,"ready":pod.ready,"resource_score":pod_score,"metrics":values}));
            }
            // 部分 Pod 缺测时，平均已覆盖副本会高估服务健康度；保留原始
            // 资源汇总供排障，但服务分必须等所有已发现副本都可评分。
            let resource_score = if scored > 0 && scored == pods.len() {
                Some(score_total / scored as f64)
            } else {
                None
            };
            if let Some(value) = resource_score {
                service_scores.push((value, service.criticality.as_str()));
            }
            services.push(json!({"id":service.id,"cluster_id":service.cluster_id,"criticality":service.criticality,"description":service.description,"pod_count":pods.len(),"covered_pods":scored,"resource_score":resource_score,"cpu_cores":cpu_cores,"memory_bytes":memory_bytes,"read_bytes_per_s":read_bytes_per_s,"write_bytes_per_s":write_bytes_per_s,"network_rx_bytes_per_s":if network_samples>0 {Some(network_rx_bytes_per_s)} else {None},"network_tx_bytes_per_s":if network_samples>0 {Some(network_tx_bytes_per_s)} else {None},"network_covered_pods":network_samples,"pods":pods,"sli_status":"not_configured"}));
        }
        // 关键依赖完全失联时不能用其他健康服务的平均值给出高分。
        let critical_expected = manifest
            .spec
            .services
            .iter()
            .filter(|service| service.criticality == "critical")
            .count();
        let critical_covered = service_scores
            .iter()
            .filter(|(_, level)| *level == "critical")
            .count();
        let infra = if critical_covered == critical_expected {
            infrastructure_score(
                &service_scores,
                manifest.spec.system_score.critical_service_max_delta,
            )
        } else {
            None
        };
        Ok(
            json!({"configured":true,"manifest_id":manifest.metadata.id,"revision":manifest.metadata.revision,"time_ms":now,"nodes":nodes,"services":services,"infrastructure_score":infra,"overall_score":null,"overall_status":"waiting_for_end_to_end_sli","service_coverage":if manifest.spec.services.is_empty(){0.0}else{service_scores.len() as f64 / manifest.spec.services.len() as f64}}),
        )
    }
}

fn matches_pod(selector: &ServiceSelector, pod: &PodObservation) -> bool {
    match selector {
        ServiceSelector::KubernetesWorkload {
            namespace,
            kind,
            name,
        } => {
            pod.namespace == *namespace && pod.workload_kind == *kind && pod.workload_name == *name
        }
        ServiceSelector::SystemdUnit { .. } => false,
    }
}

/// 只对实际存在的资源信号计算分数；缺少全部信号时返回 None。
/// PSI 和 CPU 限流衡量争用，内存接近限制衡量爆容风险。分数是资源态分，
/// 与请求延迟、错误率、成功率等业务 SLI 明确分开。
fn pod_resource_score(values: &BTreeMap<String, f64>, ready: bool) -> Option<f64> {
    let specs = [
        ("cgroup.cpu_throttled_period_ratio_pct", 5.0, 50.0, 0.35),
        ("cgroup.psi.cpu.some.avg10", 2.0, 25.0, 0.20),
        ("cgroup.psi.memory.some.avg10", 1.0, 20.0, 0.20),
        ("cgroup.psi.io.some.avg10", 2.0, 30.0, 0.10),
    ];
    let mut penalty = 0.0;
    let mut coverage = 0.0;
    for (name, good, bad, weight) in specs {
        // 空闲 Pod 的 cpu.stat 周期数可能不增长，因而不存在限流周期比。
        // 此时若内核明确上报限流次数速率为 0，可以安全视为本窗口无
        // 限流；若次数非 0 仍不猜测缺失的分母。
        let fallback;
        let measured = if name == "cgroup.cpu_throttled_period_ratio_pct"
            && !values.contains_key(name)
            && values.get("cgroup.cpu_throttled_per_s") == Some(&0.0)
        {
            fallback = 0.0;
            Some(&fallback)
        } else {
            values.get(name)
        };
        if let Some(value) = measured.filter(|value| value.is_finite()) {
            penalty += ((value - good) / (bad - good)).clamp(0.0, 1.0) * weight;
            coverage += weight;
        }
    }
    if let (Some(current), Some(limit)) = (
        values.get("cgroup.mem_current_bytes"),
        values.get("cgroup.mem_limit_bytes"),
    ) {
        if *limit > 0.0 && limit.is_finite() {
            penalty += (((current / limit * 100.0) - 70.0) / 30.0).clamp(0.0, 1.0) * 0.15;
            coverage += 0.15;
        }
    }
    if coverage < 0.50 {
        return None;
    }
    let score = (100.0 * (1.0 - penalty / coverage)).clamp(0.0, 100.0);
    Some(if ready { score } else { score.min(50.0) })
}

#[cfg(test)]
mod tests {
    use super::pod_resource_score;
    use std::collections::BTreeMap;

    #[test]
    fn absent_pod_signals_have_no_score() {
        assert_eq!(pod_resource_score(&BTreeMap::new(), true), None);
    }

    #[test]
    fn idle_pod_without_cpu_periods_uses_explicit_zero_throttling() {
        let values = BTreeMap::from([
            ("cgroup.cpu_throttled_per_s".to_owned(), 0.0),
            ("cgroup.mem_current_bytes".to_owned(), 32.0),
            ("cgroup.mem_limit_bytes".to_owned(), 128.0),
        ]);
        assert_eq!(pod_resource_score(&values, true), Some(100.0));
    }
}
