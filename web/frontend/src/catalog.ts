/** 页面展示的简版指标说明。完整口径、单位和采集前提见 web/docs/metrics.md。 */
export interface MetricInfo { title: string; unit: string; meaning: string }

export const categories = [
  { id: 'cpu', title: 'CPU 与调度', summary: '利用率、排队与调度等待', chart: ['cpu.busy_pct', 'cpu.system_pct', 'cpu.iowait_pct', 'cpu.steal_pct'] },
  { id: 'mem', title: '内存', summary: '可用容量、回收与缺页', chart: ['mem.used_pct', 'mem.available_pct', 'mem.psi.some.avg10'] },
  { id: 'disk', title: '块设备', summary: '吞吐、IOPS、队列与延迟', chart: ['disk.read_bytes_per_s', 'disk.write_bytes_per_s', 'disk.avg_queue_depth'] },
  { id: 'fs', title: '文件系统', summary: '容量与 inode', chart: ['fs.used_pct', 'fs.inodes_used_pct'] },
  { id: 'net', title: '网络接口', summary: '吞吐、丢包与错误', chart: ['net.rx_bytes_per_s', 'net.tx_bytes_per_s', 'net.rx_drops_per_s'] },
  { id: 'tcp', title: 'TCP', summary: '连接、重传与复位', chart: ['tcp.retrans_ratio_pct', 'tcp.active_opens_per_s', 'tcp.established_connections'] },
  { id: 'proc', title: '进程', summary: 'CPU、内存与 I/O 热点', chart: ['proc.cpu_pct', 'proc.read_bytes_per_s', 'proc.write_bytes_per_s'] },
  { id: 'cgroup', title: 'cgroup', summary: '容器资源与限流', chart: ['cgroup.cpu_usage_pct', 'cgroup.mem_current_bytes'] },
  { id: 'ebpf', title: 'eBPF', summary: '内核事件与尾延迟', chart: ['ebpf.sched.runqueue_latency_p95_ms', 'ebpf.block.latency_p95_ms', 'ebpf.tcp.connect_latency_p95_ms'] },
  { id: 'agent', title: '采集质量', summary: '缓存与采集端开销', chart: ['agent.buffer_batches', 'agent.rss_bytes'] }
] as const

