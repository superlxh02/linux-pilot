<script setup lang="ts">
/** 服务详情按稳定工作负载身份查询；Pod UID 变化后自动切换到新副本。 */
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from 'vue'
import { storeToRefs } from 'pinia'
import { useRoute } from 'vue-router'
import { Activity, Database, HardDrive, Network, RefreshCw } from '@lucide/vue'
import { request, type Metric } from '../api'
import { usePlatform } from '../store'
import type { ClusterSnapshot, PodView } from '../topology'

const LineChart = defineAsyncComponent(() => import('../components/LineChart.vue'))
const route = useRoute()
const platform = usePlatform()
const { activeCluster, range } = storeToRefs(platform)
const snapshot = ref<ClusterSnapshot | null>(null)
const chosenService = ref('')
const chosenPod = ref('')
const cgroupPoints = ref<Metric[]>([])
const networkPoints = ref<Metric[]>([])
const error = ref('')
let timer: number | undefined

const services = computed(() => snapshot.value?.services.filter((item) => item.cluster_id === activeCluster.value?.id) || [])
const service = computed(() => services.value.find((item) => item.id === chosenService.value))
const pod = computed<PodView | undefined>(() => service.value?.pods?.find((item) => item.uid === chosenPod.value))

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
  const { from, to, step_ms } = platform.timeWindow()
  const common = { host_id: target.node_id, from: String(from), to: String(to), step_ms: String(step_ms), limit: '10000' }
  const cgroup = new URLSearchParams({ ...common, category: 'cgroup', labels: JSON.stringify({ pod_uid: target.uid, cgroup: `pod:${target.uid}` }) })
  const network = new URLSearchParams({ ...common, category: 'pod', labels: JSON.stringify({ pod_uid: target.uid }) })
  const [group, net] = await Promise.all([
    request<Metric[]>(`/api/v1/metrics?${cgroup}`),
    request<Metric[]>(`/api/v1/metrics?${network}`)
  ])
  if (pod.value?.uid === target.uid) { cgroupPoints.value = group; networkPoints.value = net }
}

