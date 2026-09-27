//! Aya 用户态探针加载与直方图读取。
//!
//! 探针按能力独立加载；某个 tracepoint 缺失不会影响其他探针或基础采集。

use anyhow::{Context, Result};
use aya::{
    Btf, Ebpf,
    maps::{Array, MapData},
    programs::{BtfTracePoint, KProbe, TracePoint},
};
use linux_pilot_model::Metric;
use std::collections::BTreeMap;
use tracing::warn;

pub struct EbpfCollector {
    _programs: Ebpf,
    counters: Array<MapData, u64>,
    previous: [u64; 104],
    attached: usize,
}

impl EbpfCollector {
    /// 逐个加载探针，保留能工作的子集。
    ///
    /// tracepoint、BTF 和内核符号在发行版间差异较大，单个探针失败不能
    /// 撤销已成功附着的探针。若一个都无法附着才返回错误，让基础采集继续。
    pub fn load() -> Result<Self> {
        let mut programs =
            Ebpf::load(include_bytes!("../ebpf/pilot.bpf.o")).context("解析 eBPF ELF 失败")?;
        let mut attached = 0;
        let tcp_result = (|| -> Result<()> {
            let program: &mut KProbe = programs
                .program_mut("tcp_retrans")
                .context("缺少 TCP 探针")?
                .try_into()?;
            program.load()?;
            program.attach("tcp_retransmit_skb", 0)?;
            Ok(())
        })();
        if let Err(error) = tcp_result {
            warn!(%error, "TCP 重传探针加载失败");
        } else {
            attached += 1;
        }
        let sched_result = (|| -> Result<()> {
            let program: &mut TracePoint = programs
                .program_mut("sched_switch")
                .context("缺少调度探针")?
                .try_into()?;
            program.load()?;
            program.attach("sched", "sched_switch")?;
            Ok(())
        })();
        if let Err(error) = sched_result {
            warn!(%error, "调度探针加载失败");
        } else {
            attached += 1;
        }
        if let Ok(btf) = Btf::from_sys_fs() {
            for (program_name, tracepoint) in [
                ("block_issue", "block_rq_issue"),
                ("block_complete", "block_rq_complete"),
                ("sched_wakeup", "sched_wakeup"),
                ("sched_wait_finish", "sched_switch"),
                ("tcp_state", "inet_sock_set_state"),
            ] {
                let result = (|| -> Result<()> {
                    let program: &mut BtfTracePoint = programs
                        .program_mut(program_name)
                        .context("缺少块设备探针")?
                        .try_into()?;
                    program.load(tracepoint, &btf)?;
                    program.attach()?;
                    Ok(())
                })();
                if let Err(error) = result {
                    warn!(%error, tracepoint, "块设备探针加载失败");
                } else {
                    attached += 1;
                }
            }
        }
        if attached == 0 {
            anyhow::bail!("没有可加载的 eBPF 探针")
        }
        let counters = Array::try_from(
            programs
                .take_map("COUNTERS")
                .context("缺少 BPF 计数器 map")?,
        )?;
        let mut previous = [0_u64; 104];
        for (index, slot) in previous.iter_mut().enumerate() {
            *slot = counters.get(&(index as u32), 0).unwrap_or(0);
        }
        Ok(Self {
            _programs: programs,
            counters,
            previous,
            attached,
        })
    }

    /// 至少一个探针附着后，能力握手才报告 eBPF 可用。
    pub fn active(&self) -> bool {
        self.attached > 0
    }

    /// 从 BPF map 读取本轮增量并输出速率和延迟分位数。
    ///
    /// map 内计数器自加载以来累计；`previous` 只在用户态维护，故每轮需
    /// 差分。分位数只在直方图有样本时输出，避免以零延迟掩盖数据缺失。
    pub fn sample(&mut self, now: i64, seconds: f64, output: &mut Vec<Metric>) {
        let mut delta = [0_u64; 104];
        for index in 0..104 {
            if let Ok(current) = self.counters.get(&(index as u32), 0) {
                delta[index] = current.saturating_sub(self.previous[index]);
                self.previous[index] = current;
            }
        }
        let mut emit = |name: &str, value: f64| {
            output.push(Metric {
                name: name.to_owned(),
                value,
                time_ms: now,
                labels: BTreeMap::new(),
                source: "ebpf".to_owned(),
            });
        };
        emit("ebpf.tcp.retransmits_per_s", delta[0] as f64 / seconds);
        emit("ebpf.sched.switches_per_s", delta[1] as f64 / seconds);
        emit("ebpf.block.completed_per_s", delta[2] as f64 / seconds);
        if delta[2] > 0 {
            emit(
                "ebpf.block.mean_latency_ms",
                delta[3] as f64 / delta[2] as f64 / 1_000_000.0,
            );
            if let Some(value) = histogram_p95(&delta[4..36]) {
                emit("ebpf.block.latency_p95_ms", value);
            }
        }
        emit(
            "ebpf.tcp.connect_attempts_per_s",
            delta[36] as f64 / seconds,
        );
        emit(
            "ebpf.tcp.connect_failures_per_s",
            delta[37] as f64 / seconds,
        );
        if delta[36] >= 10 {
            emit(
                "ebpf.tcp.connect_failure_ratio_pct",
                delta[37] as f64 / delta[36] as f64 * 100.0,
            );
        }
        if let Some(value) = histogram_p95(&delta[39..71]) {
            emit("ebpf.tcp.connect_latency_p95_ms", value);
        }
        emit("ebpf.sched.long_waits_per_s", delta[71] as f64 / seconds);
        if let Some(value) = histogram_p95(&delta[72..104]) {
            emit("ebpf.sched.runqueue_latency_p95_ms", value);
        }
    }
}

/// 在对数桶直方图上估算 p95；结果是桶上界近似值而非精确逐事件分位数。
fn histogram_p95(buckets: &[u64]) -> Option<f64> {
    let total: u64 = buckets.iter().sum();
    if total == 0 {
        return None;
    }
    let threshold = (total as f64 * 0.95).ceil() as u64;
    let mut seen = 0_u64;
    for (index, count) in buckets.iter().enumerate() {
        seen += count;
        if seen >= threshold {
            return Some((1_u64 << index) as f64 / 1000.0);
        }
    }
    None
}