export const metricInfo: Record<string, MetricInfo> = {
  'cpu.busy_pct': { title: 'CPU 忙碌率', unit: '%', meaning: '用户态、内核态与中断实际计算时间占比；高利用率须结合排队判断瓶颈。' },
  'cpu.user_pct': { title: '用户态时间', unit: '%', meaning: '应用代码占用 CPU 的比例。' },
  'cpu.system_pct': { title: '内核态时间', unit: '%', meaning: '系统调用与内核工作占用 CPU 的比例。' },
  'cpu.iowait_pct': { title: 'I/O 等待占比', unit: '%', meaning: 'CPU 空闲且有待完成 I/O 的时间占比，不等同设备延迟。' },
  'cpu.steal_pct': { title: '虚拟化窃取时间', unit: '%', meaning: '虚拟机未得到物理 CPU 的时间占比。' },
  'cpu.runnable_per_core': { title: '每核运行队列', unit: '任务/核', meaning: '可运行任务数除以逻辑核心数，用于观察 CPU 排队。' },
  'cpu.load1': { title: '1 分钟负载', unit: '', meaning: '运行或不可中断任务的平均数；需与核心数对照。' },
  'cpu.logical_cores': { title: '逻辑核心', unit: '核', meaning: '当前可用逻辑 CPU 数。' },
  'cpu.context_switches_per_s': { title: '上下文切换', unit: '次/秒', meaning: '全机任务上下文切换速率。' },
  'cpu.psi.some.avg10': { title: 'CPU 压力', unit: '%', meaning: '最近 10 秒至少一个任务等待 CPU 的时间比例；需内核 PSI 支持。' },
  'mem.used_pct': { title: '内存使用率', unit: '%', meaning: '按 MemAvailable 计算的有效使用比例。' },
  'mem.available_pct': { title: '可用内存比例', unit: '%', meaning: '无需严重交换即可分配的内存估计值占比。' },
  'mem.total_bytes': { title: '物理内存', unit: 'B', meaning: '系统可用物理内存总量。' },
  'mem.available_bytes': { title: '可用内存', unit: 'B', meaning: '内核估计可供新应用使用的内存。' },
  'mem.swap_used_bytes': { title: '交换区使用', unit: 'B', meaning: '已占用 swap 空间，需结合换入换出速率判断。' },
  'mem.major_faults_per_s': { title: '重大缺页', unit: '次/秒', meaning: '需要外部读取的缺页速率。' },
  'mem.oom_kills_per_s': { title: 'OOM 结束任务', unit: '次/秒', meaning: '内存不足导致任务被结束的速率。' },
  'mem.psi.some.avg10': { title: '内存压力', unit: '%', meaning: '最近 10 秒至少一个任务因内存受阻的时间比例。' },
  'disk.read_bytes_per_s': { title: '磁盘读取吞吐', unit: 'B/s', meaning: '块设备完成读取的字节速率。' },
  'disk.write_bytes_per_s': { title: '磁盘写入吞吐', unit: 'B/s', meaning: '块设备完成写入的字节速率。' },
  'disk.read_ops_per_s': { title: '读取 IOPS', unit: '次/秒', meaning: '每秒完成的块读取请求。' },
  'disk.write_ops_per_s': { title: '写入 IOPS', unit: '次/秒', meaning: '每秒完成的块写入请求。' },
  'disk.read_await_ms': { title: '平均读等待', unit: 'ms', meaning: '完成的读请求平均历时，包括排队和服务。' },
  'disk.write_await_ms': { title: '平均写等待', unit: 'ms', meaning: '完成的写请求平均历时，包括排队和服务。' },
  'disk.avg_queue_depth': { title: '平均 I/O 队列', unit: '请求', meaning: '窗口中平均在途 I/O 数。' },
  'fs.used_pct': { title: '文件系统使用率', unit: '%', meaning: '挂载文件系统已使用的块比例。' },
  'fs.inodes_used_pct': { title: 'inode 使用率', unit: '%', meaning: '文件系统 inode 使用比例，可提前识别小文件容量问题。' },
  'net.rx_bytes_per_s': { title: '网络接收速率', unit: 'B/s', meaning: '网卡接收字节速率，不含环回接口的主机合计。' },
  'net.tx_bytes_per_s': { title: '网络发送速率', unit: 'B/s', meaning: '网卡发送字节速率，不含环回接口的主机合计。' },
  'net.rx_drops_per_s': { title: '接收丢包', unit: '包/秒', meaning: '接收路径丢弃的数据包速率。' },
  'net.tx_drops_per_s': { title: '发送丢包', unit: '包/秒', meaning: '发送路径丢弃的数据包速率。' },
  'tcp.established_connections': { title: '已建立连接', unit: '个', meaning: '当前 ESTABLISHED 状态的 TCP socket 数。' },
  'tcp.active_opens_per_s': { title: '主动建连', unit: '次/秒', meaning: '主动发起 TCP 连接的速率。' },
  'tcp.retrans_ratio_pct': { title: 'TCP 重传率', unit: '%', meaning: '重传段与发送段的比例；低流量时可能缺失。' },
  'proc.cpu_pct': { title: '进程 CPU', unit: '% 单核', meaning: '进程 CPU 时间增量除以墙上时间，可超过 100%。' },
  'proc.user_processes': { title: '用户态进程数', unit: '个', meaning: '命令行非空的用户态进程数量，不包含内核线程。' },
  'proc.inventory_omitted': { title: '清单截断进程数', unit: '个', meaning: '超过当前进程清单 1024 项上限的用户态进程数量。' },
  'proc.threads': { title: '进程线程数', unit: '个', meaning: '进程当前线程数量。' },
  'proc.voluntary_ctxt_per_s': { title: '主动上下文切换', unit: '次/秒', meaning: '进程主动让出 CPU 的速率。' },
  'proc.involuntary_ctxt_per_s': { title: '被动上下文切换', unit: '次/秒', meaning: '进程被抢占或调度切换的速率。' },
  'proc.rss_bytes': { title: '进程驻留内存', unit: 'B', meaning: '进程占用的物理页大小。' },
  'proc.read_bytes_per_s': { title: '进程磁盘读取', unit: 'B/s', meaning: '归因到进程的实际存储读取速率。' },
  'proc.write_bytes_per_s': { title: '进程磁盘写入', unit: 'B/s', meaning: '归因到进程的实际存储写入速率。' },
  'proc.block_io_delay_ms_per_s': { title: '进程块 I/O 等待', unit: 'ms/秒', meaning: '进程同步块 I/O 等待时间增长率，依赖 delay accounting。' },
  'cgroup.cpu_usage_pct': { title: '工作负载 CPU', unit: '% 单核', meaning: 'cgroup CPU 消耗，可超过 100%。' },
  'cgroup.cpu_throttled_period_ratio_pct': { title: 'CPU 限流周期', unit: '%', meaning: '配额周期中被限流的比例。' },
  'cgroup.mem_current_bytes': { title: '工作负载内存', unit: 'B', meaning: 'cgroup 当前计费内存。' },
  'ebpf.sched.runqueue_latency_p95_ms': { title: '调度等待 P95', unit: 'ms', meaning: '任务可运行后到获得 CPU 的尾部等待时长。' },
  'ebpf.sched.long_waits_per_s': { title: '长调度等待', unit: '次/秒', meaning: '超过 10 ms 的调度等待事件速率。' },
  'ebpf.block.latency_p95_ms': { title: '块请求延迟 P95', unit: 'ms', meaning: '块请求提交至完成的尾部延迟。' },
  'ebpf.tcp.connect_latency_p95_ms': { title: '建连延迟 P95', unit: 'ms', meaning: '主动 TCP 建连至建立成功的尾部延迟。' },
  'ebpf.tcp.connect_failures_per_s': { title: '建连失败', unit: '次/秒', meaning: '主动连接在建立前失败的速率。' },
  'agent.buffer_batches': { title: '待确认批次', unit: '批', meaning: 'Worker 本地磁盘中尚未收到服务端 ACK 的批次数。' },
  'agent.buffer_bytes': { title: '缓存占用', unit: 'B', meaning: '未确认批次占用的本地磁盘空间。' },
  'agent.rss_bytes': { title: 'Worker 常驻内存', unit: 'B', meaning: '采集进程占用的物理内存。' }
}

