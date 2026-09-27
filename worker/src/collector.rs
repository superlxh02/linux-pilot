//! Linux /proc、/sys 和 cgroup v2 基础采集器。
//!
//! 每个分类读取内核提供的原始文件，规范化成带单位约定的 [`Metric`]。
//! 计数器以相邻采样差分计算；首次采样没有前值时不输出伪造的速率。
//! 分类读取失败只影响该分类，不能阻断其他指标或本地磁盘缓冲。

use linux_pilot_model::Metric;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
};
use tracing::warn;

#[derive(Clone, Copy, Default)]
struct CpuTimes {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
}

impl CpuTimes {
    fn total(self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }
}

#[derive(Clone, Copy, Default)]
struct DiskStats {
    reads: u64,
    read_sectors: u64,
    read_ms: u64,
    writes: u64,
    write_sectors: u64,
    write_ms: u64,
    in_flight: u64,
    busy_ms: u64,
    weighted_ms: u64,
}

#[derive(Clone, Copy, Default)]
struct NetStats {
    rx_bytes: u64,
    rx_packets: u64,
    rx_errors: u64,
    rx_drops: u64,
    tx_bytes: u64,
    tx_packets: u64,
    tx_errors: u64,
    tx_drops: u64,
}

/// 保存上一轮采样基线的采集器。
///
/// 这些 `*_previous` 映射以设备、网卡、进程或 cgroup 身份为键。若本轮缺少
/// 某个对象，下一轮重新出现时仍要审慎处理计数器回退；多数速率采用饱和差分，
/// 避免重启或计数器重置后得到负值。所有速率使用实测采样间隔而非假设恰好 1 秒。
pub struct Collector {
    ebpf: Option<crate::ebpf::EbpfCollector>,
    last_ms: Option<i64>,
    cpu_previous: HashMap<String, CpuTimes>,
    disk_previous: HashMap<String, DiskStats>,
    net_previous: HashMap<String, NetStats>,
    vm_previous: HashMap<String, u64>,
    tcp_previous: HashMap<String, u64>,
    proc_previous: HashMap<i32, (u64, u64, u64)>,
    proc_extra_previous: HashMap<i32, (u64, HashMap<String, u64>)>,
    cgroup_previous: HashMap<String, HashMap<String, u64>>,
}

impl Collector {
    /// 初始化常规采集器并尝试加载 eBPF。
    ///
    /// 探针加载失败属于能力降级，不应该让 CPU/内存等基础监控停摆；
    /// 失败原因会进入日志，握手时则把 eBPF 能力标记为不可用。
    pub fn new() -> Self {
        let ebpf = match crate::ebpf::EbpfCollector::load() {
            Ok(probes) => Some(probes),
            Err(error) => {
                warn!(%error, "eBPF 探针不可用，继续常规采集");
                None
            }
        };
        Self {
            ebpf,
            last_ms: None,
            cpu_previous: HashMap::new(),
            disk_previous: HashMap::new(),
            net_previous: HashMap::new(),
            vm_previous: HashMap::new(),
            tcp_previous: HashMap::new(),
            proc_previous: HashMap::new(),
            proc_extra_previous: HashMap::new(),
            cgroup_previous: HashMap::new(),
        }
    }

    /// 返回当前是否至少有一个内核探针成功附着。
    pub fn ebpf_available(&self) -> bool {
        self.ebpf.as_ref().is_some_and(|probes| probes.active())
    }

    /// 运行一个完整采样轮次，返回各分类的规范化样本。
    ///
    /// 首次轮次仅输出瞬时量和状态量；累计计数器的每秒速率必须等下一轮
    /// 有真实时间差后再计算。eBPF 直方图同样按本轮窗口差分读取。
    pub async fn sample(&mut self) -> Vec<Metric> {
        let now = chrono::Utc::now().timestamp_millis();
        let elapsed = self
            .last_ms
            .map(|last| (now - last) as f64 / 1000.0)
            .filter(|seconds| *seconds > 0.0);
        let mut out = Vec::new();
        self.cpu(now, elapsed, &mut out);
        self.memory(now, elapsed, &mut out);
        self.psi(now, &mut out);
        self.disks(now, elapsed, &mut out);
        self.filesystems(now, &mut out);
        self.network(now, elapsed, &mut out);
        self.processes(now, elapsed, &mut out);
        self.cgroups(now, elapsed, &mut out);
        if let (Some(probes), Some(seconds)) = (&mut self.ebpf, elapsed) {
            probes.sample(now, seconds, &mut out);
        }
        self.agent_health(now, &mut out);
        self.last_ms = Some(now);
        out
    }

    /// 集中创建样本并过滤 NaN/无穷值，避免无效浮点数进入 Protobuf 与数据库。
    fn emit(
        out: &mut Vec<Metric>,
        name: &str,
        value: f64,
        now: i64,
        source: &str,
        labels: BTreeMap<String, String>,
    ) {
        if value.is_finite() {
            out.push(Metric {
                name: name.to_owned(),
                value,
                time_ms: now,
                labels,
                source: source.to_owned(),
            });
        }
    }

    fn plain(out: &mut Vec<Metric>, name: &str, value: f64, now: i64, source: &str) {
        Self::emit(out, name, value, now, source, BTreeMap::new());
    }

