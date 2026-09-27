<script setup lang="ts">
/**
 * 用户态进程工作台。清单是 Worker 最新的 /proc 快照，内核线程不在其中；
 * 时序只为 CPU Top 20 与用户显式监控的进程保存，避免所有 PID 都进入
 * 七天指标库。监控身份包含启动 tick，服务重启后不会误认复用的 PID。
 */
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from 'vue'
import { storeToRefs } from 'pinia'
import { Activity, Eye, EyeOff, RefreshCw, Search } from '@lucide/vue'
import { request, type Metric, type ProcessEntry, type ProcessWatch } from '../api'
import { formatValue, timestamp } from '../catalog'
import { usePlatform } from '../store'

const LineChart = defineAsyncComponent(() => import('../components/LineChart.vue'))
const platform = usePlatform()
const { hostId, range, role, selectedHost, latest } = storeToRefs(platform)
const processes = ref<ProcessEntry[]>([])
const watches = ref<ProcessWatch[]>([])
const selected = ref<ProcessEntry | null>(null)
const points = ref<Metric[]>([])
const threadPoints = ref<Metric[]>([])
const search = ref('')
const sortBy = ref<'cpu' | 'rss' | 'pid'>('cpu')
const page = ref(1)
const busy = ref(false)
const loading = ref(false)
const error = ref('')
let timer: number | undefined

const canManage = computed(() => role.value === 'operator' || role.value === 'admin')
const selectedWatch = computed(() => watches.value.find((item) => item.pid === selected.value?.pid && item.start_ticks === selected.value?.start_ticks))
const activeWatches = computed(() => watches.value.map((item) => ({
  ...item, running: processes.value.some((process) => process.pid === item.pid && process.start_ticks === item.start_ticks)
})))
const filtered = computed(() => {
  const text = search.value.trim().toLowerCase()
  const rows = processes.value.filter((item) => !text ||
    `${item.pid} ${item.ppid} ${item.uid} ${item.comm} ${item.command}`.toLowerCase().includes(text))
  return rows.sort((a, b) => sortBy.value === 'rss' ? b.rss_bytes - a.rss_bytes
    : sortBy.value === 'pid' ? a.pid - b.pid : b.cpu_pct - a.cpu_pct)
})
const pageCount = computed(() => Math.max(1, Math.ceil(filtered.value.length / 30)))
const visible = computed(() => filtered.value.slice((page.value - 1) * 30, page.value * 30))
const labelFilter = computed(() => selected.value ? { pid: String(selected.value.pid), start_ticks: String(selected.value.start_ticks) } : undefined)
const current = computed(() => {
  const values = new Map<string, Metric>()
  for (const point of points.value) {
    const before = values.get(point.name)
    if (!before || before.time_ms < point.time_ms) values.set(point.name, point)
  }
  return values
})
const threadRows = computed(() => {
  const grouped = new Map<string, { tid: string; values: Record<string, number> }>()
  for (const item of threadPoints.value) {
    const tid = item.labels.tid
    if (!tid) continue
    const row = grouped.get(tid) || { tid, values: {} }
    row.values[item.name] = item.value
    grouped.set(tid, row)
  }
  return [...grouped.values()].sort((a, b) => (b.values['thread.cpu_runtime_ms_per_s'] || 0) - (a.values['thread.cpu_runtime_ms_per_s'] || 0))
})

