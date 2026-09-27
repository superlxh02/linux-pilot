<script setup lang="ts">
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from 'vue'
import { storeToRefs } from 'pinia'
import { ArrowRight, CircleAlert, Cpu, Database, HardDrive, Network, Server } from '@lucide/vue'
import { RouterLink } from 'vue-router'
import { request, type AlertEvent, type Metric } from '../api'
import { formatValue, metricTitle, timestamp } from '../catalog'
import { usePlatform } from '../store'

const LineChart = defineAsyncComponent(() => import('../components/LineChart.vue'))
const platform = usePlatform()
const { hostId, range, latest, scores, hosts, selectedHost } = storeToRefs(platform)
const chartData = ref<Record<string, Metric[]>>({})
const processData = ref<Metric[]>([])
const alerts = ref<AlertEvent[]>([])
const loading = ref(false)
const error = ref('')
let refreshTimer: number | undefined

const onlineCount = computed(() => hosts.value.filter((host) => host.online).length)
const score = computed(() => scores.value.general)
const drivers = computed(() => [...(score.value?.factors || [])]
  .filter((factor) => factor.penalty > 0)
  .sort((a, b) => b.penalty * b.weight - a.penalty * a.weight).slice(0, 4))
const topProcesses = computed(() => {
  const map = new Map<string, { pid: string; name: string; cpu: number; rss: number }>()
  for (const metric of processData.value) {
    const pid = metric.labels.pid
    if (!pid) continue
    const row = map.get(pid) || { pid, name: metric.labels.comm || `PID ${pid}`, cpu: 0, rss: 0 }
    if (metric.name === 'proc.cpu_pct') row.cpu = metric.value
    if (metric.name === 'proc.rss_bytes') row.rss = metric.value
    map.set(pid, row)
  }
  return [...map.values()].sort((a, b) => b.cpu - a.cpu).slice(0, 6)
})

