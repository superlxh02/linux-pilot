<script setup lang="ts">
/** Kubernetes 集群入口：服务是第一视角，工作节点仅提供调度位置与容量信息。 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { storeToRefs } from 'pinia'
import { RouterLink, useRouter } from 'vue-router'
import { Activity, ArrowRight, Bell, Boxes, RefreshCw, Server } from '@lucide/vue'
import { request, type AlertEvent } from '../api'
import { usePlatform } from '../store'
import type { ClusterSnapshot } from '../topology'

const platform = usePlatform()
const router = useRouter()
const { activeCluster, visibleHosts } = storeToRefs(platform)
const snapshot = ref<ClusterSnapshot | null>(null)
const alerts = ref<AlertEvent[]>([])
const error = ref('')
let timer: number | undefined
const cluster = computed(() => snapshot.value?.clusters.find((item) => item.id === activeCluster.value?.id))
const services = computed(() => snapshot.value?.services.filter((item) => item.cluster_id === activeCluster.value?.id) || [])
const nodes = computed(() => snapshot.value?.nodes.filter((item) => item.cluster_id === activeCluster.value?.id) || [])
const activeAlerts = computed(() => alerts.value.filter((item) => !item.resolved_ms && nodes.value.some((node) => node.id === item.host_id)))

async function refresh() {
  try {
    const [next, events] = await Promise.all([
      request<ClusterSnapshot>('/api/v1/topology/snapshot'),
      request<AlertEvent[]>('/api/v1/alert-events')
    ])
    snapshot.value = next
    alerts.value = events
    error.value = ''
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}
function score(value: number | null | undefined) { return value == null ? '—' : value.toFixed(0) }
function bytes(value: number) { return value >= 1048576 ? `${(value / 1048576).toFixed(1)} MiB` : value >= 1024 ? `${(value / 1024).toFixed(1)} KiB` : `${value.toFixed(0)} B` }
function openNode(id: string) { platform.setHost(id); void router.push('/node-overview') }
onMounted(() => { void refresh(); timer = window.setInterval(refresh, 15_000) })
onUnmounted(() => { if (timer) window.clearInterval(timer) })
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><span class="cluster-eyebrow">KUBERNETES / {{ activeCluster?.id || '未配置' }}</span><h2>集群总览</h2><p>从微服务、Pod 副本和承载节点追踪性能；一个服务可跨节点，一个节点也可承载多个服务。</p></div><button class="button secondary" @click="refresh"><RefreshCw :size="15" /> 刷新</button></div>
    <div v-if="error" class="notice error">{{ error }}</div>
    <div v-if="!snapshot?.configured || !activeCluster" class="empty-panel">尚未配置 Kubernetes 集群。请打开“拓扑配置”导入或可视化编辑。</div>
    <template v-else>
      <div class="summary-grid three">
        <div class="summary-card"><span>集群资源分</span><strong>{{ score(cluster?.resource_score) }}</strong><small>按已覆盖的服务 Pod 资源争用汇总；业务 SLI 尚未接入</small><Activity :size="21" /></div>
        <div class="summary-card"><span>节点在线</span><strong>{{ cluster?.online_nodes ?? 0 }} / {{ cluster?.node_count ?? 0 }}</strong><small>每个 Node 各有一个 Worker DaemonSet Pod</small><Server :size="21" /></div>
        <div class="summary-card"><span>未恢复告警</span><strong :class="activeAlerts.length ? 'text-danger' : 'text-success'">{{ activeAlerts.length }}</strong><small>当前 Kubernetes 集群内的节点告警</small><Bell :size="21" /></div>
      </div>
      <section class="panel"><div class="panel-header"><div><h3>微服务</h3><p>先看服务整体，再进入副本和资源明细。{{ services.length }} 个已配置服务。</p></div><RouterLink to="/services" class="text-link">全部服务 <ArrowRight :size="14" /></RouterLink></div>
        <div class="service-summary-grid"><RouterLink v-for="service in services" :key="service.id" class="service-summary-card" :to="{ path: '/services', query: { service: service.id } }"><div class="service-summary-top"><span class="service-icon"><Boxes :size="18" /></span><span class="badge" :class="(service.covered_pods || 0) === (service.pod_count || 0) && service.pod_count ? 'badge-success' : 'badge-warning'">{{ service.covered_pods || 0 }} / {{ service.pod_count || 0 }} Pod 可评分</span></div><h4>{{ service.id }}</h4><p>{{ service.description }}</p><div class="service-summary-stats"><span>资源分 <b>{{ score(service.resource_score) }}</b></span><span>CPU <b>{{ service.cpu_cores.toFixed(2) }} 核</b></span><span>内存 <b>{{ bytes(service.memory_bytes) }}</b></span></div><div class="service-summary-foot">查看副本与趋势 <ArrowRight :size="14" /></div></RouterLink></div>
      </section>
      <section class="panel"><div class="panel-header"><div><h3>承载节点</h3><p>节点是 Kubernetes 调度单元，可能是物理机或虚拟机；此实验中的节点是 kind 容器。</p></div><RouterLink to="/nodes" class="text-link">查看节点指标 <ArrowRight :size="14" /></RouterLink></div>
        <div class="node-strip"><button v-for="node in nodes" :key="node.id" class="node-strip-item" @click="openNode(node.id)"><span class="status-dot" :class="{ offline: !node.online }"></span><span><strong>{{ node.id }}</strong><small>{{ node.description }}</small></span><b>{{ score(node.score) }}</b></button></div>
      </section>
      <p class="scope-note">当前 {{ visibleHosts.length }} 个 Node 已进入拓扑。集群资源分用于定位资源争用；缺少端到端成功率、延迟和 SLO 时，业务整体分保持空值。</p>
    </template>
  </div>
</template>
