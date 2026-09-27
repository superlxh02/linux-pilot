//! 可解释的首版规则评分器。
//!
//! 算法保持纯函数特性：同一场景、规则版本与指标窗口必然得到同一结果。
//! 先把每项指标映射到 0～1 扣分，再计算维度分，最后按场景权重合成。
//! 缺失指标仅降低覆盖率，覆盖率低于 70% 时不给总分；未来 AI 分析可以
//! 读取 `factors` 与 `missing` 解释原因，但不会暗中改写规则分。

use linux_pilot_model::{Metric, Scenario, Score, ScoreFactor};
use std::collections::{BTreeMap, HashMap};

pub mod topology;

/// 规则方向。
#[derive(Debug, Clone, Copy)]
pub enum Direction {
    HigherIsWorse,
    LowerIsWorse,
}

/// 一个输入指标的评分规则。
///
/// `good` 是开始扣分的健康阈值，`bad` 是达到最大扣分的退化阈值；
/// `weight` 是该指标在所属维度内的相对权重，不是跨场景总权重。
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub name: &'static str,
    pub dimension: &'static str,
    pub good: f64,
    pub bad: f64,
    pub weight: f64,
    pub direction: Direction,
}

/// 评分器接口，为以后从配置加载规则或增加新算法预留替换点。
pub trait ScoreEngine {
    /// 根据一个窗口内的指标计算单一场景分。
    fn calculate(
        &self,
        host_id: &str,
        time_ms: i64,
        scenario: Scenario,
        metrics: &[Metric],
    ) -> Score;
}

/// 首版确定性规则评分器。
///
/// 规则集中存放而不是散落在 API 中，方便审查、版本化和将来由配置装载。
pub struct RuleBasedScoreEngine {
    rules: Vec<Rule>,
    version: &'static str,
}

impl Default for RuleBasedScoreEngine {
    fn default() -> Self {
        // 吞吐和忙碌率主要用于展示。这里选取等待、限流和失败等退化信号。
        let rules = vec![
            Rule {
                name: "cpu.psi.some.avg10",
                dimension: "cpu",
                good: 2.0,
                bad: 25.0,
                weight: 0.15,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "ebpf.sched.runqueue_latency_p95_ms",
                dimension: "cpu",
                good: 3.0,
                bad: 50.0,
                weight: 0.35,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "cpu.steal_pct",
                dimension: "cpu",
                good: 1.0,
                bad: 20.0,
                weight: 0.20,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "cpu.runnable_per_core",
                dimension: "cpu",
                good: 0.75,
                bad: 4.0,
                weight: 0.30,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "mem.psi.some.avg10",
                dimension: "memory",
                good: 1.0,
                bad: 20.0,
                weight: 0.15,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "mem.psi.full.avg10",
                dimension: "memory",
                good: 0.0,
                bad: 10.0,
                weight: 0.10,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "mem.available_pct",
                dimension: "memory",
                good: 20.0,
                bad: 3.0,
                weight: 0.35,
                direction: Direction::LowerIsWorse,
            },
            Rule {
                name: "mem.swap_in_pages_per_s",
                dimension: "memory",
                good: 0.0,
                bad: 100.0,
                weight: 0.15,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "mem.major_faults_per_s",
                dimension: "memory",
                good: 1.0,
                bad: 100.0,
                weight: 0.15,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "mem.oom_kills_per_s",
                dimension: "memory",
                good: 0.0,
                bad: 1.0,
                weight: 0.10,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "io.psi.some.avg10",
                dimension: "application_io",
                good: 2.0,
                bad: 30.0,
                weight: 0.15,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "io.psi.full.avg10",
                dimension: "application_io",
                good: 1.0,
                bad: 20.0,
                weight: 0.10,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "proc.block_io_delay_ms_per_s",
                dimension: "application_io",
                good: 20.0,
                bad: 700.0,
                weight: 0.10,
                direction: Direction::HigherIsWorse,
            },
            // PSI 不存在时，主机磁盘等待和 CPU iowait 提供较弱但可用的代理证据。
            Rule {
                name: "disk.read_await_ms",
                dimension: "application_io",
                good: 5.0,
                bad: 100.0,
                weight: 0.20,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "disk.write_await_ms",
                dimension: "application_io",
                good: 5.0,
                bad: 100.0,
                weight: 0.20,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "cpu.iowait_pct",
                dimension: "application_io",
                good: 1.0,
                bad: 25.0,
                weight: 0.25,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "ebpf.block.latency_p95_ms",
                dimension: "storage",
                good: 5.0,
                bad: 100.0,
                weight: 0.20,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "disk.read_await_ms",
                dimension: "storage",
                good: 5.0,
                bad: 100.0,
                weight: 0.20,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "disk.write_await_ms",
                dimension: "storage",
                good: 5.0,
                bad: 100.0,
                weight: 0.20,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "io.psi.full.avg10",
                dimension: "storage",
                good: 1.0,
                bad: 20.0,
                weight: 0.10,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "io.psi.some.avg10",
                dimension: "storage",
                good: 2.0,
                bad: 30.0,
                weight: 0.05,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "disk.avg_queue_depth",
                dimension: "storage",
                good: 1.0,
                bad: 12.0,
                weight: 0.20,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "cpu.iowait_pct",
                dimension: "storage",
                good: 1.0,
                bad: 25.0,
                weight: 0.05,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "tcp.retrans_ratio_pct",
                dimension: "network",
                good: 0.2,
                bad: 8.0,
                weight: 0.25,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "net.rx_drops_per_s",
                dimension: "network",
                good: 0.0,
                bad: 100.0,
                weight: 0.15,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "net.tx_drops_per_s",
                dimension: "network",
                good: 0.0,
                bad: 100.0,
                weight: 0.10,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "net.rx_errors_per_s",
                dimension: "network",
                good: 0.0,
                bad: 20.0,
                weight: 0.08,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "net.tx_errors_per_s",
                dimension: "network",
                good: 0.0,
                bad: 20.0,
                weight: 0.07,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "tcp.established_resets_per_s",
                dimension: "network",
                good: 0.0,
                bad: 50.0,
                weight: 0.10,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "ebpf.tcp.connect_failure_ratio_pct",
                dimension: "network",
                good: 0.1,
                bad: 10.0,
                weight: 0.15,
                direction: Direction::HigherIsWorse,
            },
            Rule {
                name: "ebpf.tcp.connect_latency_p95_ms",
                dimension: "network",
                good: 30.0,
                bad: 1000.0,
                weight: 0.10,
                direction: Direction::HigherIsWorse,
            },
        ];
        Self {
            rules,
            version: "rules-v1",
        }
    }
}