// 这组补充覆盖 Worker 当前实际发送的固定指标键；名称、单位和解释集中维护，供图表、表格与 CSV 共用。
const info = (title: string, unit: string, meaning: string): MetricInfo => ({ title, unit, meaning })
Object.assign(metricInfo, {
  'host.uptime_s': info('主机运行时间', '秒', 'Linux 启动后的持续运行时间；重启后计数器可能重置。'),
  'cpu.nice_pct': info('低优先级用户态', '%', 'nice 调整过优先级的用户态任务占用 CPU 的比例。'),
  'cpu.idle_pct': info('CPU 空闲时间', '%', 'CPU 未执行任务的时间占比；需结合负载和压力判断余量。'),
  'cpu.irq_pct': info('硬中断时间', '%', '硬件中断处理占用 CPU 的时间比例。'),
  'cpu.softirq_pct': info('软中断时间', '%', '软中断处理占用 CPU 的时间比例，网络包处理升高时常见。'),
  'cpu.interrupts_per_s': info('中断速率', '次/秒', '全机每秒发生的中断数；异常升高可定位设备负载。'),
  'cpu.load5': info('5 分钟负载', '', '最近 5 分钟运行或不可中断任务的平均数。'),
  'cpu.load15': info('15 分钟负载', '', '最近 15 分钟运行或不可中断任务的平均数。'),
  'cpu.runnable_tasks': info('可运行任务', '个', '当前处于运行队列或执行中的任务数量。'),
  'mem.free_bytes': info('完全空闲内存', 'B', '未被应用或缓存使用的物理内存；单独偏低不等于内存不足。'),
  'mem.cached_bytes': info('页缓存', 'B', 'Linux 用于缓存文件页的内存，可在压力下回收。'),
  'mem.buffers_bytes': info('块缓冲', 'B', '内核块设备元数据和缓冲占用的内存。'),
  'mem.slab_reclaimable_bytes': info('可回收 Slab', 'B', '可在压力下回收的内核对象缓存。'),
  'mem.dirty_bytes': info('待写回脏页', 'B', '已经修改但尚未写入存储设备的内存页。'),
  'mem.swap_total_bytes': info('交换区容量', 'B', '系统配置的 swap 总大小。'),
  'mem.swap_in_pages_per_s': info('交换区换入', '页/秒', '每秒从 swap 读回的页数，持续出现可能拖慢响应。'),
  'mem.swap_out_pages_per_s': info('交换区换出', '页/秒', '每秒写入 swap 的页数，持续出现提示内存压力。'),
  'mem.pgscan_pages_per_s': info('内存回收扫描', '页/秒', '内核为回收内存每秒扫描的页数。'),
  'mem.pgsteal_pages_per_s': info('成功回收页', '页/秒', '内核每秒实际回收的页数，可与扫描量比较。'),
  'disk.busy_time_pct': info('磁盘繁忙时间', '%', '块设备至少有一个请求在途的时间比例。'),
  'disk.in_flight': info('当前在途 I/O', '请求', '采样瞬间仍未完成的块请求数量。'),
  'fs.size_bytes': info('文件系统容量', 'B', '挂载点总容量；按 mount 标签区分。'),
  'fs.available_bytes': info('文件系统可用', 'B', '普通用户可使用的剩余空间。'),
  'fs.inodes_total': info('inode 总量', '个', '文件系统可分配的 inode 数量。'),
  'fs.inodes_available': info('可用 inode', '个', '剩余可创建文件或目录的 inode 数量。'),
  'net.interface_up': info('接口状态', '0/1', '网络接口是否处于 UP 状态；1 为启用。'),
  'net.interface_speed_bps': info('接口链路速率', 'bit/s', '网卡报告的额定链路速度，不等于实际吞吐。'),
  'net.rx_packets_per_s': info('接收包速率', '包/秒', '网卡每秒接收的数据包数量。'),
  'net.tx_packets_per_s': info('发送包速率', '包/秒', '网卡每秒发送的数据包数量。'),
  'net.rx_errors_per_s': info('接收错误', '次/秒', '接收时出现硬件或驱动错误的速率。'),
  'net.tx_errors_per_s': info('发送错误', '次/秒', '发送时出现硬件或驱动错误的速率。'),
  'tcp.passive_opens_per_s': info('被动建连', '次/秒', '服务端接受 TCP 新连接的速率。'),
  'tcp.established_resets_per_s': info('连接复位', '次/秒', '已建立连接被复位的速率。'),
  'tcp.out_segments_per_s': info('TCP 发送段', '段/秒', 'TCP 层每秒发出的报文段数量。'),
  'tcp.retrans_segments_per_s': info('TCP 重传段', '段/秒', 'TCP 层每秒重传的报文段数量。'),
  'tcp.listen_connections': info('监听 Socket', '个', '当前处于 LISTEN 状态的 TCP socket 数。'),
  'tcp.time_wait_connections': info('TIME_WAIT Socket', '个', '当前处于 TIME_WAIT 状态的 TCP socket 数。'),
  'proc.user_cpu_pct': info('进程用户态 CPU', '% 单核', '进程在用户态消耗的 CPU 时间比例。'),
  'proc.system_cpu_pct': info('进程内核态 CPU', '% 单核', '进程在内核态消耗的 CPU 时间比例。'),
  'proc.vmsize_bytes': info('进程虚拟内存', 'B', '进程虚拟地址空间大小，不代表物理占用。'),
  'proc.threads': info('进程线程数', '个', '进程当前线程数量。'),
  'proc.major_faults_per_s': info('进程重大缺页', '次/秒', '进程每秒发生的需外部读取的缺页次数。'),
  'proc.read_syscalls_per_s': info('进程读取调用', '次/秒', '进程每秒执行的读取类系统调用次数。'),
  'proc.write_syscalls_per_s': info('进程写入调用', '次/秒', '进程每秒执行的写入类系统调用次数。'),
  'proc.voluntary_ctxt_per_s': info('主动上下文切换', '次/秒', '进程自愿让出 CPU 的切换速率，常见于等待。'),
  'proc.involuntary_ctxt_per_s': info('被动上下文切换', '次/秒', '进程被调度器抢占的切换速率。'),
  'cgroup.cpu_throttled_per_s': info('CPU 限流事件', '次/秒', 'cgroup 每秒遭遇 CPU 配额限流的周期数。'),
  'cgroup.cpu_throttled_time_ms_per_s': info('CPU 限流时长', 'ms/秒', 'cgroup 因 CPU 配额耗尽而等待的时间增长率。'),
  'cgroup.io_read_bytes_per_s': info('工作负载读取吞吐', 'B/s', 'cgroup 归因到块设备的读取字节速率。'),
  'cgroup.io_write_bytes_per_s': info('工作负载写入吞吐', 'B/s', 'cgroup 归因到块设备的写入字节速率。'),
  'cgroup.io_read_ops_per_s': info('工作负载读取 IOPS', '次/秒', 'cgroup 块读取请求速率。'),
  'cgroup.io_write_ops_per_s': info('工作负载写入 IOPS', '次/秒', 'cgroup 块写入请求速率。'),
  'cgroup.mem_high_events_per_s': info('内存高水位事件', '次/秒', 'cgroup 超过 memory.high 并受到回收压力的速率。'),
  'cgroup.oom_kills_per_s': info('工作负载 OOM', '次/秒', 'cgroup 内任务因 OOM 被结束的速率。'),
  'ebpf.block.completed_per_s': info('块请求完成', '次/秒', '内核块层完成请求的速率。'),
  'ebpf.block.mean_latency_ms': info('块请求平均延迟', 'ms', 'eBPF 记录的块请求提交至完成平均时间。'),
  'ebpf.sched.switches_per_s': info('调度切换事件', '次/秒', 'eBPF 观察到的任务切换速率。'),
  'ebpf.tcp.connect_attempts_per_s': info('TCP 建连尝试', '次/秒', 'eBPF 观察到的主动 TCP 连接尝试速率。'),
  'ebpf.tcp.connect_failure_ratio_pct': info('TCP 建连失败率', '%', '主动连接尝试中在建立前失败的比例。'),
  'ebpf.tcp.retransmits_per_s': info('内核 TCP 重传', '次/秒', 'eBPF 观察到的 TCP 重传事件速率。'),
  'agent.dropped_batches_total': info('丢弃批次累计', '批', '磁盘缓冲溢出后被删掉的历史批次总数。'),
  'agent.perf_available': info('perf 可用性', '0/1', '采集端是否检测到可执行的 perf 命令。')
})

