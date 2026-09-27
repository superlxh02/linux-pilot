<script setup lang="ts">
/**
 * Kubernetes 服务工作台按 Deployment 身份选择服务，再沿 Pod UID 追踪副本。
 * 后端的指标接口会保留 pod_uid/cgroup 标签；图表必须显式传入同一维度，
 * 否则通用图表只画无标签主机汇总数据，结果看起来会像“没有采集”。
 */
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from 'vue'
import { storeToRefs } from 'pinia'
import { useRoute, useRouter } from 'vue-router'
import { Activity, Database, HardDrive, Network, RefreshCw } from '@lucide/vue'
import { request, type Metric } from '../api'
import { formatValue } from '../catalog'
import { usePlatform } from '../store'
import type { ClusterSnapshot, PodView } from '../topology'

const LineChart = defineAsyncComponent(() => import('../components/LineChart.vue'))
const route = useRoute()
const router = useRouter()
const platform = usePlatform()
const { activeCluster, range } = storeToRefs(platform)
const snapshot = ref<ClusterSnapshot | null>(null)
const chosenService = ref('')
const chosenPod = ref('')
const cgroupPoints = ref<Metric[]>([])
const networkPoints = ref<Metric[]>([])
const error = ref('')
const loadingSeries = ref(false)
let timer: number | undefined

const services = computed(() => snapshot.value?.services.filter((item) => item.cluster_id === activeCluster.value?.id) || [])
const service = computed(() => services.value.find((item) => item.id === chosenService.value))
const pod = computed<PodView | undefined>(() => service.value?.pods?.find((item) => item.uid === chosenPod.value))
const cgroupFilter = computed(() => pod.value ? { pod_uid: pod.value.uid, cgroup: `pod:${pod.value.uid}` } : undefined)
const networkFilter = computed(() => pod.value ? { pod_uid: pod.value.uid } : undefined)
const readyPods = computed(() => service.value?.pods?.filter((item) => item.ready).length || 0)