async function load() {
  const id = hostId.value
  if (!id) { processes.value = []; watches.value = []; return }
  loading.value = true
  try {
    const params = new URLSearchParams({ host_id: id })
    const [list, watched] = await Promise.all([
      request<ProcessEntry[]>(`/api/v1/processes?${params}`),
      request<ProcessWatch[]>(`/api/v1/process-watches?${params}`)
    ])
    if (id !== hostId.value) return
    processes.value = list
    watches.value = watched
    if (selected.value) {
      // 用启动标识判断进程是否仍是同一个实例；已退出时保留历史曲线，
      // 并由页面提示用户重新选择新 PID。
      selected.value = list.find((item) => item.pid === selected.value?.pid && item.start_ticks === selected.value?.start_ticks) || selected.value
    }
    if (!selected.value && list.length) selected.value = list[0]
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { loading.value = false }
}

async function loadSeries() {
  const id = hostId.value
  const process = selected.value
  if (!id || !process) { points.value = []; threadPoints.value = []; return }
  try {
    const { from, to, step_ms } = platform.timeWindow()
    const params = new URLSearchParams({ host_id: id, category: 'proc', from: String(from), to: String(to),
      step_ms: String(step_ms), limit: '10000', labels: JSON.stringify({ pid: String(process.pid), start_ticks: String(process.start_ticks) }) })
    const threadParams = new URLSearchParams({ host_id: id, category: 'thread', from: String(Date.now() - 60_000), to: String(Date.now()), limit: '10000', labels: JSON.stringify({ pid: String(process.pid), start_ticks: String(process.start_ticks) }) })
    const [result, threads] = await Promise.all([
      request<Metric[]>(`/api/v1/metrics?${params}`),
      request<Metric[]>(`/api/v1/metrics?${threadParams}`)
    ])
    if (id === hostId.value && process.pid === selected.value?.pid && process.start_ticks === selected.value?.start_ticks) { points.value = result; threadPoints.value = threads }
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}

async function addWatch() {
  if (!selected.value || !hostId.value || !canManage.value) return
  busy.value = true
  error.value = ''
  try {
    await request<ProcessWatch>('/api/v1/process-watches', { method: 'POST', body: JSON.stringify({
      host_id: hostId.value, pid: selected.value.pid, start_ticks: selected.value.start_ticks
    }) })
    await load()
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { busy.value = false }
}

async function removeWatch(item: ProcessWatch) {
  if (!canManage.value) return
  busy.value = true
  error.value = ''
  try {
    await request(`/api/v1/process-watches/${encodeURIComponent(item.host_id)}/${item.pid}/${item.start_ticks}`, { method: 'DELETE' })
    await load()
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { busy.value = false }
}

function selectProcess(item: ProcessEntry) { selected.value = item; points.value = []; threadPoints.value = []; void loadSeries() }
function stateTitle(state: string) { return ({ R: '运行', S: '休眠', D: 'I/O 等待', T: '停止', Z: '僵尸', I: '空闲' } as Record<string, string>)[state] || state }

watch(search, () => { page.value = 1 })
watch(sortBy, () => { page.value = 1 })
watch(hostId, () => { processes.value = []; watches.value = []; selected.value = null; points.value = []; threadPoints.value = []; page.value = 1; void load() }, { immediate: true })
watch([selected, range], () => { void loadSeries() })
onMounted(() => { timer = window.setInterval(() => { void load(); void loadSeries() }, 15_000) })
onUnmounted(() => { if (timer) window.clearInterval(timer) })
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><h2>进程监控</h2><p>查看用户态进程的 CPU、内存、磁盘 I/O 与每线程调度；Pod 共享资源请到集群页查看。</p></div><button class="button secondary" @click="load"><RefreshCw :size="15" /> 刷新清单</button></div>
    <div v-if="error" class="notice error">{{ error }}</div>
    <div v-if="!hostId" class="empty-panel">请先选择一台在线 Worker 节点。</div>
    <template v-else>
      <section class="panel"><div class="panel-header"><div><h3>用户进程</h3><p>来自 /proc 的最近 30 秒快照；排除命令行为空的内核线程。{{ processes.length }} 个进程{{ latest['proc.inventory_omitted'] ? `，另有 ${latest['proc.inventory_omitted']} 个超出清单上限` : '' }}</p></div><span class="subtle-meta">{{ loading ? '更新中…' : '每 15 秒更新' }}</span></div>
        <div class="process-toolbar"><div class="search-field"><Search :size="16" /><input v-model="search" placeholder="搜索 PID、UID、名称或命令" /></div><select v-model="sortBy" aria-label="进程排序"><option value="cpu">按 CPU 排序</option><option value="rss">按内存排序</option><option value="pid">按 PID 排序</option></select></div>
        <div class="table-scroll"><table class="data-table"><thead><tr><th>PID</th><th>UID</th><th>进程 / 命令</th><th>状态</th><th class="align-right">CPU</th><th class="align-right">RSS</th><th></th></tr></thead><tbody>
          <tr v-for="item in visible" :key="`${item.pid}-${item.start_ticks}`" :class="{ 'selected-row': selected?.pid === item.pid && selected?.start_ticks === item.start_ticks }"><td class="mono">{{ item.pid }}<small>父 {{ item.ppid }}</small></td><td class="mono">{{ item.uid }}</td><td class="process-command"><strong>{{ item.comm }}</strong><small :title="item.command">{{ item.command }}</small></td><td>{{ stateTitle(item.state) }}</td><td class="align-right">{{ formatValue('proc.cpu_pct', item.cpu_pct) }}</td><td class="align-right">{{ formatValue('proc.rss_bytes', item.rss_bytes) }}</td><td><button class="button text" @click="selectProcess(item)">查看</button></td></tr>
          <tr v-if="!visible.length"><td colspan="7" class="table-empty">{{ loading ? '正在读取进程快照…' : '没有匹配的用户进程' }}</td></tr>
        </tbody></table></div><div class="table-pagination"><span>第 {{ page }} / {{ pageCount }} 页</span><div><button class="button text" :disabled="page <= 1" @click="page--">上一页</button><button class="button text" :disabled="page >= pageCount" @click="page++">下一页</button></div></div>
      </section>

      <section v-if="selected" class="panel"><div class="panel-header"><div><h3>{{ selected.comm }} · PID {{ selected.pid }}</h3><p>{{ selected.command }} · UID {{ selected.uid }} · {{ stateTitle(selected.state) }} · 最近发现 {{ timestamp(selected.last_seen_ms) }}</p></div><button v-if="canManage && !selectedWatch" class="button primary" :disabled="busy || !selectedHost?.online || !processes.some((item) => item.pid === selected?.pid && item.start_ticks === selected?.start_ticks)" @click="addWatch"><Eye :size="15" /> 固定监控</button><button v-else-if="canManage && selectedWatch" class="button secondary" :disabled="busy" @click="removeWatch(selectedWatch)"><EyeOff :size="15" /> 取消监控</button></div>
        <div v-if="!processes.some((item) => item.pid === selected?.pid && item.start_ticks === selected?.start_ticks)" class="inline-info">该进程已退出或 PID 已被复用。历史曲线仍可查看，请选择新的进程实例继续监控。</div>
        <div v-else-if="!selectedWatch" class="inline-info">未固定监控时，仅 CPU Top 20 进程有详细时序；固定后约一个明细采样周期开始产生持续数据。</div>
        <div class="process-kpis"><div><span>CPU</span><strong>{{ formatValue('proc.cpu_pct', current.get('proc.cpu_pct')?.value ?? selected.cpu_pct) }}</strong></div><div><span>驻留内存</span><strong>{{ formatValue('proc.rss_bytes', current.get('proc.rss_bytes')?.value ?? selected.rss_bytes) }}</strong></div><div><span>读取</span><strong>{{ formatValue('proc.read_bytes_per_s', current.get('proc.read_bytes_per_s')?.value) }}</strong></div><div><span>写入</span><strong>{{ formatValue('proc.write_bytes_per_s', current.get('proc.write_bytes_per_s')?.value) }}</strong></div><div><span>线程</span><strong>{{ formatValue('proc.threads', current.get('proc.threads')?.value) }}</strong></div></div>
        <div class="process-kpis"><div><span>共享内存分摊 PSS</span><strong>{{ formatValue('proc.pss_bytes', current.get('proc.pss_bytes')?.value) }}</strong></div><div><span>私有脏页</span><strong>{{ formatValue('proc.private_dirty_bytes', current.get('proc.private_dirty_bytes')?.value) }}</strong></div><div><span>打开的 FD</span><strong>{{ formatValue('proc.open_fds', current.get('proc.open_fds')?.value) }}</strong></div><div><span>虚拟栈空间</span><strong>{{ formatValue('proc.stack_virtual_bytes', current.get('proc.stack_virtual_bytes')?.value) }}</strong></div><div><span>块 I/O 等待</span><strong>{{ formatValue('proc.block_io_delay_ms_per_s', current.get('proc.block_io_delay_ms_per_s')?.value) }}</strong></div></div>
        <div class="process-kpis"><div><span>占整机 CPU</span><strong>{{ formatValue('proc.cpu_host_pct', current.get('proc.cpu_host_pct')?.value) }}</strong></div><div><span>PSS 占整机内存</span><strong>{{ formatValue('proc.pss_host_pct', current.get('proc.pss_host_pct')?.value) }}</strong></div><div><span>Socket 发送（部分）</span><strong>{{ formatValue('proc.socket_tx_bytes_per_s', current.get('proc.socket_tx_bytes_per_s')?.value) }}</strong></div><div><span>Socket 接收（部分）</span><strong>{{ formatValue('proc.socket_rx_bytes_per_s', current.get('proc.socket_rx_bytes_per_s')?.value) }}</strong></div></div>
        <div class="inline-info">PSS、FD 与线程明细只为固定监控进程采集。虚拟栈空间不是实际栈使用量；线程的运行队列等待也不是上下文切换指令本身的开销。</div>
        <div v-if="selectedWatch" class="inline-info">进程 Socket 字节来自 sendto/sendmsg/recvfrom/recvmsg 的成功返回值，是部分应用数据；write、sendfile 与零拷贝等路径尚未计入。Pod 网卡总量请在集群页查看。</div>
      </section>
      <div v-if="selected" class="process-chart-grid"><section class="panel"><div class="panel-header"><div><h3>进程 CPU</h3><p>占单核百分比，可超过 100%</p></div><Activity :size="18" /></div><LineChart :points="points" :names="['proc.cpu_pct']" :label-filter="labelFilter" :height="240" /></section><section class="panel"><div class="panel-header"><div><h3>驻留内存</h3><p>实际占用的物理内存</p></div></div><LineChart :points="points" :names="['proc.rss_bytes']" :label-filter="labelFilter" :height="240" /></section><section class="panel"><div class="panel-header"><div><h3>进程磁盘 I/O</h3><p>读写字节速率</p></div></div><LineChart :points="points" :names="['proc.read_bytes_per_s','proc.write_bytes_per_s']" :label-filter="labelFilter" :height="240" /></section><section class="panel"><div class="panel-header"><div><h3>上下文切换</h3><p>主动让出与被动抢占速率</p></div></div><LineChart :points="points" :names="['proc.voluntary_ctxt_per_s','proc.involuntary_ctxt_per_s']" :label-filter="labelFilter" :height="240" /></section></div>
      <section v-if="selected && selectedWatch" class="panel"><div class="panel-header"><div><h3>线程调度明细</h3><p>最近 60 秒，每个 TID 的最新速率。运行队列等待反映争用，不能作为上下文切换本身的耗时。</p></div></div><div class="table-scroll"><table class="data-table"><thead><tr><th>线程 TID</th><th class="align-right">CPU 时间</th><th class="align-right">运行队列等待</th><th class="align-right">主动切换</th><th class="align-right">被动切换</th><th class="align-right">时间片</th></tr></thead><tbody><tr v-for="thread in threadRows" :key="thread.tid"><td class="mono">{{ thread.tid }}</td><td class="align-right">{{ formatValue('thread.cpu_runtime_ms_per_s', thread.values['thread.cpu_runtime_ms_per_s']) }}</td><td class="align-right">{{ formatValue('thread.runqueue_wait_ms_per_s', thread.values['thread.runqueue_wait_ms_per_s']) }}</td><td class="align-right">{{ formatValue('thread.voluntary_ctxt_per_s', thread.values['thread.voluntary_ctxt_per_s']) }}</td><td class="align-right">{{ formatValue('thread.involuntary_ctxt_per_s', thread.values['thread.involuntary_ctxt_per_s']) }}</td><td class="align-right">{{ formatValue('thread.slices_per_s', thread.values['thread.slices_per_s']) }}</td></tr><tr v-if="!threadRows.length"><td colspan="6" class="table-empty">等待线程采样，或内核未提供 schedstat</td></tr></tbody></table></div></section>
      <section class="panel"><div class="panel-header"><div><h3>固定监控</h3><p>每个节点最多 20 个。服务重启后 PID 身份改变，页面会标记已退出。</p></div></div><div v-if="!activeWatches.length" class="quiet-state">尚未固定监控进程</div><div v-for="item in activeWatches" :key="`${item.pid}-${item.start_ticks}`" class="event-row"><span class="status-dot" :class="{ offline: !item.running }"></span><div><strong>{{ item.name }} · PID {{ item.pid }}</strong><small>{{ item.running ? '正在监控' : '进程已退出' }} · 创建于 {{ timestamp(item.created_ms) }}</small></div><button v-if="item.running" class="button text" :disabled="busy" @click="selected = processes.find((process) => process.pid === item.pid && process.start_ticks === item.start_ticks) || selected">查看</button><button v-if="canManage" class="button text" :disabled="busy" @click="removeWatch(item)">移除</button></div></section>
    </template>
  </div>
</template>
