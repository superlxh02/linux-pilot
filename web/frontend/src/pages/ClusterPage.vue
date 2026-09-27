<script setup lang="ts">
/**
 * 集群页展示跨节点资源的实际归属，分别标明基础设施分与业务整体分。
 * 导入和导出共用版本化 JSON 契约，运维与将来的 AI 工具无需复制界面操作。
 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { storeToRefs } from 'pinia'
import { Download, FileJson2, RefreshCw, Upload } from '@lucide/vue'
import { request } from '../api'
import { usePlatform } from '../store'

interface PodView { uid: string; name: string; node_id: string; ready: boolean; resource_score: number | null; metrics: Record<string, number> }
interface ServiceView { id: string; description: string; criticality: string; pod_count: number; covered_pods: number; resource_score: number | null; cpu_cores: number; memory_bytes: number; read_bytes_per_s: number; write_bytes_per_s: number; network_rx_bytes_per_s: number | null; network_tx_bytes_per_s: number | null; pods: PodView[] }
interface NodeView { id: string; role: string; description: string; score: number | null; coverage: number; profile: string; scenario: string; capacity: { cpuCores: number; memoryBytes: number } }
interface ClusterSnapshot { configured: boolean; revision?: number; time_ms?: number; infrastructure_score: number | null; overall_score: number | null; service_coverage?: number; nodes: NodeView[]; services: ServiceView[] }

const platform = usePlatform()
const { role } = storeToRefs(platform)
const canEdit = computed(() => role.value === 'operator' || role.value === 'admin')
const snapshot = ref<ClusterSnapshot | null>(null)
const manifest = ref<unknown>(null)
const importText = ref('')
const showImport = ref(false)
const busy = ref(false)
const message = ref('')
const error = ref('')
let timer: number | undefined

async function refresh() {
  try {
    const [next, config] = await Promise.all([
      request<ClusterSnapshot>('/api/v1/topology/snapshot'),
      request<unknown>('/api/v1/topology/manifest')
    ])
    snapshot.value = next
    manifest.value = config
    error.value = ''
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}

function exportManifest() {
  if (!manifest.value) return
  const blob = new Blob([JSON.stringify(manifest.value, null, 2) + '\n'], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = 'linux-pilot-topology.json'
  link.click()
  URL.revokeObjectURL(url)
}

async function loadFile(event: Event) {
  const file = (event.target as HTMLInputElement).files?.[0]
  if (file) importText.value = await file.text()
}

async function applyManifest() {
  if (!canEdit.value) return
  busy.value = true
  error.value = ''
  message.value = ''
  try {
    const parsed = JSON.parse(importText.value)
    await request('/api/v1/topology/validate', { method: 'POST', body: JSON.stringify(parsed) })
    await request('/api/v1/topology/manifest', { method: 'PUT', body: JSON.stringify(parsed) })
    message.value = `配置版本 ${parsed.metadata.revision} 已应用`
    showImport.value = false
    await refresh()
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { busy.value = false }
}

function bytes(value: number | null) {
  if (value == null || !Number.isFinite(value)) return '—'
  if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(1)} GiB`
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(1)} MiB`
  if (value >= 1024) return `${(value / 1024).toFixed(1)} KiB`
  return `${value.toFixed(0)} B`
}
function score(value: number | null) { return value == null ? '—' : value.toFixed(0) }
function scenarioName(value: string) { return ({ general: '通用', cpu: 'CPU', application_io: '应用 I/O', storage: '存储', network: '网络' } as Record<string, string>)[value] || value }

onMounted(() => { void refresh(); timer = window.setInterval(refresh, 15_000) })
onUnmounted(() => { if (timer) window.clearInterval(timer) })
</script>

<template>
  <div class="page-stack cluster-page">
    <div class="page-intro"><div><h2>集群与服务</h2><p>从节点、Pod 到逻辑服务查看资源归属。配置以版本化 JSON 导入，可由脚本复用。</p></div><div class="table-tools"><button class="button secondary" @click="refresh"><RefreshCw :size="15" /> 刷新</button><button v-if="manifest" class="button secondary" @click="exportManifest"><Download :size="15" /> 导出配置</button><button v-if="canEdit" class="button primary" @click="showImport = !showImport"><Upload :size="15" /> 导入配置</button></div></div>
    <div v-if="error" class="notice error">{{ error }}</div><div v-if="message" class="notice cluster-success">{{ message }}</div>
    <section v-if="showImport" class="panel"><div class="panel-header"><div><h3>导入 TopologyManifest</h3><p>先校验 schema，再按 revision 应用。已有版本必须递增 1。</p></div><FileJson2 :size="19" /></div><input type="file" accept="application/json,.json" @change="loadFile" /><textarea v-model="importText" class="cluster-json" spellcheck="false" placeholder="选择 JSON 文件，或粘贴 TopologyManifest" /><div class="cluster-actions"><button class="button primary" :disabled="busy || !importText" @click="applyManifest">{{ busy ? '校验中…' : '校验并应用' }}</button></div></section>
    <div v-if="snapshot && !snapshot.configured" class="empty-panel">尚未导入拓扑配置。可从项目 virtual_env/manifest.json 开始。</div>
    <template v-if="snapshot?.configured">
      <div class="summary-grid three"><div class="summary-card"><span>服务基础设施分</span><strong>{{ score(snapshot.infrastructure_score) }}</strong><small>由已覆盖 Pod 的资源争用与健康状态汇总</small></div><div class="summary-card"><span>业务整体分</span><strong>待接入</strong><small>缺少端到端请求延迟、成功率等 SLI；不显示假分数</small></div><div class="summary-card"><span>服务覆盖率</span><strong>{{ Math.round((snapshot.service_coverage || 0) * 100) }}%</strong><small>配置版本 {{ snapshot.revision }} · {{ snapshot.services.length }} 个逻辑服务</small></div></div>
      <section class="panel"><div class="panel-header"><div><h3>逻辑服务</h3><p>同一服务的 Pod 可以分布在不同节点；统计选取 Pod 父 cgroup，避免重复计算子容器。</p></div></div><div class="cluster-service-grid"><article v-for="service in snapshot.services" :key="service.id" class="cluster-service"><div class="cluster-service-head"><div><span class="cluster-eyebrow">{{ service.criticality === 'critical' ? '关键服务' : '服务' }}</span><h4>{{ service.id }}</h4><p>{{ service.description }}</p></div><div class="cluster-score">{{ score(service.resource_score) }}<small>资源分</small></div></div><div class="cluster-resource-row"><div><span>CPU 用量</span><strong>{{ service.cpu_cores.toFixed(2) }} 核</strong></div><div><span>内存</span><strong>{{ bytes(service.memory_bytes) }}</strong></div><div><span>磁盘读取</span><strong>{{ bytes(service.read_bytes_per_s) }}/s</strong></div><div><span>磁盘写入</span><strong>{{ bytes(service.write_bytes_per_s) }}/s</strong></div><div><span>Pod 网络接收</span><strong>{{ bytes(service.network_rx_bytes_per_s) }}{{ service.network_rx_bytes_per_s == null ? '' : '/s' }}</strong></div><div><span>Pod 网络发送</span><strong>{{ bytes(service.network_tx_bytes_per_s) }}{{ service.network_tx_bytes_per_s == null ? '' : '/s' }}</strong></div></div><div class="cluster-pods"><div class="cluster-pod" v-for="pod in service.pods" :key="pod.uid"><span class="status-dot" :class="{ offline: !pod.ready }"></span><span><strong>{{ pod.name }}</strong><small>{{ pod.node_id }}</small></span><b>{{ score(pod.resource_score) }}</b></div><div v-if="!service.pod_count" class="inline-empty">等待 Kubernetes 发现器上报 Pod</div></div><div class="cluster-coverage">已覆盖 {{ service.covered_pods }} / {{ service.pod_count }} 个 Pod · 暂无应用 SLI</div></article></div></section>
      <section class="panel"><div class="panel-header"><div><h3>节点场景评分</h3><p>每个节点的评分场景与维度权重来自当前 JSON 配置。</p></div></div><div class="table-scroll"><table class="data-table"><thead><tr><th>节点</th><th>角色 / 描述</th><th>评分配置</th><th>申报容量</th><th class="align-right">分数</th><th class="align-right">覆盖率</th></tr></thead><tbody><tr v-for="node in snapshot.nodes" :key="node.id"><td class="mono"><strong>{{ node.id }}</strong></td><td>{{ node.role }}<small>{{ node.description }}</small></td><td>{{ scenarioName(node.scenario) }}<small class="mono">{{ node.profile }}</small></td><td>{{ node.capacity.cpuCores }} 核 · {{ bytes(node.capacity.memoryBytes) }}</td><td class="align-right number-cell">{{ score(node.score) }}</td><td class="align-right">{{ Math.round(node.coverage * 100) }}%</td></tr></tbody></table></div></section>
    </template>
  </div>
</template>
