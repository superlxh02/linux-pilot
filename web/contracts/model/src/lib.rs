//! 采集端、服务端和评分器共享的领域数据。
//!
//! 所有时间使用 Unix 毫秒；指标单位由稳定的指标名约定。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 单个规范化指标样本。
///
/// `labels` 只允许有限维度，例如设备名和进程身份；禁止把任意路径直接放入标签。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub value: f64,
    pub time_ms: i64,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// 数据来源，例如 proc、cgroup、ebpf 或 perf。
    pub source: String,
}

/// 一次采样窗口内的指标集合。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricBatch {
    pub host_id: String,
    pub boot_id: String,
    pub sequence: u64,
    pub metrics: Vec<Metric>,
}

/// 主机探针和环境的能力信息。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub ebpf: bool,
    pub perf: bool,
    pub cgroup_v2: bool,
    pub details: BTreeMap<String, String>,
}

/// 前端可选择的性能场景。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    General,
    Cpu,
    ApplicationIo,
    Storage,
    Network,
}

impl Scenario {
    /// 返回 API 和数据库使用的稳定标识。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Cpu => "cpu",
            Self::ApplicationIo => "application_io",
            Self::Storage => "storage",
            Self::Network => "network",
        }
    }

    /// 列出首版所有场景，避免前后端对场景集合产生歧义。
    pub const fn all() -> [Self; 5] {
        [
            Self::General,
            Self::Cpu,
            Self::ApplicationIo,
            Self::Storage,
            Self::Network,
        ]
    }
}

impl std::str::FromStr for Scenario {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "general" => Ok(Self::General),
            "cpu" => Ok(Self::Cpu),
            "application_io" => Ok(Self::ApplicationIo),
            "storage" => Ok(Self::Storage),
            "network" => Ok(Self::Network),
            _ => Err("未知性能场景"),
        }
    }
}

/// 单项指标对评分的解释。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreFactor {
    pub metric: String,
    pub dimension: String,
    pub value: f64,
    pub good: f64,
    pub bad: f64,
    pub penalty: f64,
    pub weight: f64,
}

/// 可复算、可解释的场景评分。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Score {
    pub host_id: String,
    pub time_ms: i64,
    pub scenario: Scenario,
    pub value: Option<f64>,
    pub coverage: f64,
    pub profile_version: String,
    pub dimension_scores: BTreeMap<String, Option<f64>>,
    pub factors: Vec<ScoreFactor>,
    pub missing: Vec<String>,
}