/** PSI 指标有资源、some/full 和窗口维度，按键名生成一致的中文口径。 */
export function metricDetails(name: string): MetricInfo | undefined {
  if (metricInfo[name]) return metricInfo[name]
  const match = name.match(/^(cpu|mem|io|cgroup\.cpu|cgroup\.memory|cgroup\.io)\.psi\.(some|full)\.(avg10|avg60|avg300|total_us)$/)
  if (!match) return undefined
  const resource = ({ cpu: 'CPU', mem: '内存', io: 'I/O', 'cgroup.cpu': '工作负载 CPU', 'cgroup.memory': '工作负载内存', 'cgroup.io': '工作负载 I/O' } as Record<string, string>)[match[1]]
  const scope = match[2] === 'some' ? '至少一个任务等待' : '所有非空闲任务同时等待'
  const window = match[3] === 'total_us' ? '累计阻塞时间' : `最近 ${match[3].slice(3)} 秒压力均值`
  return info(`${resource} PSI ${match[2]}`, match[3] === 'total_us' ? 'μs' : '%', `${scope}${resource}；${window}。`)
}

export function formatValue(name: string, value: number | null | undefined, digits = 1): string {
  if (value == null || !Number.isFinite(value)) return '—'
  const unit = metricDetails(name)?.unit || ''
  if (unit === 'B' || unit === 'B/s') {
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
    let size = Math.abs(value); let index = 0
    while (size >= 1024 && index < units.length - 1) { size /= 1024; index++ }
    return `${value < 0 ? '-' : ''}${size.toFixed(index === 0 ? 0 : 1)} ${units[index]}${unit === 'B/s' ? '/s' : ''}`
  }
  return `${value.toLocaleString('zh-CN', { maximumFractionDigits: digits })}${unit ? ` ${unit}` : ''}`
}

export function metricTitle(name: string) { return metricDetails(name)?.title || name }
export function timestamp(value: number) { return new Date(value).toLocaleString('zh-CN', { hour12: false }) }
