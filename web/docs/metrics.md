# 指标字典

本文件是 Linux-Pilot 的指标口径参考。**实际已上报的指标请以 Worker 代码和前端目录为准**，候选项不会伪装成已交付能力。


下表列出Linux-Pilot 指标契约涉及的指标及其用途，**其中包含未实现的候选项**。M1 表示基础采集范围，M2 表示内核与 perf 深度观测范围，P1 表示下一阶段候选；阶段标记不代表已经实现。实际已发出的指标名以 Worker 代码和前端指标字典为准。百分比通常为 0～100，进程单核 CPU 等明确例外；速率由相邻累计计数器之差除以采样时长得到，延迟分位数按窗口直方图计算。

**PSI 统一解释**：`some` 表示窗口中至少一个任务被该资源阻塞的时间比例，`full` 表示所有非空闲任务都被阻塞的比例；`avg10/avg60/avg300` 是内核给出的 10/60/300 秒均值，`total_us` 是累计阻塞微秒数，可用于自定义窗口。系统级 CPU 的 `full` 无有效含义，不采集。下列 `*.psi.*` 指标均保留这四种读数，而评分默认使用 `avg10` 与窗口 `total_us` 差值。[内核 PSI 说明](https://www.kernel.org/doc/html/latest/accounting/psi.html)

#### A. 主机 CPU 与调度（M1）

| 指标（单位） | 意义 / 诊断用途 | 来源 |
| --- | --- | --- |
| `host.uptime_s`（秒） | 主机持续运行时间；解释重启后计数器重置与短时波动。 | `/proc/uptime` |
| `cpu.logical_cores`（个） | 在线逻辑 CPU 数；用于解释总负载和归一化运行队列。 | `/sys/devices/system/cpu/online` |
| `cpu.busy_pct`（%） | user + nice + system + irq + softirq 的时间占比，不含 idle、iowait、steal；显示实际计算资源使用情况，不单独等同于瓶颈。 | `/proc/stat` 差分 |
| `cpu.user_pct`（%） | 普通优先级用户态工作占比；观察应用计算消耗。 | `/proc/stat` |
| `cpu.nice_pct`（%） | 低优先级用户态工作占比；判断后台任务占用。 | `/proc/stat` |
| `cpu.system_pct`（%） | 内核态工作占比；过高时检查系统调用、网络与文件系统路径。 | `/proc/stat` |
| `cpu.idle_pct`（%） | CPU 空闲时间占比；与运行队列和 PSI 联合判断容量。 | `/proc/stat` |
| `cpu.iowait_pct`（%） | CPU 空闲且有待完成 I/O 的时间占比；只是线索，**不是设备延迟或进程等待时长**。 | `/proc/stat` |
| `cpu.irq_pct`（%） | 硬中断处理占比；定位设备中断负载。 | `/proc/stat` |
| `cpu.softirq_pct`（%） | 软中断处理占比；常用于排查网络包处理开销。 | `/proc/stat` |
| `cpu.steal_pct`（%） | 虚拟机被宿主机夺走的 CPU 时间占比；高值提示虚拟化层竞争。 | `/proc/stat` |
| `cpu.load1`、`load5`、`load15`（负载值） | 过去 1/5/15 分钟运行或不可中断任务的平均数量；与核心数比较，不能当作 CPU 使用率。 | `/proc/loadavg` |
| `cpu.runnable_tasks`（个） | 当前可运行任务数；持续高于可用核心数提示排队。 | `/proc/loadavg` |
| `cpu.context_switches_per_s`（次/秒） | 全机上下文切换速率；异常上升可能是线程竞争或频繁唤醒。 | `/proc/stat` 的 `ctxt` |
| `cpu.interrupts_per_s`（次/秒） | 全机中断速率；定位异常硬件或高包速率。 | `/proc/stat` 的 `intr` |
| `cpu.psi.some.{avg10,avg60,avg300,total_us}`（% / μs） | 任务等待 CPU 的时间压力；比“CPU 很忙”更直接反映排队。 | `/proc/pressure/cpu` |

以上 CPU 时间占比同时提供主机总计和每逻辑核心维度；每核心是同一组指标的 `cpu_id` 维度，不另造一套指标名。CPU 的 `user` 已包含 guest 相关时间，计算 busy 时不可重复累加 guest。[proc_stat 说明](https://man7.org/linux/man-pages/man5/proc_stat.5.html)

#### B. 主机内存（M1）

| 指标（单位） | 意义 / 诊断用途 | 来源 |
| --- | --- | --- |
| `mem.total_bytes`（B） | 可供系统使用的物理内存总量；计算容量比例的分母。 | `/proc/meminfo` |
| `mem.available_bytes`（B） | 估计无需严重交换即可分配的内存；判断内存余量的首选值。 | `/proc/meminfo` 的 `MemAvailable` |
| `mem.used_pct`（%） | `(MemTotal - MemAvailable) / MemTotal`；展示当前有效内存压力，需结合 PSI 判断。 | 派生 |
| `mem.free_bytes`（B） | 完全未使用的内存；单独低值不代表内存不足。 | `/proc/meminfo` |
| `mem.cached_bytes`（B） | 页缓存大小；解释“已使用但可回收”的内存。 | `/proc/meminfo` |
| `mem.buffers_bytes`（B） | 块设备元数据缓冲；帮助解释缓存构成。 | `/proc/meminfo` |
| `mem.slab_reclaimable_bytes`（B） | 可回收内核 slab；持续异常增长可排查内核缓存。 | `/proc/meminfo` 的 `SReclaimable` |
| `mem.dirty_bytes`（B） | 已修改、尚待写回的页；持续偏高提示写回压力。 | `/proc/meminfo` |
| `mem.swap_total_bytes`（B） | 已配置交换空间总量。 | `/proc/meminfo` |
| `mem.swap_used_bytes`（B） | 已使用交换空间；持续增长通常需要结合换入/换出速率看。 | `SwapTotal - SwapFree` |
| `mem.swap_in_pages_per_s`（页/秒） | 从交换区读回的页数；频繁换入可能拖慢响应。 | `/proc/vmstat` 的 `pswpin` |
| `mem.swap_out_pages_per_s`（页/秒） | 写入交换区的页数；持续发生说明内存回收压力。 | `/proc/vmstat` 的 `pswpout` |
| `mem.major_faults_per_s`（次/秒） | 需要磁盘读入等外部获取的缺页速率；定位冷数据或内存不足。 | `/proc/vmstat` 的 `pgmajfault` |
| `mem.pgscan_pages_per_s`（页/秒） | 内核扫描待回收页的速率；衡量回收工作量。 | `/proc/vmstat` 中互不重叠的 `pgscan_*` 字段求和 |
| `mem.pgsteal_pages_per_s`（页/秒） | 成功回收页的速率；与扫描量比较评估回收效率。 | `/proc/vmstat` 中互不重叠的 `pgsteal_*` 字段求和 |
| `mem.oom_kills_per_s`（次/秒） | OOM killer 结束任务的速率；严重容量故障信号。 | `/proc/vmstat` 的 `oom_kill`，若无则标记缺失 |
| `mem.psi.some.{avg10,avg60,avg300,total_us}`（% / μs） | 至少一个任务因内存受阻的时间比例；反映回收/分配阻塞。 | `/proc/pressure/memory` |
| `mem.psi.full.{avg10,avg60,avg300,total_us}`（% / μs） | 所有非空闲任务同时因内存受阻的时间比例；高值提示严重抖动。 | 同上 |

#### C. 块设备、文件系统与主机 I/O（M1）

| 指标（单位） | 意义 / 诊断用途 | 来源 |
| --- | --- | --- |
| `disk.read_bytes_per_s`（B/秒） | 每个块设备读吞吐；查看读压力与容量规划。 | `/proc/diskstats` 扇区差分 |
| `disk.write_bytes_per_s`（B/秒） | 每个块设备写吞吐；查看写压力。 | 同上 |
| `disk.read_ops_per_s`（次/秒） | 完成的读请求速率，即读 IOPS。 | `/proc/diskstats` |
| `disk.write_ops_per_s`（次/秒） | 完成的写请求速率，即写 IOPS。 | `/proc/diskstats` |
| `disk.read_await_ms`（ms） | 采样窗口内完成读请求的平均历时；包括排队及服务时间。 | 读耗时增量 / 读完成数增量 |
| `disk.write_await_ms`（ms） | 采样窗口内完成写请求的平均历时；包括排队及服务时间。 | 写耗时增量 / 写完成数增量 |
| `disk.in_flight`（个） | 采样时刻正在处理的 I/O 请求数量；查看瞬时队列压力。 | `/proc/diskstats` |
| `disk.avg_queue_depth`（个） | 窗口内平均在途 I/O 数；持续增长需结合延迟判断拥塞。 | 加权 I/O 时间增量 / 窗口时长 |
| `disk.busy_time_pct`（%） | 至少存在请求的时间比例估计；多队列设备上**不等同于真实设备利用率**。 | I/O busy 时间增量 / 窗口时长 |
| `io.psi.some.{avg10,avg60,avg300,total_us}`（% / μs） | 至少一个任务因 I/O 阻塞的时间比例；从工作负载视角看等待。 | `/proc/pressure/io` |
| `io.psi.full.{avg10,avg60,avg300,total_us}`（% / μs） | 所有非空闲任务同时因 I/O 阻塞的比例；高值说明全机生产能力受损。 | 同上 |
| `fs.size_bytes`（B） | 挂载文件系统总容量；容量告警分母。 | `statvfs` |
| `fs.available_bytes`（B） | 普通用户可用容量；比“总空闲”更贴近应用能否写入。 | `statvfs` 的 `f_bavail` |
| `fs.used_pct`（%） | 文件系统已使用块比例；预测空间耗尽。 | `statvfs` 派生 |
| `fs.inodes_total`（个） | 可用 inode 总数；检测小文件场景上限。 | `statvfs` |
| `fs.inodes_available`（个） | 尚可分配的 inode；即使容量未满也可能因 inode 耗尽无法建文件。 | `statvfs` |
| `fs.inodes_used_pct`（%） | inode 使用比例；便于阈值告警。 | `statvfs` 派生 |

磁盘指标按设备区分，默认排除 loop/ram 等虚拟设备并允许配置；文件系统按挂载点区分，去重 bind mount 与临时文件系统。零完成请求时 `await` 为缺失而非 0。[内核磁盘统计字段说明](https://www.kernel.org/doc/html/latest/admin-guide/iostats.html)

#### D. 网络与 TCP（M1）

| 指标（单位） | 意义 / 诊断用途 | 来源 |
| --- | --- | --- |
| `net.rx_bytes_per_s`（B/秒） | 网卡接收吞吐；判断入口流量。 | `/proc/net/dev`，按网卡 |
| `net.tx_bytes_per_s`（B/秒） | 网卡发送吞吐；判断出口流量。 | 同上 |
| `net.rx_packets_per_s`（包/秒） | 接收包速率；高包速率可导致 CPU/软中断压力。 | 同上 |
| `net.tx_packets_per_s`（包/秒） | 发送包速率；分析包量和带宽差异。 | 同上 |
| `net.rx_drops_per_s`（包/秒） | 接收路径丢弃；可能是队列、驱动或缓冲压力。 | 同上 |
| `net.tx_drops_per_s`（包/秒） | 发送路径丢弃；可能是队列或链路异常。 | 同上 |
| `net.rx_errors_per_s`（次/秒） | 接收错误；提示链路或驱动问题。 | 同上 |
| `net.tx_errors_per_s`（次/秒） | 发送错误；提示链路或驱动问题。 | 同上 |
| `net.interface_up`（0/1） | 网卡是否处于可用状态；区分“无流量”与“接口离线”。 | `/sys/class/net/*/operstate` |
| `net.interface_speed_bps`（bit/秒） | 接口报告的链路速率；可计算带宽占用，虚拟网卡可能未知。 | `/sys/class/net/*/speed` |
| `tcp.established_connections`（个） | 当前 ESTABLISHED 连接数；观察并发连接负载。 | sock_diag/netlink，必要时退化到 `/proc/net/tcp{,6}` |
| `tcp.time_wait_connections`（个） | 当前 TIME_WAIT 连接数；异常增多可提示短连接激增。 | 同上 |
| `tcp.listen_connections`（个） | 当前监听 socket 数；辅助发现服务异常退出。 | 同上 |
| `tcp.retrans_segments_per_s`（段/秒） | TCP 重传段速率；提示丢包、拥塞或对端接收能力问题。 | `/proc/net/snmp` 的 `RetransSegs` |
| `tcp.out_segments_per_s`（段/秒） | 发送 TCP 段速率；重传率分母。 | `/proc/net/snmp` 的 `OutSegs` |
| `tcp.retrans_ratio_pct`（%） | 重传段 / 发送段；不同流量规模下更便于比较，低流量时标记“不足以计算”。 | 派生 |
| `tcp.active_opens_per_s`（次/秒） | 主动发起 TCP 连接的速率；识别连接风暴。 | `/proc/net/snmp` |
| `tcp.passive_opens_per_s`（次/秒） | 被动接受 TCP 连接的速率；反映服务端新连接负载。 | `/proc/net/snmp` |
| `tcp.established_resets_per_s`（次/秒） | 已建立连接被重置的速率；辅助定位网络或应用故障。 | `/proc/net/snmp` 的 `EstabResets` |

网卡指标保留物理/虚拟接口类型，默认隐藏 lo、veth 等高基数接口，但允许查询。全机 TCP 计数器与 eBPF 的按目标/进程数据口径不同，分别命名且不直接相加。[内核 TCP socket 接口说明](https://docs.kernel.org/networking/proc_net_tcp.html)

#### E. 进程与 cgroup v2（M1）

进程指标只保留 CPU、内存或 I/O 排名前 N 的进程；进程身份使用 `pid + start_time`，避免 PID 复用串联历史。读取权限不足时只返回可访问项和缺失原因。cgroup 指标默认采集配置选定的顶层工作负载及 top N 子组，避免容器标签无限增长。

| 指标（单位） | 意义 / 诊断用途 | 来源 |
| --- | --- | --- |
| `proc.cpu_pct`（% 单核） | 进程 CPU 时间增量 / 墙上时间；可超过 100%，用于找 CPU 热点。 | `/proc/<pid>/stat` 的 utime + stime |
| `proc.user_cpu_pct`（% 单核） | 进程用户态 CPU 消耗；定位应用计算。 | 同上 |
| `proc.system_cpu_pct`（% 单核） | 进程内核态 CPU 消耗；定位系统调用开销。 | 同上 |
| `proc.rss_bytes`（B） | 进程实际驻留物理页大小；找内存大户。 | `/proc/<pid>/status` 的 VmRSS |
| `proc.vmsize_bytes`（B） | 虚拟地址空间大小；与 RSS 分开看，不能当成实际内存占用。 | `/proc/<pid>/status` 的 VmSize |
| `proc.threads`（个） | 进程线程数；发现线程膨胀。 | `/proc/<pid>/status` |
| `proc.major_faults_per_s`（次/秒） | 进程需外部读入的缺页速率；帮助定位个别应用内存压力。 | `/proc/<pid>/stat` |
| `proc.block_io_delay_ms_per_s`（ms/秒） | 进程等待同步块 I/O 的累计时间增长；直接观察应用 I/O 等待，需内核启用 delay accounting。 | `/proc/<pid>/stat` 的 `delayacct_blkio_ticks`，不可用则缺失 |
| `proc.read_bytes_per_s`（B/秒） | 归因到进程的实际存储读取速率；不同于 read 系统调用返回字节数。 | `/proc/<pid>/io` 的 `read_bytes` |
| `proc.write_bytes_per_s`（B/秒） | 归因到进程的实际存储写入速率；可能与写回时点不同。 | `/proc/<pid>/io` 的 `write_bytes` |
| `proc.read_syscalls_per_s`（次/秒） | 读类系统调用速率；识别小块频繁读取。 | `/proc/<pid>/io` 的 `syscr` |
| `proc.write_syscalls_per_s`（次/秒） | 写类系统调用速率；识别小块频繁写入。 | `/proc/<pid>/io` 的 `syscw` |
| `proc.voluntary_ctxt_per_s`（次/秒） | 进程主动让出 CPU 的频率；可能在等锁、I/O 或睡眠。 | `/proc/<pid>/status` |
| `proc.involuntary_ctxt_per_s`（次/秒） | 进程被调度器抢占的频率；辅助判断 CPU 竞争。 | 同上 |
| `cgroup.cpu_usage_pct`（% 单核） | 工作负载 CPU 消耗；可超过 100%，用于容器资源归因。 | `cpu.stat:usage_usec` |
| `cgroup.cpu_quota_cores`（核） | cgroup CPU 配额折算核心数；解释容器为何被限流。 | `cpu.max` |
| `cgroup.cpu_throttled_per_s`（次/秒） | CPU 配额被触发的周期速率；高值提示受限。 | `cpu.stat:nr_throttled` |
| `cgroup.cpu_throttled_period_ratio_pct`（%） | 被限流周期 / 总周期；衡量 CPU 配额触发频率。 | `cpu.stat:nr_throttled/nr_periods` 差分 |
| `cgroup.cpu_throttled_time_ms_per_s`（ms/秒） | 被限流时间增长率；观察限流严重度，不强行限制在 100%。 | `cpu.stat:throttled_usec` 差分 |
| `cgroup.mem_current_bytes`（B） | 工作负载当前计费内存；定位容器内存占用。 | `memory.current` |
| `cgroup.mem_limit_bytes`（B） | 工作负载内存上限；`max` 表示未设置硬限制。 | `memory.max` |
| `cgroup.mem_high_events_per_s`（次/秒） | 超过 soft 上限后触发回收/节流的次数；提示接近限额。 | `memory.events:high` |
| `cgroup.oom_kills_per_s`（次/秒） | 工作负载发生 OOM kill 的速率；判断容器级故障。 | `memory.events:oom_kill` |
| `cgroup.io_read_bytes_per_s`（B/秒） | 工作负载存储读吞吐；按设备归因。 | `io.stat:rbytes` |
| `cgroup.io_write_bytes_per_s`（B/秒） | 工作负载存储写吞吐；按设备归因。 | `io.stat:wbytes` |
| `cgroup.io_read_ops_per_s`（次/秒） | 工作负载读 IOPS；按设备归因。 | `io.stat:rios` |
| `cgroup.io_write_ops_per_s`（次/秒） | 工作负载写 IOPS；按设备归因。 | `io.stat:wios` |
| `cgroup.psi.cpu.some.{avg10,avg60,avg300,total_us}`（% / μs） | 工作负载等待 CPU 的压力；辨别全机正常但个别容器受限的情况。 | cgroup v2 的 `cpu.pressure` |
| `cgroup.psi.memory.{some,full}.{avg10,avg60,avg300,total_us}`（% / μs） | 工作负载部分/全部任务等待内存的压力。 | cgroup v2 的 `memory.pressure` |
| `cgroup.psi.io.{some,full}.{avg10,avg60,avg300,total_us}`（% / μs） | 工作负载部分/全部任务等待 I/O 的压力。 | cgroup v2 的 `io.pressure` |

[Linux cgroup v2 统计说明](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html)；[进程 I/O 字段说明](https://man7.org/linux/man-pages/man5/proc_pid_io.5.html)。进程 `read_bytes/write_bytes` 与网卡字节数、系统调用字节数均是不同口径。

#### F. eBPF 深度指标（M2，能力探测后启用）

下列延迟指标用 `{p50,p95,p99}` 三个分位数和同名前缀的 `_count` 样本数呈现（例如 `ebpf.block.read_latency_count`）；没有样本时分位数为缺失。网络按有界目标分组，块 I/O 按设备及读/写方向，调度按主机及 top N 进程聚合。各探针实际挂载点按目标内核可用 tracepoint/BTF 验证，不能假定所有发行版完全相同。

| 指标（单位） | 意义 / 诊断用途 | 观测方式 |
| --- | --- | --- |
| `ebpf.tcp.retransmits_per_s`（次/秒） | 按目标/进程归因的重传事件速率；定位哪些流受影响。 | TCP 重传内核事件 |
| `ebpf.tcp.connect_latency_{p50,p95,p99}_ms`（ms） | 主动建连到建立成功的延迟分布；发现慢连接。 | TCP 状态转换关联 |
| `ebpf.tcp.connect_failures_per_s`（次/秒） | 建连失败速率；区分慢与失败。 | TCP 状态转换/连接结果 |
| `ebpf.tcp.connect_attempts_per_s`（次/秒） | 建连尝试总量；失败率分母。 | 同上 |
| `ebpf.tcp.connect_failure_ratio_pct`（%） | 失败尝试 / 总尝试；避免只看绝对失败数。 | 派生 |
| `ebpf.socket.send_latency_{p50,p95,p99}_ms`（ms） | 发送系统调用历时；识别应用在发送路径等待，**不是链路 RTT**。 | socket 发送调用入口/返回关联 |
| `ebpf.socket.recv_latency_{p50,p95,p99}_ms`（ms） | 接收系统调用历时；解释应用读等待，**不是服务端响应时间**。 | socket 接收调用入口/返回关联 |
| `ebpf.block.read_latency_{p50,p95,p99}_ms`（ms） | 块请求提交至完成的读延迟分布；识别尾延迟。 | 块请求 tracepoint 关联 |
| `ebpf.block.write_latency_{p50,p95,p99}_ms`（ms） | 块请求提交至完成的写延迟分布；识别写入尾延迟。 | 同上 |
| `ebpf.block.slow_reads_per_s`（次/秒） | 超过可配置阈值的读请求速率；便于告警。 | 读延迟直方图/事件 |
| `ebpf.block.slow_writes_per_s`（次/秒） | 超过可配置阈值的写请求速率。 | 写延迟直方图/事件 |
| `ebpf.block.errors_per_s`（次/秒） | 驱动报告失败的块请求速率；需要与设备日志联合定位。 | 块请求错误事件 |
| `ebpf.sched.runqueue_latency_{p50,p95,p99}_ms`（ms） | 任务变为可运行到真正获得 CPU 的等待时长；识别 CPU 排队。 | `sched_wakeup` / `sched_switch` 关联 |
| `ebpf.sched.long_waits_per_s`（次/秒） | 超过 10 ms 的调度等待速率；突出受影响进程。 | 调度等待直方图 |
| `ebpf.sched.switches_per_s`（次/秒） | 任务切换事件速率；与 `cpu.context_switches_per_s` 交叉验证口径。 | `sched_switch` |
| `ebpf.events_lost_per_s`（次/秒） | BPF map/RingBuf 容量不足或采样丢失的事件速率；决定指标可信度。 | agent 探针运行状态 |

建连失败、发送/接收调用延迟的归因依赖具体探针可用性；不满足时分别标记不可用。发送/接收调用可因应用等待、调度和缓冲区状态变慢，不能据此直接断言网络链路故障。[内核事件追踪说明](https://www.kernel.org/doc/html/latest/trace/events.html)

#### G. 按需 perf 输出（M2，不作持续采样）

| 字段（单位） | 意义 / 诊断用途 | 来源 |
| --- | --- | --- |
| `perf.duration_s`（秒） | 实际采样持续时间；判断剖析覆盖窗口。 | perf 任务元数据 |
| `perf.requested_frequency_hz`（Hz） | 请求的采样频率；估计开销和目标分辨率，实际样本量见 `perf.samples`。 | perf 任务元数据 |
| `perf.samples`（个） | 有效样本总数；低样本量的热点结论不可靠。 | perf record/report |
| `perf.lost_samples`（个） | 丢失样本数；判断剖析是否受缓冲或负载影响。 | perf 输出 |
| `perf.hotspot.sample_pct`（%） | 函数或调用栈占总样本比例；定位 CPU 热点，不等同精确耗时。 | 聚合调用栈 |
| `perf.symbolized_pct`（%） | 成功解析函数名的样本比例；低值说明符号缺失、火焰图解释受限。 | 符号化结果 |

#### H. Agent 自身质量指标（M1/M2）

| 指标（单位） | 意义 / 诊断用途 | 来源 |
| --- | --- | --- |
| `agent.cpu_pct`（% 单核） | agent 自身 CPU 开销；监测采集是否扰动业务。 | agent 自采集 |
| `agent.rss_bytes`（B） | agent 自身常驻内存；发现泄漏或 map/缓冲过大。 | agent 自采集 |
| `agent.buffer_batches`（个） | 未确认批次数；持续增长说明连接或服务端写入跟不上。 | 本地缓冲 |
| `agent.buffer_bytes`（B） | 本地缓冲占用；预测磁盘缓冲耗尽。 | 本地缓冲 |
| `agent.dropped_batches_per_s`（批/秒） | 缓冲满后丢弃的批次速率；数据完整性信号。 | 本地缓冲 |
| `agent.collect_errors_per_s`（次/秒） | 读取系统接口或解析失败速率；定位权限/兼容性问题。 | 各采集器 |
| `agent.probe_status`（状态） | 每个 eBPF 探针启用、加载失败或不支持；解释缺失指标。 | eBPF 管理器 |
| `agent.perf_available`（0/1） | perf 命令与权限是否可用；控制前端任务入口。 | 启动/周期能力检测 |
| `agent.clock_offset_ms`（ms） | agent 与服务端估计时钟偏差；决定时间戳是否可信。 | 心跳往返估计 |
| `agent.last_success_age_s`（秒） | 距上次成功上报的时间；区分主机离线与单项采集失败。 | 服务端接入状态 |

#### I. 下一阶段候选（P1，不计入首版必达）

| 指标（单位） | 意义 / 诊断用途 | 预计来源 |
| --- | --- | --- |
| `ebpf.file.read_latency_{p50,p95,p99}_ms`（ms） | 应用文件读取等待分布；定位应用 I/O 瓶颈。 | VFS/文件系统探针 |
| `ebpf.file.write_latency_{p50,p95,p99}_ms`（ms） | 应用文件写入等待分布；区分系统调用耗时与块设备耗时。 | VFS/文件系统探针 |
| `ebpf.file.slow_ops_per_s`（次/秒） | 慢文件操作数；快速定位异常进程。 | 文件操作探针 |
| `ebpf.mem.major_fault_latency_{p50,p95,p99}_ms`（ms） | 进程缺页处理耗时；识别读取/回收导致的停顿。 | 缺页探针 |
| `ebpf.mem.reclaim_latency_{p50,p95,p99}_ms`（ms） | 内存直接回收耗时；识别内存压力尾延迟。 | 回收探针 |
| `perf.cpu_cycles`、`perf.instructions`（次） | 硬件周期与指令数；计算 IPC 辅助判断流水线效率，受 PMU 支持限制。 | perf stat |
| `perf.cache_misses`（次） | 缓存未命中；辅助分析内存访问瓶颈，须结合总访问量和硬件口径。 | perf stat |
| `perf.off_cpu_time_ms`（ms） | 进程不在 CPU 上的等待时间；定位锁、I/O 等非 CPU 热点。 | perf/eBPF 组合剖析 |

**指标使用约束**：静态容量、状态与累计计数器在协议中保留原始类型；速率和比例明确记录窗口长度。评分只使用经过质量校验的指标，不把缺失值当 0。所有分位数必须附带 `count`；跨设备/主机的分位数不能直接取平均。主机级与 cgroup 级、网卡级与 socket 级口径不混加。
