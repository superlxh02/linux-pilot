//! Aya 用户态探针加载与直方图读取。
//!
//! 探针按能力独立加载；某个 tracepoint 缺失不会影响其他探针或基础采集。

use anyhow::{Context, Result};
use aya::{
    Btf, Ebpf,
    maps::{Array, HashMap as AyaHashMap, MapData},
    programs::{BtfTracePoint, KProbe, TracePoint},
};
use linux_pilot_model::Metric;
use std::collections::{BTreeMap, HashMap};
use tracing::warn;

pub struct EbpfCollector {
    _programs: Ebpf,
    counters: Array<MapData, u64>,
    previous: [u64; 106],
    attached: usize,
    socket: AyaHashMap<MapData, u32, SocketCounters>,
    socket_previous: HashMap<(u32, u64), SocketCounters>,
}

/// 与 eBPF C 结构体共享内存布局；两个 u64 都是单调递增计数器。
#[repr(C)]
#[derive(Clone, Copy)]
struct SocketCounters {
    tx_bytes: u64,
    rx_bytes: u64,
}

// SAFETY: repr(C) 且全部字段是 u64，没有指针、生命周期或无效位模式。
unsafe impl aya::Pod for SocketCounters {}

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
        for (program_name, tracepoint) in [
            ("process_sendto", "sys_exit_sendto"),
            ("process_sendmsg", "sys_exit_sendmsg"),
            ("process_recvfrom", "sys_exit_recvfrom"),
            ("process_recvmsg", "sys_exit_recvmsg"),
        ] {
            let result = (|| -> Result<()> {
                let program: &mut TracePoint = programs
                    .program_mut(program_name)
                    .context("缺少进程 socket 探针")?
                    .try_into()?;
                program.load()?;
                program.attach("syscalls", tracepoint)?;
                Ok(())
            })();
            if let Err(error) = result {
                warn!(%error, tracepoint, "进程 socket 探针加载失败");
            } else {
                attached += 1;
            }
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
        let socket = AyaHashMap::try_from(
            programs
                .take_map("PROCESS_SOCKET")
                .context("缺少进程 socket map")?,
        )?;
        let mut previous = [0_u64; 106];
        for (index, slot) in previous.iter_mut().enumerate() {
            *slot = counters.get(&(index as u32), 0).unwrap_or(0);
        }
        Ok(Self {
            _programs: programs,
            counters,
            previous,
            attached,
            socket,
            socket_previous: HashMap::new(),
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
        let mut delta = [0_u64; 106];
        for index in 0..106 {
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
        // 全局 syscall 命中率用于判断进程归因探针是否真的有样本；
        // 某进程缺少 socket 字节时，不能仅凭总 eBPF 可用就推断为零流量。
        emit("ebpf.socket_send_calls_per_s", delta[104] as f64 / seconds);
        emit("ebpf.socket_recv_calls_per_s", delta[105] as f64 / seconds);
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

    /// 只把固定监控目标的 syscall socket 字节计数送入时序库。
    /// 这是 sendto/sendmsg/recvfrom/recvmsg 的成功返回值，不是网卡线上字节。
    pub fn sample_watched(
        &mut self,
        now: i64,
        seconds: f64,
        watched: &[(i32, u64)],
        output: &mut Vec<Metric>,
    ) {
        let mut active = std::collections::HashSet::new();
        for &(pid, start_ticks) in watched {
            let Ok(pid_key) = u32::try_from(pid) else {
                continue;
            };
            let Ok(current) = self.socket.get(&pid_key, 0) else {
                continue;
            };
            let key = (pid_key, start_ticks);
            active.insert(key);
            if let Some(before) = self.socket_previous.insert(key, current) {
                let labels = BTreeMap::from([
                    ("pid".to_owned(), pid.to_string()),
                    ("start_ticks".to_owned(), start_ticks.to_string()),
                ]);
                for (name, delta) in [
                    (
                        "proc.socket_tx_bytes_per_s",
                        current.tx_bytes.saturating_sub(before.tx_bytes),
                    ),
                    (
                        "proc.socket_rx_bytes_per_s",
                        current.rx_bytes.saturating_sub(before.rx_bytes),
                    ),
                ] {
                    output.push(Metric {
                        name: name.to_owned(),
                        value: delta as f64 / seconds,
                        time_ms: now,
                        labels: labels.clone(),
                        source: "ebpf-syscall".to_owned(),
                    });
                }
            }
        }
        self.socket_previous.retain(|key, _| active.contains(key));
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