async function refresh() {
  try {
    snapshot.value = await request<ClusterSnapshot>('/api/v1/topology/snapshot')
    const requested = typeof route.query.service === 'string' ? route.query.service : ''
    if (requested && services.value.some((item) => item.id === requested)) chosenService.value = requested
    else if (!services.value.some((item) => item.id === chosenService.value)) chosenService.value = services.value[0]?.id || ''
    if (!service.value?.pods?.some((item) => item.uid === chosenPod.value)) chosenPod.value = service.value?.pods?.[0]?.uid || ''
    await loadSeries()
    error.value = ''
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}

async function loadSeries() {
  const target = pod.value
  if (!target) { cgroupPoints.value = []; networkPoints.value = []; return }
  loadingSeries.value = true
  const { from, to, step_ms } = platform.timeWindow()
  const common = { host_id: target.node_id, from: String(from), to: String(to), step_ms: String(step_ms), limit: '10000' }
  const cgroup = new URLSearchParams({ ...common, category: 'cgroup', labels: JSON.stringify({ pod_uid: target.uid, cgroup: `pod:${target.uid}` }) })
  const network = new URLSearchParams({ ...common, category: 'pod', labels: JSON.stringify({ pod_uid: target.uid }) })
  try {
    const [group, net] = await Promise.all([
      request<Metric[]>(`/api/v1/metrics?${cgroup}`),
      request<Metric[]>(`/api/v1/metrics?${network}`)
    ])
    // 用户切换副本时旧请求可能晚到；UID 校验避免把旧 Pod 曲线画到新 Pod。
    if (pod.value?.uid === target.uid) { cgroupPoints.value = group; networkPoints.value = net }
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { loadingSeries.value = false }
}

function chooseService(id: string) { void router.replace({ query: { ...route.query, service: id } }) }
function choosePod(uid: string) { chosenPod.value = uid; void loadSeries() }
function bytes(value: number | null | undefined, rate = false) {
  return value == null ? '—' : formatValue(rate ? 'pod.net_rx_bytes_per_s' : 'cgroup.mem_current_bytes', value)
}
function cores(value: number | null | undefined) { return value == null ? '—' : `${(value / 100).toFixed(2)} 核` }
function score(value: number | null | undefined) { return value == null ? '—' : value.toFixed(0) }
watch(range, () => { void loadSeries() })
watch(() => route.query.service, () => { void refresh() })
onMounted(() => { void refresh(); timer = window.setInterval(refresh, 15_000) })
onUnmounted(() => { if (timer) window.clearInterval(timer) })
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><span class="cluster-eyebrow">KUBERNETES / WORKLOADS</span><h2>微服务性能</h2><p>按服务查看所有 Pod 的当前资源，再选择一个副本分析历史曲线。</p></div><button class="button secondary" @click="refresh"><RefreshCw :size="15" /> 刷新</button></div>
    <div v-if="error" class="notice error">{{ error }}</div>
    <div v-if="!services.length" class="empty-panel">当前集群没有配置微服务。请在拓扑配置中添加工作负载选择器。</div>
    <template v-else>
      <div class="service-picker"><button v-for="item in services" :key="item.id" :class="{ active: item.id === chosenService }" @click="chooseService(item.id)"><span>{{ item.id }}</span><small>{{ item.covered_pods || 0 }} / {{ item.pod_count || 0 }} 副本可评分</small></button></div>
      <template v-if="service">
        <section class="panel service-heading"><div><span class="cluster-eyebrow">{{ service.criticality === 'critical' ? '关键服务' : '服务' }}</span><h3>{{ service.id }}</h3><p>{{ service.description }}</p></div><div class="cluster-score">{{ score(service.resource_score) }}<small>资源分 · 非业务 SLO</small></div></section>
        <div class="kpi-grid">
          <div class="kpi-card"><div class="kpi-top"><span>就绪副本</span><Activity :size="18" /></div><strong>{{ readyPods }} / {{ service.pod_count || 0 }}</strong><small>跨节点 Deployment 实例</small></div>
          <div class="kpi-card"><div class="kpi-top"><span>服务 CPU</span><Activity :size="18" /></div><strong>{{ service.covered_pods ? service.cpu_cores.toFixed(2) + ' 核' : '—' }}</strong><small>各 Pod 父 cgroup 用量之和</small></div>
          <div class="kpi-card"><div class="kpi-top"><span>服务内存</span><Database :size="18" /></div><strong>{{ service.covered_pods ? bytes(service.memory_bytes) : '—' }}</strong><small>所有可用副本计费内存</small></div>
          <div class="kpi-card"><div class="kpi-top"><span>磁盘读取</span><HardDrive :size="18" /></div><strong>{{ service.covered_pods ? bytes(service.read_bytes_per_s, true) : '—' }}</strong><small>当前采样窗口</small></div>
          <div class="kpi-card"><div class="kpi-top"><span>磁盘写入</span><HardDrive :size="18" /></div><strong>{{ service.covered_pods ? bytes(service.write_bytes_per_s, true) : '—' }}</strong><small>当前采样窗口</small></div>
          <div class="kpi-card"><div class="kpi-top"><span>Pod 网络接收</span><Network :size="18" /></div><strong>{{ bytes(service.network_rx_bytes_per_s, true) }}</strong><small>覆盖 {{ service.network_covered_pods || 0 }} 个副本</small></div>
          <div class="kpi-card"><div class="kpi-top"><span>Pod 网络发送</span><Network :size="18" /></div><strong>{{ bytes(service.network_tx_bytes_per_s, true) }}</strong><small>Pod 网络命名空间口径</small></div>
        </div>
        <section class="panel"><div class="panel-header"><div><h3>Pod 副本</h3><p>选择副本查看 cgroup、网络与资源争用曲线；一个 Node 可同时承载多个服务。</p></div></div><div class="pod-picker"><button v-for="item in service.pods || []" :key="item.uid" :class="{ active: item.uid === chosenPod }" @click="choosePod(item.uid)"><span class="status-dot" :class="{ offline: !item.ready }"></span><strong>{{ item.name }}</strong><small>{{ item.node_id }}</small><b>{{ score(item.resource_score) }}</b></button></div><div v-if="!service.pods?.length" class="inline-empty">尚未发现 Pod；检查发现器和命名空间选择器。</div></section>
        <template v-if="pod">
          <div class="pod-metric-strip">
            <div><span>副本 CPU</span><strong>{{ cores(pod.metrics['cgroup.cpu_usage_pct']) }}</strong></div>
            <div><span>计费内存</span><strong>{{ bytes(pod.metrics['cgroup.mem_current_bytes']) }}</strong></div>
            <div><span>磁盘读</span><strong>{{ bytes(pod.metrics['cgroup.io_read_bytes_per_s'], true) }}</strong></div>
            <div><span>磁盘写</span><strong>{{ bytes(pod.metrics['cgroup.io_write_bytes_per_s'], true) }}</strong></div>
            <div><span>网络接收</span><strong>{{ bytes(pod.metrics['pod.net_rx_bytes_per_s'], true) }}</strong></div>
            <div><span>网络发送</span><strong>{{ bytes(pod.metrics['pod.net_tx_bytes_per_s'], true) }}</strong></div>
            <div><span>CPU 限流周期</span><strong>{{ formatValue('cgroup.cpu_throttled_period_ratio_pct', pod.metrics['cgroup.cpu_throttled_period_ratio_pct']) }}</strong></div>
            <div><span>内存 PSI</span><strong>{{ formatValue('cgroup.psi.memory.some.avg10', pod.metrics['cgroup.psi.memory.some.avg10']) }}</strong></div>
          </div>
          <div v-if="loadingSeries" class="loading-note">正在读取副本历史数据…</div>
          <div class="chart-grid">
            <section class="panel"><div class="panel-header"><div><h3>副本 CPU</h3><p>{{ pod.node_id }} · 100% 表示占用一个逻辑核心</p></div></div><LineChart v-if="cgroupPoints.length" :points="cgroupPoints" :names="['cgroup.cpu_usage_pct']" :label-filter="cgroupFilter" :height="220" /><div v-else class="inline-empty">该时间范围没有 CPU 时序</div></section>
            <section class="panel"><div class="panel-header"><div><h3>副本内存</h3><p>Pod 父 cgroup 计费量</p></div></div><LineChart v-if="cgroupPoints.length" :points="cgroupPoints" :names="['cgroup.mem_current_bytes']" :label-filter="cgroupFilter" :height="220" /><div v-else class="inline-empty">该时间范围没有内存时序</div></section>
            <section class="panel"><div class="panel-header"><div><h3>副本存储 I/O</h3><p>读写字节速率</p></div></div><LineChart v-if="cgroupPoints.length" :points="cgroupPoints" :names="['cgroup.io_read_bytes_per_s','cgroup.io_write_bytes_per_s']" :label-filter="cgroupFilter" :height="220" /><div v-else class="inline-empty">该时间范围没有 I/O 时序</div></section>
            <section class="panel"><div class="panel-header"><div><h3>Pod 网络</h3><p>共享网络命名空间，不按容器拆分</p></div></div><LineChart v-if="networkPoints.length" :points="networkPoints" :names="['pod.net_rx_bytes_per_s','pod.net_tx_bytes_per_s']" :label-filter="networkFilter" :height="220" /><div v-else class="inline-empty">该时间范围没有 Pod 网络时序</div></section>
            <section class="panel"><div class="panel-header"><div><h3>CPU 限流</h3><p>配额周期被限流的比例</p></div></div><LineChart v-if="cgroupPoints.length" :points="cgroupPoints" :names="['cgroup.cpu_throttled_period_ratio_pct']" :label-filter="cgroupFilter" :height="220" /><div v-else class="inline-empty">该时间范围没有限流时序</div></section>
            <section class="panel"><div class="panel-header"><div><h3>资源压力</h3><p>CPU、内存和 I/O PSI</p></div></div><LineChart v-if="cgroupPoints.length" :points="cgroupPoints" :names="['cgroup.psi.cpu.some.avg10','cgroup.psi.memory.some.avg10','cgroup.psi.io.some.avg10']" :label-filter="cgroupFilter" :height="220" /><div v-else class="inline-empty">该时间范围没有 PSI 时序</div></section>
          </div>
        </template>
      </template>
    </template>
  </div>
</template>