impl RuleBasedScoreEngine {
    /// 显式注入规则，用于配置变更和契约测试。
    pub fn new(rules: Vec<Rule>, version: &'static str) -> Self {
        Self { rules, version }
    }

    /// 使用清单中经过校验的维度权重计算节点分；底层指标阈值保持规则版本固定。
    /// 这样配置只改变场景关注点，不会让未校验的 JSON 改写具体测量口径。
    pub fn calculate_with_weights(
        &self,
        host_id: &str,
        time_ms: i64,
        scenario: Scenario,
        metrics: &[Metric],
        weights: &BTreeMap<String, f64>,
    ) -> Score {
        let dimensions = ["cpu", "memory", "application_io", "storage", "network"];
        let selected: Vec<(&str, f64)> = dimensions
            .iter()
            .filter_map(|name| weights.get(*name).map(|weight| (*name, *weight)))
            .collect();
        self.calculate_dimensions(host_id, time_ms, scenario, metrics, &selected)
    }

    fn calculate_dimensions(
        &self,
        host_id: &str,
        time_ms: i64,
        scenario: Scenario,
        metrics: &[Metric],
        dimensions: &[(&str, f64)],
    ) -> Score {
        let values: HashMap<&str, f64> = metrics
            .iter()
            .filter(|metric| metric.labels.is_empty() && metric.value.is_finite())
            .map(|metric| (metric.name.as_str(), metric.value))
            .collect();
        let mut factors = Vec::new();
        let mut missing = Vec::new();
        let mut dimension_scores = BTreeMap::new();
        let mut coverage = 0.0;
        let mut weighted_score = 0.0;
        let mut available_dimensions = 0.0;

        for &(dimension, dimension_weight) in dimensions {
            let mut configured = 0.0;
            let mut available = 0.0;
            let mut weighted_penalty = 0.0;
            for rule in self.rules.iter().filter(|rule| rule.dimension == dimension) {
                configured += rule.weight;
                if let Some(&value) = values.get(rule.name) {
                    let item_penalty = penalty(*rule, value);
                    available += rule.weight;
                    weighted_penalty += item_penalty * rule.weight;
                    factors.push(ScoreFactor {
                        metric: rule.name.to_owned(),
                        dimension: dimension.to_owned(),
                        value,
                        good: rule.good,
                        bad: rule.bad,
                        penalty: item_penalty,
                        weight: rule.weight * dimension_weight,
                    });
                } else {
                    missing.push(rule.name.to_owned());
                }
            }
            if configured > 0.0 {
                coverage += dimension_weight * available / configured;
            }
            let score = if available > 0.0 {
                let score = 100.0 * (1.0 - weighted_penalty / available);
                weighted_score += dimension_weight * score;
                available_dimensions += dimension_weight;
                Some(score)
            } else {
                None
            };
            dimension_scores.insert(dimension.to_owned(), score);
        }
        let value = if coverage >= 0.70 && available_dimensions > 0.0 {
            Some((weighted_score / available_dimensions).clamp(0.0, 100.0))
        } else {
            None
        };
        Score {
            host_id: host_id.to_owned(),
            time_ms,
            scenario,
            value,
            coverage,
            profile_version: self.version.to_owned(),
            dimension_scores,
            factors,
            missing,
        }
    }
}

