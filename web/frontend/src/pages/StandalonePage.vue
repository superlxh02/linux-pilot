<script setup lang="ts">
/** 普通主机集群以机器及其单体服务为主线，进程详情留在独立工作台。 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { storeToRefs } from 'pinia'
import { useRouter } from 'vue-router'
import { Activity, Bell, RefreshCw, Server } from '@lucide/vue'
import { request, type AlertEvent } from '../api'
import { usePlatform } from '../store'
import type { ClusterSnapshot, NodeView, ServiceView } from '../topology'

const router = useRouter()
const platform = usePlatform()
const { activeCluster, topologyManifest } = storeToRefs(platform)
const snapshot = ref<ClusterSnapshot | null>(null)
const alerts = ref<AlertEvent[]>([])
const error = ref('')
let timer: number | undefined
const cluster = computed(() => snapshot.value?.clusters.find((item) => item.id === activeCluster.value?.id))
const nodes = computed(() => snapshot.value?.nodes.filter((item) => item.cluster_id === activeCluster.value?.id) || [])
const activeAlerts = computed(() => alerts.value.filter((item) => !item.resolved_ms && nodes.value.some((node) => node.id === item.host_id)))

function serviceFor(node: NodeView): ServiceView | undefined {
  const ids = topologyManifest.value?.spec.services.filter((item) => item.clusterId === activeCluster.value?.id && item.match.type === 'host-process' && item.match.hostId === node.id).map((item) => item.id) || []
  return snapshot.value?.services.find((item) => ids.includes(item.id))
}
async function refresh() {
  try {
    const [next, events] = await Promise.all([request<ClusterSnapshot>('/api/v1/topology/snapshot'), request<AlertEvent[]>('/api/v1/alert-events')])
    snapshot.value = next
    alerts.value = events
    error.value = ''
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}
function openNode(id: string) { platform.setHost(id); void router.push('/overview') }
function score(value: number | null | undefined) { return value == null ? '—' : value.toFixed(0) }
function bytes(value: number) { return value >= 1048576 ? `${(value / 1048576).toFixed(1)} MiB` : value >= 1024 ? `${(value / 1024).toFixed(1)} KiB` : `${value.toFixed(0)} B` }
onMounted(() => { void refresh(); timer = window.setInterval(refresh, 15_000) })
onUnmounted(() => { if (timer) window.clearInterval(timer) })
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><span class="cluster-eyebrow">STANDALONE / {{ activeCluster?.id || '未配置' }}</span><h2>普通主机集群</h2><p>一台主机可以运行多个服务；本地实验为三台逻辑主机，每台部署一个单体服务和一个 Worker。</p></div><button class="button secondary" @click="refresh"><RefreshCw :size="15" /> 刷新</button></div>
    <div v-if="error" class="notice error">{{ error }}</div>
    <div v-if="!snapshot?.configured || !activeCluster" class="empty-panel">尚未配置普通主机集群。请打开“拓扑配置”导入或可视化编辑。</div>
    <template v-else>
      <div class="summary-grid three"><div class="summary-card"><span>集群节点资源分</span><strong>{{ score(cluster?.resource_score) }}</strong><small>所有配置节点数据完整时才计算平均值</small><Activity :size="21" /></div><div class="summary-card"><span>主机在线</span><strong>{{ cluster?.online_nodes ?? 0 }} / {{ cluster?.node_count ?? 0 }}</strong><small>每台模拟主机独立运行 Worker</small><Server :size="21" /></div><div class="summary-card"><span>未恢复告警</span><strong :class="activeAlerts.length ? 'text-danger' : 'text-success'">{{ activeAlerts.length }}</strong><small>当前普通主机集群</small><Bell :size="21" /></div></div>
      <section class="panel"><div class="panel-header"><div><h3>主机与单体服务</h3><p>点击主机进入系统资源总览；进程列表、perf 和详细指标在普通主机模式单独提供。</p></div></div><div class="standalone-node-grid"><button v-for="node in nodes" :key="node.id" class="standalone-node-card" @click="openNode(node.id)"><div class="standalone-node-head"><span class="status-dot" :class="{ offline: !node.online }"></span><strong>{{ node.id }}</strong><span class="badge" :class="node.online ? 'badge-success' : 'badge-muted'">{{ node.online ? '在线' : '离线' }}</span></div><p>{{ node.description }}</p><div class="standalone-node-score"><span>节点分</span><b>{{ score(node.score) }}</b></div><div class="standalone-service-line"><strong>{{ serviceFor(node)?.id || node.role }}</strong><span>{{ serviceFor(node)?.instance_count ? '服务进程在线' : '等待服务进程' }}</span></div><div class="standalone-node-metrics"><span>CPU {{ serviceFor(node)?.cpu_cores.toFixed(2) || '0.00' }} 核</span><span>RSS {{ bytes(serviceFor(node)?.memory_bytes || 0) }}</span><span>写入 {{ bytes(serviceFor(node)?.write_bytes_per_s || 0) }}/s</span><span>Socket 接收 {{ serviceFor(node)?.network_rx_bytes_per_s == null ? '—' : `${bytes(serviceFor(node)!.network_rx_bytes_per_s!)}/s` }}</span></div></button></div></section>
      <p class="scope-note">这三个 Docker 容器各有独立 PID 与 cgroup 边界，但共享 Mac 的 Docker Linux VM 内核与底层硬件；本实验用于验证部署、服务归属和负载联动，不用于模拟三台物理服务器的基准性能。</p>
    </template>
  </div>
</template>