    /// 从 `/proc/stat` 计算整机与每核 CPU 时间占比。
    ///
    /// `/proc/stat` 的时间字段是自开机累计的 tick；百分比必须用相邻轮次
    /// 每个状态的增量除以总增量，不能把累计值直接视作当前利用率。
    fn cpu(&mut self, now: i64, elapsed: Option<f64>, out: &mut Vec<Metric>) {
        let Ok(text) = fs::read_to_string("/proc/stat") else {
            return;
        };
        let mut online = 0;
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let Some(name) = parts.next() else { continue };
            if name.starts_with("cpu") {
                if name != "cpu" {
                    online += 1;
                }
                let values: Vec<u64> = parts.take(8).filter_map(|item| item.parse().ok()).collect();
                if values.len() < 8 {
                    continue;
                }
                let current = CpuTimes {
                    user: values[0],
                    nice: values[1],
                    system: values[2],
                    idle: values[3],
                    iowait: values[4],
                    irq: values[5],
                    softirq: values[6],
                    steal: values[7],
                };
                if let Some(previous) = self.cpu_previous.insert(name.to_owned(), current) {
                    let total = current.total().saturating_sub(previous.total()) as f64;
                    if total > 0.0 {
                        let labels = if name == "cpu" {
                            BTreeMap::new()
                        } else {
                            BTreeMap::from([("cpu_id".to_owned(), name[3..].to_owned())])
                        };
                        let fields = [
                            ("cpu.user_pct", current.user.saturating_sub(previous.user)),
                            ("cpu.nice_pct", current.nice.saturating_sub(previous.nice)),
                            (
                                "cpu.system_pct",
                                current.system.saturating_sub(previous.system),
                            ),
                            ("cpu.idle_pct", current.idle.saturating_sub(previous.idle)),
                            (
                                "cpu.iowait_pct",
                                current.iowait.saturating_sub(previous.iowait),
                            ),
                            ("cpu.irq_pct", current.irq.saturating_sub(previous.irq)),
                            (
                                "cpu.softirq_pct",
                                current.softirq.saturating_sub(previous.softirq),
                            ),
                            (
                                "cpu.steal_pct",
                                current.steal.saturating_sub(previous.steal),
                            ),
                        ];
                        let busy = fields
                            .iter()
                            .filter(|(metric, _)| {
                                !["cpu.idle_pct", "cpu.iowait_pct", "cpu.steal_pct"]
                                    .contains(metric)
                            })
                            .map(|(_, delta)| *delta)
                            .sum::<u64>();
                        Self::emit(
                            out,
                            "cpu.busy_pct",
                            busy as f64 / total * 100.0,
                            now,
                            "proc",
                            labels.clone(),
                        );
                        for (metric, delta) in fields {
                            Self::emit(
                                out,
                                metric,
                                delta as f64 / total * 100.0,
                                now,
                                "proc",
                                labels.clone(),
                            );
                        }
                    }
                }
            } else if let (Some(seconds), "ctxt" | "intr") = (elapsed, name) {
                let current = parts.next().and_then(|value| value.parse::<u64>().ok());
                if let Some(current) = current {
                    let key = format!("stat.{name}");
                    if let Some(previous) = self.vm_previous.insert(key, current) {
                        let metric = if name == "ctxt" {
                            "cpu.context_switches_per_s"
                        } else {
                            "cpu.interrupts_per_s"
                        };
                        Self::plain(
                            out,
                            metric,
                            current.saturating_sub(previous) as f64 / seconds,
                            now,
                            "proc",
                        );
                    }
                }
            }
        }
        Self::plain(out, "cpu.logical_cores", online as f64, now, "sys");
        if let Ok(loadavg) = fs::read_to_string("/proc/loadavg") {
            let fields: Vec<&str> = loadavg.split_whitespace().collect();
            for (name, index) in [("cpu.load1", 0), ("cpu.load5", 1), ("cpu.load15", 2)] {
                if let Some(value) = fields
                    .get(index)
                    .and_then(|value| value.parse::<f64>().ok())
                {
                    Self::plain(out, name, value, now, "proc");
                }
            }
            if let Some(runnable) = fields
                .get(3)
                .and_then(|value| value.split('/').next())
                .and_then(|value| value.parse::<f64>().ok())
            {
                Self::plain(out, "cpu.runnable_tasks", runnable, now, "proc");
                if online > 0 {
                    Self::plain(
                        out,
                        "cpu.runnable_per_core",
                        runnable / online as f64,
                        now,
                        "proc",
                    );
                }
            }
        }
        if let Ok(uptime) = fs::read_to_string("/proc/uptime") {
            if let Some(value) = uptime
                .split_whitespace()
                .next()
                .and_then(|part| part.parse::<f64>().ok())
            {
                Self::plain(out, "host.uptime_s", value, now, "proc");
            }
        }
    }

    fn memory(&mut self, now: i64, elapsed: Option<f64>, out: &mut Vec<Metric>) {
        let mem = read_kv("/proc/meminfo", ':', 1024);
        let mapped = [
            ("MemTotal", "mem.total_bytes"),
            ("MemAvailable", "mem.available_bytes"),
            ("MemFree", "mem.free_bytes"),
            ("Cached", "mem.cached_bytes"),
            ("Buffers", "mem.buffers_bytes"),
            ("SReclaimable", "mem.slab_reclaimable_bytes"),
            ("Dirty", "mem.dirty_bytes"),
            ("SwapTotal", "mem.swap_total_bytes"),
        ];
        for (key, name) in mapped {
            if let Some(&value) = mem.get(key) {
                Self::plain(out, name, value as f64, now, "proc");
            }
        }
        if let (Some(&total), Some(&available)) = (mem.get("MemTotal"), mem.get("MemAvailable")) {
            if total > 0 {
                Self::plain(
                    out,
                    "mem.used_pct",
                    (total - available) as f64 / total as f64 * 100.0,
                    now,
                    "proc",
                );
                Self::plain(
                    out,
                    "mem.available_pct",
                    available as f64 / total as f64 * 100.0,
                    now,
                    "proc",
                );
            }
        }
        if let (Some(&total), Some(&free)) = (mem.get("SwapTotal"), mem.get("SwapFree")) {
            Self::plain(
                out,
                "mem.swap_used_bytes",
                total.saturating_sub(free) as f64,
                now,
                "proc",
            );
        }
        let vm = read_kv("/proc/vmstat", ' ', 1);
        for (key, name) in [
            ("pswpin", "mem.swap_in_pages_per_s"),
            ("pswpout", "mem.swap_out_pages_per_s"),
            ("pgmajfault", "mem.major_faults_per_s"),
            ("oom_kill", "mem.oom_kills_per_s"),
        ] {
            self.counter_rate(&vm, key, name, now, elapsed, out);
        }
        for (prefix, metric) in [
            ("pgscan_", "mem.pgscan_pages_per_s"),
            ("pgsteal_", "mem.pgsteal_pages_per_s"),
        ] {
            let total: u64 = vm
                .iter()
                .filter(|(name, _)| name.starts_with(prefix))
                .map(|(_, value)| *value)
                .sum();
            let key = format!("vm.{prefix}");
            if let (Some(seconds), Some(previous)) = (elapsed, self.vm_previous.insert(key, total))
            {
                Self::plain(
                    out,
                    metric,
                    total.saturating_sub(previous) as f64 / seconds,
                    now,
                    "proc",
                );
            }
        }
    }

    /// 为 `/proc/vmstat` 等单调计数器计算每秒变化量。
    ///
    /// 缺少前值时仅保存基线，不输出速率；计数器回退用饱和差分保护。
    fn counter_rate(
        &mut self,
        values: &HashMap<String, u64>,
        key: &str,
        name: &str,
        now: i64,
        elapsed: Option<f64>,
        out: &mut Vec<Metric>,
    ) {
        if let Some(&current) = values.get(key) {
            if let (Some(seconds), Some(previous)) =
                (elapsed, self.vm_previous.insert(key.to_owned(), current))
            {
                Self::plain(
                    out,
                    name,
                    current.saturating_sub(previous) as f64 / seconds,
                    now,
                    "proc",
                );
            }
        }
    }

    /// 读取内核 PSI；`some` 与 `full` 的具体口径见 `web/docs/metrics.md`。
    ///
    /// CPU `full` 在系统级没有有效诊断含义，故主动跳过。
    fn psi(&self, now: i64, out: &mut Vec<Metric>) {
        for (resource, path) in [
            ("cpu", "/proc/pressure/cpu"),
            ("mem", "/proc/pressure/memory"),
            ("io", "/proc/pressure/io"),
        ] {
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            for line in text.lines() {
                let mut parts = line.split_whitespace();
                let Some(kind) = parts.next() else { continue };
                if resource == "cpu" && kind == "full" {
                    continue;
                }
                for field in parts {
                    let Some((name, value)) = field.split_once('=') else {
                        continue;
                    };
                    if let Ok(value) = value.parse::<f64>() {
                        let field = if name == "total" { "total_us" } else { name };
                        Self::plain(
                            out,
                            &format!("{resource}.psi.{kind}.{field}"),
                            value,
                            now,
                            "psi",
                        );
                    }
                }
            }
        }
    }

    /// 解析 `/proc/diskstats`，同时生成设备明细和主机汇总。
    ///
    /// 扇区按 Linux diskstats 的 512 字节口径换算，延迟只在有完成请求的
    /// 窗口中计算；空窗口不制造 0 ms 的假延迟。
    fn disks(&mut self, now: i64, elapsed: Option<f64>, out: &mut Vec<Metric>) {
        let Ok(text) = fs::read_to_string("/proc/diskstats") else {
            return;
        };
        let mut all_reads = 0_u64;
        let mut all_writes = 0_u64;
        let mut all_read_bytes = 0_u64;
        let mut all_write_bytes = 0_u64;
        let mut all_read_ms = 0_u64;
        let mut all_write_ms = 0_u64;
        let mut all_weighted_ms = 0_u64;
        let mut all_busy_ms = 0_u64;
        for line in text.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 14 {
                continue;
            }
            let device = fields[2];
            if device.starts_with("loop") || device.starts_with("ram") {
                continue;
            }
            let values: Vec<u64> = fields[3..14]
                .iter()
                .filter_map(|item| item.parse().ok())
                .collect();
            if values.len() != 11 {
                continue;
            }
            let current = DiskStats {
                reads: values[0],
                read_sectors: values[2],
                read_ms: values[3],
                writes: values[4],
                write_sectors: values[6],
                write_ms: values[7],
                in_flight: values[8],
                busy_ms: values[9],
                weighted_ms: values[10],
            };
            let labels = BTreeMap::from([("device".to_owned(), device.to_owned())]);
            Self::emit(
                out,
                "disk.in_flight",
                current.in_flight as f64,
                now,
                "proc",
                labels.clone(),
            );
            if let (Some(seconds), Some(previous)) = (
                elapsed,
                self.disk_previous.insert(device.to_owned(), current),
            ) {
                let reads = current.reads.saturating_sub(previous.reads);
                let writes = current.writes.saturating_sub(previous.writes);
                // 全机汇总只取物理设备，避免分区和上层虚拟设备重复计数。
                if Path::new("/sys/block").join(device).exists()
                    && !device.starts_with("dm-")
                    && !device.starts_with("md")
                {
                    all_reads += reads;
                    all_writes += writes;
                    all_read_bytes +=
                        current.read_sectors.saturating_sub(previous.read_sectors) * 512;
                    all_write_bytes +=
                        current.write_sectors.saturating_sub(previous.write_sectors) * 512;
                    all_read_ms += current.read_ms.saturating_sub(previous.read_ms);
                    all_write_ms += current.write_ms.saturating_sub(previous.write_ms);
                    all_weighted_ms += current.weighted_ms.saturating_sub(previous.weighted_ms);
                    all_busy_ms += current.busy_ms.saturating_sub(previous.busy_ms);
                }
                let rate_fields = [
                    (
                        "disk.read_bytes_per_s",
                        current.read_sectors.saturating_sub(previous.read_sectors) * 512,
                    ),
                    (
                        "disk.write_bytes_per_s",
                        current.write_sectors.saturating_sub(previous.write_sectors) * 512,
                    ),
                    ("disk.read_ops_per_s", reads),
                    ("disk.write_ops_per_s", writes),
                ];
                for (metric, value) in rate_fields {
                    Self::emit(
                        out,
                        metric,
                        value as f64 / seconds,
                        now,
                        "proc",
                        labels.clone(),
                    );
                }
                if reads > 0 {
                    Self::emit(
                        out,
                        "disk.read_await_ms",
                        current.read_ms.saturating_sub(previous.read_ms) as f64 / reads as f64,
                        now,
                        "proc",
                        labels.clone(),
                    );
                }
                if writes > 0 {
                    Self::emit(
                        out,
                        "disk.write_await_ms",
                        current.write_ms.saturating_sub(previous.write_ms) as f64 / writes as f64,
                        now,
                        "proc",
                        labels.clone(),
                    );
                }
                Self::emit(
                    out,
                    "disk.busy_time_pct",
                    (current.busy_ms.saturating_sub(previous.busy_ms) as f64 / (seconds * 1000.0)
                        * 100.0)
                        .min(100.0),
                    now,
                    "proc",
                    labels.clone(),
                );
                Self::emit(
                    out,
                    "disk.avg_queue_depth",
                    current.weighted_ms.saturating_sub(previous.weighted_ms) as f64
                        / (seconds * 1000.0),
                    now,
                    "proc",
                    labels,
                );
            }
        }
        if all_reads > 0 {
            Self::plain(
                out,
                "disk.read_await_ms",
                all_read_ms as f64 / all_reads as f64,
                now,
                "proc",
            );
        }
        if all_writes > 0 {
            Self::plain(
                out,
                "disk.write_await_ms",
                all_write_ms as f64 / all_writes as f64,
                now,
                "proc",
            );
        }
        if let Some(seconds) = elapsed {
            for (name, value) in [
                ("disk.read_bytes_per_s", all_read_bytes),
                ("disk.write_bytes_per_s", all_write_bytes),
                ("disk.read_ops_per_s", all_reads),
                ("disk.write_ops_per_s", all_writes),
            ] {
                Self::plain(out, name, value as f64 / seconds, now, "proc");
            }
            Self::plain(
                out,
                "disk.busy_time_pct",
                (all_busy_ms as f64 / (seconds * 1000.0) * 100.0).min(100.0),
                now,
                "proc",
            );
            Self::plain(
                out,
                "disk.avg_queue_depth",
                all_weighted_ms as f64 / (seconds * 1000.0),
                now,
                "proc",
            );
        }
    }

    /// 采集挂载点容量与 inode，避免把可用空间和文件系统已用空间混为一谈。
    fn filesystems(&self, now: i64, out: &mut Vec<Metric>) {
        let Ok(text) = fs::read_to_string("/proc/mounts") else {
            return;
        };
        for line in text.lines().take(256) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 3
                || ["proc", "sysfs", "tmpfs", "devtmpfs", "cgroup2", "overlay"].contains(&fields[2])
            {
                continue;
            }
            let mount = fields[1].replace("\\040", " ");
            let Ok(path) = std::ffi::CString::new(mount.as_bytes()) else {
                continue;
            };
            let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            // SAFETY: CString 保证 NUL 结尾；statvfs 成功后才读取初始化的输出结构。
            let result = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };
            if result != 0 {
                continue;
            }
            // SAFETY: statvfs 返回 0，内核已经写入完整结构。
            let stat = unsafe { stat.assume_init() };
            let size = stat.f_blocks as f64 * stat.f_frsize as f64;
            let available = stat.f_bavail as f64 * stat.f_frsize as f64;
            let labels = BTreeMap::from([("mount".to_owned(), mount)]);
            Self::emit(out, "fs.size_bytes", size, now, "statvfs", labels.clone());
            Self::emit(
                out,
                "fs.available_bytes",
                available,
                now,
                "statvfs",
                labels.clone(),
            );
            if stat.f_blocks > 0 {
                Self::emit(
                    out,
                    "fs.used_pct",
                    (stat.f_blocks - stat.f_bfree) as f64 / stat.f_blocks as f64 * 100.0,
                    now,
                    "statvfs",
                    labels.clone(),
                );
            }
            Self::emit(
                out,
                "fs.inodes_total",
                stat.f_files as f64,
                now,
                "statvfs",
                labels.clone(),
            );
            Self::emit(
                out,
                "fs.inodes_available",
                stat.f_favail as f64,
                now,
                "statvfs",
                labels.clone(),
            );
            if stat.f_files > 0 {
                Self::emit(
                    out,
                    "fs.inodes_used_pct",
                    (stat.f_files - stat.f_ffree) as f64 / stat.f_files as f64 * 100.0,
                    now,
                    "statvfs",
                    labels,
                );
            }
        }
    }

    /// 读取 `/proc/net/dev` 的每接口计数器并计算吞吐、包速率与丢包。
    ///
    /// 每接口名称仅作为有限维度标签；主机总量独立汇总，避免前端误把
    /// 设备标签样本与整机样本重复相加。
    fn network(&mut self, now: i64, elapsed: Option<f64>, out: &mut Vec<Metric>) {
        let mut total_rx_bytes = 0_u64;
        let mut total_tx_bytes = 0_u64;
        let mut total_rx_packets = 0_u64;
        let mut total_tx_packets = 0_u64;
        let mut total_rx_drops = 0_u64;
        let mut total_tx_drops = 0_u64;
        let mut total_rx_errors = 0_u64;
        let mut total_tx_errors = 0_u64;
        if let Ok(text) = fs::read_to_string("/proc/net/dev") {
            for line in text.lines().skip(2) {
                let Some((name, counters)) = line.split_once(':') else {
                    continue;
                };
                let name = name.trim();
                let values: Vec<u64> = counters
                    .split_whitespace()
                    .filter_map(|value| value.parse().ok())
                    .collect();
                if values.len() < 16 {
                    continue;
                }
                let current = NetStats {
                    rx_bytes: values[0],
                    rx_packets: values[1],
                    rx_errors: values[2],
                    rx_drops: values[3],
                    tx_bytes: values[8],
                    tx_packets: values[9],
                    tx_errors: values[10],
                    tx_drops: values[11],
                };
                let labels = BTreeMap::from([("interface".to_owned(), name.to_owned())]);
                if let (Some(seconds), Some(previous)) =
                    (elapsed, self.net_previous.insert(name.to_owned(), current))
                {
                    if name != "lo" {
                        total_rx_bytes += current.rx_bytes.saturating_sub(previous.rx_bytes);
                        total_tx_bytes += current.tx_bytes.saturating_sub(previous.tx_bytes);
                        total_rx_packets += current.rx_packets.saturating_sub(previous.rx_packets);
                        total_tx_packets += current.tx_packets.saturating_sub(previous.tx_packets);
                        total_rx_drops += current.rx_drops.saturating_sub(previous.rx_drops);
                        total_tx_drops += current.tx_drops.saturating_sub(previous.tx_drops);
                        total_rx_errors += current.rx_errors.saturating_sub(previous.rx_errors);
                        total_tx_errors += current.tx_errors.saturating_sub(previous.tx_errors);
                    }
                    for (metric, delta) in [
                        (
                            "net.rx_bytes_per_s",
                            current.rx_bytes.saturating_sub(previous.rx_bytes),
                        ),
                        (
                            "net.tx_bytes_per_s",
                            current.tx_bytes.saturating_sub(previous.tx_bytes),
                        ),
                        (
                            "net.rx_packets_per_s",
                            current.rx_packets.saturating_sub(previous.rx_packets),
                        ),
                        (
                            "net.tx_packets_per_s",
                            current.tx_packets.saturating_sub(previous.tx_packets),
                        ),
                        (
                            "net.rx_drops_per_s",
                            current.rx_drops.saturating_sub(previous.rx_drops),
                        ),
                        (
                            "net.tx_drops_per_s",
                            current.tx_drops.saturating_sub(previous.tx_drops),
                        ),
                        (
                            "net.rx_errors_per_s",
                            current.rx_errors.saturating_sub(previous.rx_errors),
                        ),
                        (
                            "net.tx_errors_per_s",
                            current.tx_errors.saturating_sub(previous.tx_errors),
                        ),
                    ] {
                        Self::emit(
                            out,
                            metric,
                            delta as f64 / seconds,
                            now,
                            "proc",
                            labels.clone(),
                        );
                    }
                }
                if let Ok(state) = fs::read_to_string(format!("/sys/class/net/{name}/operstate")) {
                    Self::emit(
                        out,
                        "net.interface_up",
                        f64::from(state.trim() == "up"),
                        now,
                        "sys",
                        labels.clone(),
                    );
                }
                if let Ok(speed) = fs::read_to_string(format!("/sys/class/net/{name}/speed")) {
                    if let Ok(mbps) = speed.trim().parse::<f64>() {
                        if mbps > 0.0 {
                            Self::emit(
                                out,
                                "net.interface_speed_bps",
                                mbps * 1_000_000.0,
                                now,
                                "sys",
                                labels.clone(),
                            );
                        }
                    }
                }
            }
        }
        if let Some(seconds) = elapsed {
            for (name, value) in [
                ("net.rx_bytes_per_s", total_rx_bytes),
                ("net.tx_bytes_per_s", total_tx_bytes),
                ("net.rx_packets_per_s", total_rx_packets),
                ("net.tx_packets_per_s", total_tx_packets),
                ("net.rx_drops_per_s", total_rx_drops),
                ("net.tx_drops_per_s", total_tx_drops),
                ("net.rx_errors_per_s", total_rx_errors),
                ("net.tx_errors_per_s", total_tx_errors),
            ] {
                Self::plain(out, name, value as f64 / seconds, now, "proc");
            }
        }
        self.tcp(now, elapsed, out);
    }

    /// 从内核 TCP 计数器计算协议层事件，不把网卡错误等同于 TCP 重传。
    fn tcp(&mut self, now: i64, elapsed: Option<f64>, out: &mut Vec<Metric>) {
        let Ok(text) = fs::read_to_string("/proc/net/snmp") else {
            return;
        };
        let mut current = HashMap::new();
        let lines: Vec<&str> = text.lines().collect();
        for pair in lines.chunks_exact(2) {
            if !pair[0].starts_with("Tcp:") {
                continue;
            }
            let names = pair[0].split_whitespace().skip(1);
            let values = pair[1].split_whitespace().skip(1);
            for (name, value) in names.zip(values) {
                if let Ok(value) = value.parse::<u64>() {
                    current.insert(name.to_owned(), value);
                }
            }
        }
        let mappings = [
            ("RetransSegs", "tcp.retrans_segments_per_s"),
            ("OutSegs", "tcp.out_segments_per_s"),
            ("ActiveOpens", "tcp.active_opens_per_s"),
            ("PassiveOpens", "tcp.passive_opens_per_s"),
            ("EstabResets", "tcp.established_resets_per_s"),
        ];
        let mut deltas = HashMap::new();
        for (key, metric) in mappings {
            if let Some(&value) = current.get(key) {
                if let (Some(seconds), Some(previous)) =
                    (elapsed, self.tcp_previous.insert(key.to_owned(), value))
                {
                    let delta = value.saturating_sub(previous);
                    deltas.insert(key, delta);
                    Self::plain(out, metric, delta as f64 / seconds, now, "proc");
                }
            }
        }
        if let (Some(&retrans), Some(&sent)) = (deltas.get("RetransSegs"), deltas.get("OutSegs")) {
            if sent >= 10 {
                Self::plain(
                    out,
                    "tcp.retrans_ratio_pct",
                    retrans as f64 / sent as f64 * 100.0,
                    now,
                    "proc",
                );
            }
        }
        if let Some(&established) = current.get("CurrEstab") {
            Self::plain(
                out,
                "tcp.established_connections",
                established as f64,
                now,
                "proc",
            );
        }
        let mut listen = 0_u64;
        let mut time_wait = 0_u64;
        for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
            if let Ok(text) = fs::read_to_string(path) {
                for line in text.lines().skip(1) {
                    match line.split_whitespace().nth(3) {
                        Some("0A") => listen += 1,
                        Some("06") => time_wait += 1,
                        _ => {}
                    }
                }
            }
        }
        Self::plain(out, "tcp.listen_connections", listen as f64, now, "proc");
        Self::plain(
            out,
            "tcp.time_wait_connections",
            time_wait as f64,
            now,
            "proc",
        );
    }

    /// 有界扫描进程并输出资源占用 Top N，控制高基数标签和传输体积。
    fn processes(&mut self, now: i64, elapsed: Option<f64>, out: &mut Vec<Metric>) {
        let Ok(entries) = fs::read_dir("/proc") else {
            return;
        };
        let mut candidates = Vec::new();
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            let Ok(text) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            let Some((_, tail)) = text.rsplit_once(") ") else {
                continue;
            };
            let fields: Vec<&str> = tail.split_whitespace().collect();
            if fields.len() < 22 {
                continue;
            }
            let (Ok(user), Ok(system), Ok(start)) = (
                fields[11].parse::<u64>(),
                fields[12].parse::<u64>(),
                fields[19].parse::<u64>(),
            ) else {
                continue;
            };
            let previous = self
                .proc_previous
                .insert(pid, (start, user, system))
                .filter(|(old_start, _, _)| *old_start == start);
            let user_ticks = previous
                .map(|(_, old_user, _)| user.saturating_sub(old_user))
                .unwrap_or(0);
            let system_ticks = previous
                .map(|(_, _, old_system)| system.saturating_sub(old_system))
                .unwrap_or(0);
            let major_faults = fields
                .get(9)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            let block_delay = fields.get(39).and_then(|value| value.parse::<u64>().ok());
            candidates.push((
                pid,
                start,
                user_ticks,
                system_ticks,
                major_faults,
                block_delay,
            ));
        }
        candidates.sort_by_key(|(_, _, user, system, _, _)| std::cmp::Reverse(*user + *system));
        for (pid, start, user_ticks, system_ticks, major_faults, block_delay) in
            candidates.into_iter().take(20)
        {
            let comm = fs::read_to_string(format!("/proc/{pid}/comm"))
                .unwrap_or_default()
                .trim()
                .chars()
                .take(64)
                .collect::<String>();
            let labels = BTreeMap::from([
                ("pid".to_owned(), pid.to_string()),
                ("start_ticks".to_owned(), start.to_string()),
                ("comm".to_owned(), comm),
            ]);
            if let Some(seconds) = elapsed {
                // USER_HZ 通过 sysconf 获取，不能假设所有体系结构都是 100。
                let hertz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
                if hertz > 0.0 {
                    for (name, ticks) in [
                        ("proc.cpu_pct", user_ticks + system_ticks),
                        ("proc.user_cpu_pct", user_ticks),
                        ("proc.system_cpu_pct", system_ticks),
                    ] {
                        Self::emit(
                            out,
                            name,
                            ticks as f64 / hertz / seconds * 100.0,
                            now,
                            "proc",
                            labels.clone(),
                        );
                    }
                }
            }
            let status = read_kv(format!("/proc/{pid}/status"), ':', 1);
            for (key, metric, scale) in [
                ("VmRSS", "proc.rss_bytes", 1024.0),
                ("VmSize", "proc.vmsize_bytes", 1024.0),
                ("Threads", "proc.threads", 1.0),
            ] {
                if let Some(&value) = status.get(key) {
                    Self::emit(
                        out,
                        metric,
                        value as f64 * scale,
                        now,
                        "proc",
                        labels.clone(),
                    );
                }
            }
            let io = read_kv(format!("/proc/{pid}/io"), ':', 1);
            let mut snapshot = io.clone();
            snapshot.insert("majflt".to_owned(), major_faults);
            if let Some(block_delay) = block_delay {
                snapshot.insert("block_delay".to_owned(), block_delay);
            }
            for key in ["voluntary_ctxt_switches", "nonvoluntary_ctxt_switches"] {
                if let Some(&value) = status.get(key) {
                    snapshot.insert(key.to_owned(), value);
                }
            }
            if let (Some(seconds), Some((old_start, previous))) = (
                elapsed,
                self.proc_extra_previous
                    .insert(pid, (start, snapshot.clone())),
            ) {
                if old_start == start {
                    for (key, metric, multiplier) in [
                        ("read_bytes", "proc.read_bytes_per_s", 1.0),
                        ("write_bytes", "proc.write_bytes_per_s", 1.0),
                        ("syscr", "proc.read_syscalls_per_s", 1.0),
                        ("syscw", "proc.write_syscalls_per_s", 1.0),
                        ("majflt", "proc.major_faults_per_s", 1.0),
                        ("block_delay", "proc.block_io_delay_ms_per_s", 10.0),
                        ("voluntary_ctxt_switches", "proc.voluntary_ctxt_per_s", 1.0),
                        (
                            "nonvoluntary_ctxt_switches",
                            "proc.involuntary_ctxt_per_s",
                            1.0,
                        ),
                    ] {
                        if let (Some(&current), Some(&before)) =
                            (snapshot.get(key), previous.get(key))
                        {
                            Self::emit(
                                out,
                                metric,
                                current.saturating_sub(before) as f64 * multiplier / seconds,
                                now,
                                "proc",
                                labels.clone(),
                            );
                        }
                    }
                }
            }
        }
        // 已退出进程的快照下轮无须保留，避免长时间运行时内存不断增长。
        self.proc_previous
            .retain(|pid, _| Path::new(&format!("/proc/{pid}")).exists());
        self.proc_extra_previous
            .retain(|pid, _| Path::new(&format!("/proc/{pid}")).exists());
    }

    /// 读取 cgroup v2 用量与限制；不可用的控制器只影响对应子指标。
    fn cgroups(&mut self, now: i64, elapsed: Option<f64>, out: &mut Vec<Metric>) {
        let root = Path::new("/sys/fs/cgroup");
        if !root.join("cgroup.controllers").exists() {
            return;
        }
        let mut groups = vec![root.to_path_buf()];
        if let Ok(entries) = fs::read_dir(root) {
            groups.extend(
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .take(20),
            );
        }
        for path in groups {
            let group = path
                .strip_prefix(root)
                .ok()
                .and_then(|item| item.to_str())
                .unwrap_or("");
            let labels = BTreeMap::from([(
                "cgroup".to_owned(),
                if group.is_empty() {
                    "/".to_owned()
                } else {
                    format!("/{group}")
                },
            )]);
            let mut snapshot = HashMap::new();
            for (file, fields) in [
                (
                    "cpu.stat",
                    vec![
                        ("usage_usec", "cgroup.cpu_usage_pct"),
                        ("nr_periods", ""),
                        ("nr_throttled", "cgroup.cpu_throttled_per_s"),
                        ("throttled_usec", "cgroup.cpu_throttled_time_ms_per_s"),
                    ],
                ),
                (
                    "memory.events",
                    vec![
                        ("high", "cgroup.mem_high_events_per_s"),
                        ("oom_kill", "cgroup.oom_kills_per_s"),
                    ],
                ),
            ] {
                let values = read_kv(path.join(file), ' ', 1);
                for (key, metric) in fields {
                    if let Some(&value) = values.get(key) {
                        let snapshot_key = format!("{file}:{key}");
                        snapshot.insert(snapshot_key.clone(), value);
                        if let (Some(seconds), Some(previous)) = (
                            elapsed,
                            self.cgroup_previous
                                .get(group)
                                .and_then(|previous| previous.get(&snapshot_key)),
                        ) {
                            let delta = value.saturating_sub(*previous) as f64;
                            let result = if key == "usage_usec" {
                                delta / 1_000_000.0 / seconds * 100.0
                            } else if key == "throttled_usec" {
                                delta / 1000.0 / seconds
                            } else {
                                delta / seconds
                            };
                            if !metric.is_empty() {
                                Self::emit(out, metric, result, now, "cgroup", labels.clone());
                            }
                        }
                    }
                }
            }
            if let Some(previous) = self.cgroup_previous.get(group) {
                let periods = snapshot
                    .get("cpu.stat:nr_periods")
                    .copied()
                    .unwrap_or(0)
                    .saturating_sub(previous.get("cpu.stat:nr_periods").copied().unwrap_or(0));
                let throttled = snapshot
                    .get("cpu.stat:nr_throttled")
                    .copied()
                    .unwrap_or(0)
                    .saturating_sub(previous.get("cpu.stat:nr_throttled").copied().unwrap_or(0));
                if periods > 0 {
                    Self::emit(
                        out,
                        "cgroup.cpu_throttled_period_ratio_pct",
                        throttled as f64 / periods as f64 * 100.0,
                        now,
                        "cgroup",
                        labels.clone(),
                    );
                }
            }
            if let Ok(value) = fs::read_to_string(path.join("cpu.max")) {
                let mut parts = value.split_whitespace();
                if let (Some(quota), Some(period)) = (parts.next(), parts.next()) {
                    if let (Ok(quota), Ok(period)) = (quota.parse::<f64>(), period.parse::<f64>()) {
                        if period > 0.0 {
                            Self::emit(
                                out,
                                "cgroup.cpu_quota_cores",
                                quota / period,
                                now,
                                "cgroup",
                                labels.clone(),
                            );
                        }
                    }
                }
            }
            if let Ok(text) = fs::read_to_string(path.join("io.stat")) {
                let mut totals = [0_u64; 4];
                for line in text.lines() {
                    for field in line.split_whitespace().skip(1) {
                        if let Some((key, value)) = field.split_once('=') {
                            if let Ok(value) = value.parse::<u64>() {
                                let index = match key {
                                    "rbytes" => Some(0),
                                    "wbytes" => Some(1),
                                    "rios" => Some(2),
                                    "wios" => Some(3),
                                    _ => None,
                                };
                                if let Some(index) = index {
                                    totals[index] += value;
                                }
                            }
                        }
                    }
                }
                for (index, key, metric) in [
                    (0, "io:rbytes", "cgroup.io_read_bytes_per_s"),
                    (1, "io:wbytes", "cgroup.io_write_bytes_per_s"),
                    (2, "io:rios", "cgroup.io_read_ops_per_s"),
                    (3, "io:wios", "cgroup.io_write_ops_per_s"),
                ] {
                    snapshot.insert(key.to_owned(), totals[index]);
                    if let (Some(seconds), Some(previous)) = (
                        elapsed,
                        self.cgroup_previous.get(group).and_then(|old| old.get(key)),
                    ) {
                        Self::emit(
                            out,
                            metric,
                            totals[index].saturating_sub(*previous) as f64 / seconds,
                            now,
                            "cgroup",
                            labels.clone(),
                        );
                    }
                }
            }
            for (file, metric) in [
                ("memory.current", "cgroup.mem_current_bytes"),
                ("memory.max", "cgroup.mem_limit_bytes"),
            ] {
                if let Ok(value) = fs::read_to_string(path.join(file)) {
                    if let Ok(value) = value.trim().parse::<f64>() {
                        Self::emit(out, metric, value, now, "cgroup", labels.clone());
                    }
                }
            }
            for (resource, file) in [
                ("cpu", "cpu.pressure"),
                ("memory", "memory.pressure"),
                ("io", "io.pressure"),
            ] {
                if let Ok(text) = fs::read_to_string(path.join(file)) {
                    for line in text.lines() {
                        let mut parts = line.split_whitespace();
                        let Some(kind) = parts.next() else { continue };
                        if resource == "cpu" && kind == "full" {
                            continue;
                        }
                        for field in parts {
                            let Some((name, value)) = field.split_once('=') else {
                                continue;
                            };
                            if let Ok(value) = value.parse::<f64>() {
                                let name = if name == "total" { "total_us" } else { name };
                                Self::emit(
                                    out,
                                    &format!("cgroup.psi.{resource}.{kind}.{name}"),
                                    value,
                                    now,
                                    "cgroup",
                                    labels.clone(),
                                );
                            }
                        }
                    }
                }
            }
            self.cgroup_previous.insert(group.to_owned(), snapshot);
        }
    }

    /// 采集 Worker 自身的 RSS 与 perf 能力，便于区分目标负载和采集开销。
    fn agent_health(&self, now: i64, out: &mut Vec<Metric>) {
        if let Ok(text) = fs::read_to_string("/proc/self/status") {
            for line in text.lines() {
                if let Some(value) = line
                    .strip_prefix("VmRSS:")
                    .and_then(|value| value.split_whitespace().next())
                    .and_then(|value| value.parse::<f64>().ok())
                {
                    Self::plain(out, "agent.rss_bytes", value * 1024.0, now, "agent");
                }
            }
        }
        Self::plain(
            out,
            "agent.perf_available",
            f64::from(crate::perf::available()),
            now,
            "agent",
        );
    }
}

/// 读取 proc/cgroup 的简单键值文件；字段缺失时自然降级。
fn read_kv(path: impl AsRef<Path>, separator: char, scale: u64) -> HashMap<String, u64> {
    let mut values = HashMap::new();
    let Ok(text) = fs::read_to_string(path) else {
        return values;
    };
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(separator) else {
            continue;
        };
        if let Some(value) = rest
            .split_whitespace()
            .next()
            .and_then(|part| part.parse::<u64>().ok())
        {
            values.insert(key.to_owned(), value.saturating_mul(scale));
        }
    }
    values
}