impl ScoreEngine for RuleBasedScoreEngine {
    /// 计算场景分和完整证据。
    ///
    /// 只接受无标签主机汇总，防止设备/进程明细与整机样本混用。
    /// 每个维度按**已到达**指标权重归一化，但同时把缺失份额计入覆盖率。
    /// 所有维度分、扣分项和缺失项都返回给前端，便于复核结果。
    fn calculate(
        &self,
        host_id: &str,
        time_ms: i64,
        scenario: Scenario,
        metrics: &[Metric],
    ) -> Score {
        self.calculate_dimensions(
            host_id,
            time_ms,
            scenario,
            metrics,
            &scenario_weights(scenario),
        )
    }
}

/// 将实际值线性映射为 0～1 扣分，并在阈值外截断。
///
/// 方向由规则显式给出，因此可用内存等“越低越差”的指标和
/// 等待时长等“越高越差”的指标可以共用同一评分流程。
fn penalty(rule: Rule, value: f64) -> f64 {
    let ratio = match rule.direction {
        Direction::HigherIsWorse => (value - rule.good) / (rule.bad - rule.good),
        Direction::LowerIsWorse => (rule.good - value) / (rule.good - rule.bad),
    };
    ratio.clamp(0.0, 1.0)
}

/// 首版固定场景权重。返回顺序也是前端维度展示的稳定顺序。
///
/// 权重总和为 1；单维度场景会提高目标维度比重，但仍保留其他系统
/// 资源的证据，避免把 CPU 等相关等待完全排除在外。
fn scenario_weights(scenario: Scenario) -> [(&'static str, f64); 5] {
    match scenario {
        Scenario::General => [
            ("cpu", 0.25),
            ("memory", 0.20),
            ("application_io", 0.20),
            ("storage", 0.20),
            ("network", 0.15),
        ],
        Scenario::Cpu => [
            ("cpu", 0.45),
            ("memory", 0.20),
            ("application_io", 0.10),
            ("storage", 0.10),
            ("network", 0.15),
        ],
        Scenario::ApplicationIo => [
            ("cpu", 0.15),
            ("memory", 0.15),
            ("application_io", 0.40),
            ("storage", 0.20),
            ("network", 0.10),
        ],
        Scenario::Storage => [
            ("cpu", 0.10),
            ("memory", 0.15),
            ("application_io", 0.15),
            ("storage", 0.50),
            ("network", 0.10),
        ],
        Scenario::Network => [
            ("cpu", 0.15),
            ("memory", 0.15),
            ("application_io", 0.10),
            ("storage", 0.10),
            ("network", 0.50),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy_metrics(engine: &RuleBasedScoreEngine) -> Vec<Metric> {
        engine
            .rules
            .iter()
            .map(|rule| Metric {
                name: rule.name.to_owned(),
                value: rule.good,
                time_ms: 1,
                labels: BTreeMap::new(),
                source: "test".to_owned(),
            })
            .collect()
    }

    #[test]
    fn missing_metrics_do_not_receive_perfect_score() {
        let score = RuleBasedScoreEngine::default().calculate("host", 0, Scenario::General, &[]);
        assert_eq!(score.value, None);
    }
    #[test]
    fn penalty_is_clamped_to_one() {
        let rule = Rule {
            name: "x",
            dimension: "cpu",
            good: 1.0,
            bad: 2.0,
            weight: 1.0,
            direction: Direction::HigherIsWorse,
        };
        assert_eq!(penalty(rule, 100.0), 1.0);
    }

    #[test]
    fn healthy_complete_window_scores_one_hundred() {
        let engine = RuleBasedScoreEngine::default();
        let score = engine.calculate("host", 1, Scenario::General, &healthy_metrics(&engine));
        assert_eq!(score.value, Some(100.0));
    }

    #[test]
    fn cpu_pressure_has_larger_effect_in_cpu_scenario() {
        let engine = RuleBasedScoreEngine::default();
        let mut metrics = healthy_metrics(&engine);
        for metric in &mut metrics {
            if metric.name == "cpu.psi.some.avg10" {
                metric.value = 25.0;
            }
        }
        let cpu = engine.calculate("host", 1, Scenario::Cpu, &metrics);
        let storage = engine.calculate("host", 1, Scenario::Storage, &metrics);
        assert!(cpu.value.unwrap_or(100.0) < storage.value.unwrap_or(0.0));
    }

    #[test]
    fn psi_missing_still_scores_with_explicitly_lower_coverage() {
        let engine = RuleBasedScoreEngine::default();
        let metrics: Vec<_> = healthy_metrics(&engine)
            .into_iter()
            .filter(|metric| {
                !metric.name.contains(".psi.")
                    && !metric.name.starts_with("proc.")
                    && !metric.name.starts_with("ebpf.tcp.connect_")
            })
            .collect();
        for scenario in Scenario::all() {
            let score = engine.calculate("host", 1, scenario, &metrics);
            assert!(score.value.is_some(), "{scenario:?} 覆盖率过低");
            assert!(score.coverage < 1.0);
        }
    }
}