function chooseService(id: string) { chosenService.value = id; chosenPod.value = services.value.find((item) => item.id === id)?.pods?.[0]?.uid || ''; void loadSeries() }
function choosePod(uid: string) { chosenPod.value = uid; void loadSeries() }
function bytes(value: number | null | undefined) { if (value == null) return '—'; return value >= 1048576 ? `${(value / 1048576).toFixed(1)} MiB` : value >= 1024 ? `${(value / 1024).toFixed(1)} KiB` : `${value.toFixed(0)} B` }
function score(value: number | null | undefined) { return value == null ? '—' : value.toFixed(0) }
watch(range, () => { void loadSeries() })
watch(() => route.query.service, () => { void refresh() })
onMounted(() => { void refresh(); timer = window.setInterval(refresh, 15_000) })
onUnmounted(() => { if (timer) window.clearInterval(timer) })
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><span class="cluster-eyebrow">KUBERNETES / WORKLOADS</span><h2>微服务性能</h2><p>服务跨节点聚合；Pod 副本按自身 cgroup 统计 CPU、内存与 I/O，网络来自 Pod 命名空间。</p></div><button class="button secondary" @click="refresh"><RefreshCw :size="15" /> 刷新</button></div>
    <div v-if="error" class="notice error">{{ error }}</div>
    <div v-if="!services.length" class="empty-panel">当前集群没有配置微服务。请在拓扑配置中添加工作负载选择器。</div>
    <template v-else>
      <div class="service-picker"><button v-for="item in services" :key="item.id" :class="{ active: item.id === chosenService }" @click="chooseService(item.id)"><span>{{ item.id }}</span><small>{{ item.covered_pods || 0 }} / {{ item.pod_count || 0 }} 副本可评分</small></button></div>
      <template v-if="service">
        <section class="panel service-heading"><div><span class="cluster-eyebrow">{{ service.criticality === 'critical' ? '关键服务' : '服务' }}</span><h3>{{ service.id }}</h3><p>{{ service.description }}</p></div><div class="cluster-score">{{ score(service.resource_score) }}<small>资源分 · 非业务 SLO</small></div></section>
        <div class="kpi-grid"><div class="kpi-card"><div class="kpi-top"><span>CPU 用量</span><Activity :size="18" /></div><strong>{{ service.cpu_cores.toFixed(2) }} 核</strong><small>所有 Pod 父 cgroup 求和</small></div><div class="kpi-card"><div class="kpi-top"><span>内存</span><Database :size="18" /></div><strong>{{ bytes(service.memory_bytes) }}</strong><small>所有副本当前计费内存</small></div><div class="kpi-card"><div class="kpi-top"><span>磁盘写入</span><HardDrive :size="18" /></div><strong>{{ bytes(service.write_bytes_per_s) }}/s</strong><small>当前采样窗口</small></div><div class="kpi-card"><div class="kpi-top"><span>Pod 网络接收</span><Network :size="18" /></div><strong>{{ bytes(service.network_rx_bytes_per_s) }}{{ service.network_rx_bytes_per_s == null ? '' : '/s' }}</strong><small>已覆盖 {{ service.network_covered_pods || 0 }} 个 Pod</small></div></div>
        <section class="panel"><div class="panel-header"><div><h3>Pod 副本</h3><p>点击副本查看其独立时序；同一 Node 上可承载多个不同服务的 Pod。</p></div></div><div class="pod-picker"><button v-for="item in service.pods || []" :key="item.uid" :class="{ active: item.uid === chosenPod }" @click="choosePod(item.uid)"><span class="status-dot" :class="{ offline: !item.ready }"></span><strong>{{ item.name }}</strong><small>{{ item.node_id }}</small><b>{{ score(item.resource_score) }}</b></button></div><div v-if="!service.pods?.length" class="inline-empty">尚未发现 Pod；检查发现器和命名空间选择器。</div></section>
        <template v-if="pod"><div class="pod-metric-strip"><div><span>CPU</span><strong>{{ ((pod.metrics['cgroup.cpu_usage_pct'] || 0) / 100).toFixed(2) }} 核</strong></div><div><span>计费内存</span><strong>{{ bytes(pod.metrics['cgroup.mem_current_bytes']) }}</strong></div><div><span>磁盘读</span><strong>{{ bytes(pod.metrics['cgroup.io_read_bytes_per_s']) }}/s</strong></div><div><span>磁盘写</span><strong>{{ bytes(pod.metrics['cgroup.io_write_bytes_per_s']) }}/s</strong></div></div>
          <div class="chart-grid"><section class="panel"><div class="panel-header"><div><h3>副本 CPU 与内存</h3><p>{{ pod.node_id }} · {{ pod.name }}</p></div></div><LineChart :points="cgroupPoints" :names="['cgroup.cpu_usage_pct','cgroup.mem_current_bytes']" :height="240" /></section><section class="panel"><div class="panel-header"><div><h3>副本存储 I/O</h3><p>父 Pod cgroup，避免子容器重复计数</p></div></div><LineChart :points="cgroupPoints" :names="['cgroup.io_read_bytes_per_s','cgroup.io_write_bytes_per_s']" :height="240" /></section><section class="panel"><div class="panel-header"><div><h3>Pod 网络</h3><p>共享网络命名空间，不按容器拆分</p></div></div><LineChart :points="networkPoints" :names="['pod.net_rx_bytes_per_s','pod.net_tx_bytes_per_s']" :height="240" /></section><section class="panel"><div class="panel-header"><div><h3>资源争用</h3><p>CPU 限流周期与内存压力</p></div></div><LineChart :points="cgroupPoints" :names="['cgroup.cpu_throttled_period_ratio_pct','cgroup.psi.memory.some.avg10']" :height="240" /></section></div>
        </template>
      </template>
    </template>
  </div>
</template>
