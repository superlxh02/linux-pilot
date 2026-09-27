//! 集群拓扑配置与可复算的基础设施聚合规则。
//!
//! 这里不依赖 Kubernetes 客户端、HTTP 或数据库。外部发现的 Pod 是事实，
//! Manifest 是用户意图，两者应分别存储；AI 将来生成的文件也必须经过相同校验。

use linux_pilot_model::Scenario;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopologyManifest {
    pub api_version: String,
    pub kind: String,
    pub metadata: ManifestMetadata,
    pub spec: ManifestSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestMetadata {
    pub id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestSpec {
    pub clusters: Vec<ClusterSpec>,
    pub score_profiles: Vec<ScoreProfileSpec>,
    pub nodes: Vec<NodeSpec>,
    pub services: Vec<ServiceSpec>,
    pub system_score: SystemScorePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterSpec {
    pub id: String,
    #[serde(rename = "type")]
    pub cluster_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScoreProfileSpec {
    pub id: String,
    pub scope: String,
    pub engine: String,
    pub scenario: Scenario,
    pub weights: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeSpec {
    pub id: String,
    pub cluster_id: String,
    pub description: String,
    pub role: String,
    pub score_profile_ref: String,
    pub capacity: NodeCapacity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeCapacity {
    pub cpu_cores: u32,
    pub memory_bytes: u64,
    pub storage: Vec<StorageCapacity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageCapacity {
    pub mount: String,
    pub medium: String,
    pub capacity_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceSpec {
    pub id: String,
    pub cluster_id: String,
    pub description: String,
    pub criticality: String,
    #[serde(rename = "match")]
    pub selector: ServiceSelector,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ServiceSelector {
    KubernetesWorkload {
        namespace: String,
        kind: String,
        name: String,
    },
    /// 单体主机上按进程 comm 稳定识别应用；PID 仍由运行时发现。
    HostProcess {
        #[serde(rename = "hostId")]
        host_id: String,
        comm: String,
    },
    SystemdUnit {
        #[serde(rename = "hostId")]
        host_id: String,
        unit: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemScorePolicy {
    pub weights: BTreeMap<String, f64>,
    pub critical_service_max_delta: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PodObservation {
    pub cluster_id: String,
    pub pod_uid: String,
    pub pod_name: String,
    pub node_id: String,
    pub namespace: String,
    pub workload_kind: String,
    pub workload_name: String,
    pub ready: bool,
    pub observed_ms: i64,
}

impl TopologyManifest {
    /// 只接受已知 schema 和引用关系。所有评分权重必须完整且和为 1。
    /// 有意不读取实际遥测：配置是否有效与当前节点是否在线是两件事。
    pub fn validate(&self) -> Result<(), String> {
        if self.api_version != "linux-pilot.io/v1alpha1" || self.kind != "TopologyManifest" {
            return Err("apiVersion 或 kind 不受支持".into());
        }
        if self.metadata.id.trim().is_empty() || self.metadata.revision == 0 {
            return Err("metadata.id 和正整数 revision 必填".into());
        }
        let clusters: HashSet<_> = self
            .spec
            .clusters
            .iter()
            .map(|item| item.id.as_str())
            .collect();
        if clusters.len() != self.spec.clusters.len()
            || clusters.is_empty()
            || clusters.contains("")
        {
            return Err("clusters 必须非空且 id 唯一".into());
        }
        if self
            .spec
            .clusters
            .iter()
            .any(|item| !matches!(item.cluster_type.as_str(), "kubernetes" | "standalone"))
        {
            return Err("cluster.type 只能为 kubernetes 或 standalone".into());
        }
        let profiles: HashSet<_> = self
            .spec
            .score_profiles
            .iter()
            .map(|item| item.id.as_str())
            .collect();
        if profiles.len() != self.spec.score_profiles.len() {
            return Err("scoreProfiles.id 必须唯一".into());
        }
        for profile in &self.spec.score_profiles {
            if profile.scope != "node" || profile.engine != "rule-based/v1" {
                return Err(format!(
                    "评分配置 {} 的 scope 或 engine 不受支持",
                    profile.id
                ));
            }
            check_weights(
                &profile.weights,
                &["cpu", "memory", "application_io", "storage", "network"],
            )?;
        }
        let nodes: HashSet<_> = self
            .spec
            .nodes
            .iter()
            .map(|item| item.id.as_str())
            .collect();
        if nodes.len() != self.spec.nodes.len() {
            return Err("nodes.id 必须唯一".into());
        }
        for node in &self.spec.nodes {
            if node.id.is_empty()
                || !clusters.contains(node.cluster_id.as_str())
                || !profiles.contains(node.score_profile_ref.as_str())
            {
                return Err(format!("节点 {} 的集群或评分配置引用无效", node.id));
            }
            if node.capacity.cpu_cores == 0
                || node.capacity.memory_bytes == 0
                || node
                    .capacity
                    .storage
                    .iter()
                    .any(|disk| disk.mount.is_empty() || disk.capacity_bytes == 0)
            {
                return Err(format!("节点 {} 的硬件容量无效", node.id));
            }
        }
        let services: HashSet<_> = self
            .spec
            .services
            .iter()
            .map(|item| item.id.as_str())
            .collect();
        if services.len() != self.spec.services.len() {
            return Err("services.id 必须唯一".into());
        }
        for service in &self.spec.services {
            if service.id.is_empty()
                || !clusters.contains(service.cluster_id.as_str())
                || !matches!(
                    service.criticality.as_str(),
                    "critical" | "high" | "medium" | "low"
                )
            {
                return Err(format!("服务 {} 的集群或重要性无效", service.id));
            }
            if service
                .depends_on
                .iter()
                .any(|id| id == &service.id || !services.contains(id.as_str()))
            {
                return Err(format!("服务 {} 有无效依赖", service.id));
            }
            match &service.selector {
                ServiceSelector::KubernetesWorkload {
                    namespace,
                    kind,
                    name,
                } if namespace.is_empty() || kind.is_empty() || name.is_empty() => {
                    return Err(format!("服务 {} 的工作负载选择器不完整", service.id));
                }
                ServiceSelector::HostProcess { host_id, comm }
                    if !nodes.contains(host_id.as_str()) || comm.is_empty() || comm.len() > 15 =>
                {
                    return Err(format!("服务 {} 的主机进程选择器无效", service.id));
                }
                // v1alpha1 已保留 systemd 的 JSON 形状，但当前发现器只上报
                // Kubernetes Pod。提前拒绝，避免导入后展示空白服务且误以为
                // 它已被纳入评分；后续增加 unit 发现和独立资源仓储再开放。
                ServiceSelector::SystemdUnit { .. } => {
                    return Err(format!("服务 {} 的 systemd 归属暂未支持", service.id));
                }
                _ => {}
            }
            let cluster_type = self
                .spec
                .clusters
                .iter()
                .find(|cluster| cluster.id == service.cluster_id)
                .map(|cluster| cluster.cluster_type.as_str());
            if !matches!(
                (&service.selector, cluster_type),
                (
                    ServiceSelector::KubernetesWorkload { .. },
                    Some("kubernetes")
                ) | (ServiceSelector::HostProcess { .. }, Some("standalone"))
            ) {
                return Err(format!("服务 {} 的选择器与集群类型不匹配", service.id));
            }
            if let ServiceSelector::HostProcess { host_id, .. } = &service.selector {
                if !self
                    .spec
                    .nodes
                    .iter()
                    .any(|node| node.id == *host_id && node.cluster_id == service.cluster_id)
                {
                    return Err(format!("服务 {} 的主机不属于当前集群", service.id));
                }
            }
        }
        check_weights(
            &self.spec.system_score.weights,
            &["endToEnd", "criticalServices", "capacityAndResilience"],
        )?;
        if !self
            .spec
            .system_score
            .critical_service_max_delta
            .is_finite()
            || !(0.0..=100.0).contains(&self.spec.system_score.critical_service_max_delta)
        {
            return Err("criticalServiceMaxDelta 必须在 0～100".into());
        }
        Ok(())
    }
}

fn check_weights(weights: &BTreeMap<String, f64>, expected: &[&str]) -> Result<(), String> {
    if weights.len() != expected.len() || expected.iter().any(|key| !weights.contains_key(*key)) {
        return Err(format!("评分权重必须恰好包含 {}", expected.join(", ")));
    }
    if weights
        .values()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        || (weights.values().sum::<f64>() - 1.0).abs() > 1e-6
    {
        return Err("评分权重必须在 0～1 且总和为 1".into());
    }
    Ok(())
}

/// 仅从有效服务分合成基础设施视角的集群分。业务 SLI 缺席时不要计算整体分。
/// 关键服务的保护上限防止大量无关健康节点掩盖关键依赖失效。
pub fn infrastructure_score(scores: &[(f64, &str)], max_delta: f64) -> Option<f64> {
    let mut total = 0.0;
    let mut weight_sum = 0.0;
    let mut critical_min: Option<f64> = None;
    for &(value, criticality) in scores {
        if !value.is_finite() {
            continue;
        }
        let weight = match criticality {
            "critical" => 4.0,
            "high" => 2.0,
            "medium" => 1.0,
            _ => 0.5,
        };
        total += value * weight;
        weight_sum += weight;
        if criticality == "critical" {
            critical_min = Some(critical_min.map_or(value, |old| old.min(value)));
        }
    }
    if weight_sum == 0.0 {
        return None;
    }
    let average = total / weight_sum;
    Some(
        critical_min
            .map_or(average, |critical| average.min(critical + max_delta))
            .clamp(0.0, 100.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_failure_caps_infrastructure_score() {
        assert_eq!(
            infrastructure_score(&[(20.0, "critical"), (100.0, "low")], 10.0),
            Some(28.88888888888889)
        );
    }

    #[test]
    fn missing_services_do_not_become_perfect_score() {
        assert_eq!(infrastructure_score(&[], 10.0), None);
    }
}