async function load() {
  if (!hostId.value) return
  const id = hostId.value
  loading.value = true
  error.value = ''
  try {
    const now = Date.now()
    const params = new URLSearchParams({ host_id: id, category: 'proc', from: String(now - 15_000), to: String(now), limit: '6000' })
    const [cpu, mem, disk, net, processes, events, history] = await Promise.all([
      platform.queryMetrics('cpu'), platform.queryMetrics('mem'), platform.queryMetrics('disk'),
      platform.queryMetrics('net'), request<Metric[]>(`/api/v1/metrics?${params}`),
      request<AlertEvent[]>('/api/v1/alert-events'), platform.queryScores('general')
    ])
    if (id !== hostId.value) return
    chartData.value = { cpu, mem, disk, net }
    processData.value = processes
    alerts.value = events.filter((item) => item.host_id === id).slice(0, 4)
    if (!scores.value.general && history.length) scores.value.general = history.at(-1)!
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { loading.value = false }
}

watch([hostId, range], load, { immediate: true })
onMounted(() => { refreshTimer = window.setInterval(load, 20_000) })
onUnmounted(() => { if (refreshTimer) window.clearInterval(refreshTimer) })
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><h2>运行概况</h2><p>所选节点的资源、内核事件与近期风险。数据每秒上报，图表按当前时间范围查询。</p></div><span class="subtle-meta">{{ selectedHost?.hostname || '未选择节点' }}</span></div>
    <div v-if="error" class="notice error">{{ error }}</div>
    <div v-if="!hostId" class="empty-panel">暂无节点。请先部署 Worker 并等待接入。</div>
    <template v-else>
      <div class="kpi-grid">
        <div class="kpi-card"><div class="kpi-top"><span>CPU 使用率</span><Cpu :size="18" /></div><strong>{{ formatValue('cpu.busy_pct', latest['cpu.busy_pct']) }}</strong><small>每核运行队列 {{ formatValue('cpu.runnable_per_core', latest['cpu.runnable_per_core']) }}</small></div>
        <div class="kpi-card"><div class="kpi-top"><span>内存使用率</span><Database :size="18" /></div><strong>{{ formatValue('mem.used_pct', latest['mem.used_pct']) }}</strong><small>可用 {{ formatValue('mem.available_bytes', latest['mem.available_bytes']) }}</small></div>
        <div class="kpi-card"><div class="kpi-top"><span>磁盘队列</span><HardDrive :size="18" /></div><strong>{{ formatValue('disk.avg_queue_depth', latest['disk.avg_queue_depth']) }}</strong><small>写等待 {{ formatValue('disk.write_await_ms', latest['disk.write_await_ms']) }}</small></div>
        <div class="kpi-card"><div class="kpi-top"><span>网络发送</span><Network :size="18" /></div><strong>{{ formatValue('net.tx_bytes_per_s', latest['net.tx_bytes_per_s']) }}</strong><small>TCP 重传率 {{ formatValue('tcp.retrans_ratio_pct', latest['tcp.retrans_ratio_pct']) }}</small></div>
      </div>

      <div class="dashboard-grid">
        <section class="panel score-overview"><div class="panel-header"><div><h3>性能评分</h3><p>通用场景 · 最近 60 秒</p></div><RouterLink to="/scores" class="text-link">查看解释 <ArrowRight :size="14" /></RouterLink></div>
          <div class="score-main"><strong :class="score?.value == null ? 'neutral' : score.value < 60 ? 'danger' : score.value < 80 ? 'warning' : 'healthy'">{{ score?.value == null ? '—' : score.value.toFixed(0) }}</strong><div><span> / 100</span><p>指标覆盖 {{ score ? (score.coverage * 100).toFixed(0) : '—' }}%</p><small>{{ score?.value == null ? '数据不足，显示缺失项' : '分值按场景权重计算' }}</small></div></div>
          <div class="panel-divider"></div><div class="compact-list"><div v-if="!drivers.length" class="quiet-row">当前窗口没有明显扣分项</div><div v-for="factor in drivers" :key="factor.metric" class="compact-row"><span>{{ metricTitle(factor.metric) }}</span><strong>{{ formatValue(factor.metric, factor.value) }}</strong></div></div>
        </section>
        <section class="panel status-overview"><div class="panel-header"><div><h3>节点与采集能力</h3><p>主机状态和可用探针</p></div><Server :size="18" /></div>
          <div class="status-large"><span class="status-dot" :class="{ offline: !selectedHost?.online }"></span><strong>{{ selectedHost?.online ? '节点在线' : '节点离线' }}</strong><span>最近上报 {{ selectedHost ? timestamp(selectedHost.last_seen_ms) : '—' }}</span></div>
          <div class="capability-row"><span>eBPF</span><strong :class="selectedHost?.capabilities.ebpf ? 'text-success' : 'text-warning'">{{ selectedHost?.capabilities.ebpf ? '可用' : '不可用' }}</strong></div>
          <div class="capability-row"><span>perf</span><strong :class="selectedHost?.capabilities.perf ? 'text-success' : 'text-warning'">{{ selectedHost?.capabilities.perf ? '已安装' : '不可用' }}</strong></div>
          <div class="capability-row"><span>cgroup v2</span><strong :class="selectedHost?.capabilities.cgroup_v2 ? 'text-success' : 'text-warning'">{{ selectedHost?.capabilities.cgroup_v2 ? '可用' : '不可用' }}</strong></div>
          <div class="capability-row"><span>在线节点</span><strong>{{ onlineCount }} / {{ hosts.length }}</strong></div>
        </section>
      </div>

      <div class="chart-grid">
        <section class="panel"><div class="panel-header"><div><h3>CPU 与等待</h3><p>CPU 忙碌、系统态和 I/O 等待</p></div><RouterLink to="/metrics" class="text-link">详情 <ArrowRight :size="14" /></RouterLink></div><LineChart :points="chartData.cpu || []" :names="['cpu.busy_pct','cpu.system_pct','cpu.iowait_pct']" :height="245" /></section>
        <section class="panel"><div class="panel-header"><div><h3>内存</h3><p>有效使用率与可用比例</p></div></div><LineChart :points="chartData.mem || []" :names="['mem.used_pct','mem.available_pct']" :height="245" /></section>
        <section class="panel"><div class="panel-header"><div><h3>磁盘 I/O</h3><p>主机合计读写吞吐</p></div></div><LineChart :points="chartData.disk || []" :names="['disk.read_bytes_per_s','disk.write_bytes_per_s']" :height="245" /></section>
        <section class="panel"><div class="panel-header"><div><h3>网络流量</h3><p>主机合计收发速率</p></div></div><LineChart :points="chartData.net || []" :names="['net.rx_bytes_per_s','net.tx_bytes_per_s']" :height="245" /></section>
      </div>

      <div class="dashboard-grid">
        <section class="panel"><div class="panel-header"><div><h3>CPU 热点进程</h3><p>最近 15 秒采集的前 20 个用户进程</p></div><RouterLink to="/processes" class="text-link">查看进程 <ArrowRight :size="14" /></RouterLink></div>
          <table class="data-table"><thead><tr><th>进程</th><th>PID</th><th class="align-right">CPU</th><th class="align-right">RSS</th></tr></thead><tbody><tr v-for="item in topProcesses" :key="item.pid"><td>{{ item.name }}</td><td class="mono">{{ item.pid }}</td><td class="align-right">{{ formatValue('proc.cpu_pct', item.cpu) }}</td><td class="align-right">{{ formatValue('proc.rss_bytes', item.rss) }}</td></tr><tr v-if="!topProcesses.length"><td colspan="4" class="table-empty">暂无线程/进程数据</td></tr></tbody></table>
        </section>
        <section class="panel"><div class="panel-header"><div><h3>近期告警</h3><p>当前节点最新事件</p></div><RouterLink to="/alerts" class="text-link">告警中心 <ArrowRight :size="14" /></RouterLink></div>
          <div v-if="!alerts.length" class="quiet-state"><CircleAlert :size="22" /><span>当前节点没有近期告警</span></div>
          <div v-for="item in alerts" :key="item.id" class="event-row"><CircleAlert :size="16" /><div><strong>{{ metricTitle(item.metric) }}</strong><small>{{ timestamp(item.triggered_ms) }}</small></div><span class="badge" :class="item.resolved_ms ? 'badge-muted' : 'badge-danger'">{{ item.resolved_ms ? '已恢复' : '进行中' }}</span></div>
        </section>
      </div>
      <div v-if="loading" class="loading-note">正在更新数据…</div>
    </template>
  </div>
</template>
