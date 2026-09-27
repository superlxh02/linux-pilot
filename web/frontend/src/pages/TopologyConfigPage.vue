<script setup lang="ts">
/** 可视化拓扑编辑器。所有修改先留在本地草稿，按版本一次性验证和保存。 */
import { computed, onMounted, ref } from 'vue'
import { storeToRefs } from 'pinia'
import { Download, FileJson2, Plus, Save, Trash2, Upload } from '@lucide/vue'
import { request } from '../api'
import { usePlatform } from '../store'
import type { NodeSpec, ScoreProfileSpec, ServiceSpec, TopologyManifest } from '../topology'

const platform = usePlatform()
const { activeCluster, deploymentMode, role } = storeToRefs(platform)
const draft = ref<TopologyManifest | null>(null)
const error = ref('')
const notice = ref('')
const saving = ref(false)
// 记录打开编辑器时的版本；并发修改必须由后端冲突检查拒绝，不能静默覆盖。
const baseRevision = ref(0)
const canEdit = computed(() => role.value === 'operator' || role.value === 'admin')
const nodes = computed(() => draft.value?.spec.nodes.filter((item) => item.clusterId === activeCluster.value?.id) || [])
const services = computed(() => draft.value?.spec.services.filter((item) => item.clusterId === activeCluster.value?.id) || [])
const profiles = computed(() => draft.value?.spec.scoreProfiles || [])
const dimensions = [
  { key: 'cpu', title: 'CPU' }, { key: 'memory', title: '内存' },
  { key: 'application_io', title: '应用 I/O' }, { key: 'storage', title: '存储' },
  { key: 'network', title: '网络' }
]

async function load() {
  try {
    const current = await request<TopologyManifest | null>('/api/v1/topology/manifest')
    draft.value = current ? structuredClone(current) : null
    baseRevision.value = current?.metadata.revision || 0
    error.value = ''
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}
function setMemory(node: NodeSpec, event: Event) { node.capacity.memoryBytes = Math.round(Number((event.target as HTMLInputElement).value) * 1048576) }
function setDisk(node: NodeSpec, event: Event) { node.capacity.storage[0]!.capacityBytes = Math.round(Number((event.target as HTMLInputElement).value) * 1073741824) }
function addNode() {
  if (!draft.value || !activeCluster.value) return
  draft.value.spec.nodes.push({ id: '', clusterId: activeCluster.value.id, description: '', role: '', scoreProfileRef: profiles.value[0]?.id || '', capacity: { cpuCores: 4, memoryBytes: 8589934592, storage: [{ mount: '/', medium: 'ssd', capacityBytes: 107374182400 }] } })
}
function addProfile() {
  if (!draft.value) return
  draft.value.spec.scoreProfiles.push({ id: `profile-${draft.value.spec.scoreProfiles.length + 1}`, scope: 'node', engine: 'rule-based/v1', scenario: 'general', weights: { cpu: 0.2, memory: 0.2, application_io: 0.2, storage: 0.2, network: 0.2 } })
}
function addService() {
  if (!draft.value || !activeCluster.value) return
  const match = deploymentMode.value === 'kubernetes'
    ? { type: 'kubernetes-workload' as const, namespace: 'default', kind: 'Deployment', name: '' }
    : { type: 'host-process' as const, hostId: nodes.value[0]?.id || '', comm: '' }
  draft.value.spec.services.push({ id: '', clusterId: activeCluster.value.id, description: '', criticality: 'medium', match, dependsOn: [] })
}
function removeNode(node: NodeSpec) { if (draft.value) draft.value.spec.nodes = draft.value.spec.nodes.filter((item) => item !== node) }
function removeService(service: ServiceSpec) { if (draft.value) draft.value.spec.services = draft.value.spec.services.filter((item) => item !== service) }
function profileWeight(profile: ScoreProfileSpec) { return Object.values(profile.weights).reduce((sum, value) => sum + Number(value || 0), 0) }

async function importFile(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  try {
    const parsed = JSON.parse(await file.text()) as TopologyManifest
    await request('/api/v1/topology/validate', { method: 'POST', body: JSON.stringify(parsed) })
    draft.value = parsed
    notice.value = `已读取 ${file.name}，尚未保存到服务器`
    error.value = ''
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  input.value = ''
}
function exportFile() {
  if (!draft.value) return
  const payload = JSON.stringify(draft.value, null, 2) + '\n'
  const url = URL.createObjectURL(new Blob([payload], { type: 'application/json' }))
  const link = document.createElement('a')
  link.href = url
  link.download = 'linux-pilot-topology.json'
  link.click()
  URL.revokeObjectURL(url)
}
async function save() {
  if (!draft.value || !canEdit.value) return
  saving.value = true
  notice.value = ''
  error.value = ''
  try {
    const next = structuredClone(draft.value)
    next.metadata.revision = baseRevision.value + 1
    await request('/api/v1/topology/validate', { method: 'POST', body: JSON.stringify(next) })
    await request('/api/v1/topology/manifest', { method: 'PUT', body: JSON.stringify(next) })
    draft.value = next
    baseRevision.value = next.metadata.revision
    await platform.loadTopology()
    await platform.loadHosts()
    notice.value = `配置版本 ${next.metadata.revision} 已保存`
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { saving.value = false }
}
onMounted(() => { void load() })
</script>

<template>
  <div class="page-stack topology-editor">
    <div class="page-intro"><div><span class="cluster-eyebrow">TOPOLOGY MANIFEST / {{ activeCluster?.id || '未配置' }}</span><h2>可视化拓扑配置</h2><p>编辑节点描述、硬件容量、评分场景和服务归属；所有配置可作为 JSON 文件导入或导出。</p></div><div class="table-tools"><label v-if="canEdit" class="button secondary file-button"><Upload :size="15" /> 导入 JSON<input type="file" accept=".json,application/json" @change="importFile" /></label><button class="button secondary" :disabled="!draft" @click="exportFile"><Download :size="15" /> 导出草稿</button><button v-if="canEdit" class="button primary" :disabled="!draft || saving" @click="save"><Save :size="15" /> {{ saving ? '保存中…' : '校验并保存' }}</button></div></div>
    <div v-if="error" class="notice error">{{ error }}</div><div v-if="notice" class="notice cluster-success">{{ notice }}</div>
    <div v-if="!draft" class="empty-panel">当前没有拓扑配置。先导入项目 virtual_env/manifest.json。</div>
    <template v-else>
      <div class="config-summary"><FileJson2 :size="18" /><span><strong>{{ draft.metadata.id }}</strong> · 当前草稿版本 {{ draft.metadata.revision }}</span><small>保存时从已加载版本递增；若其他人先保存，将提示版本冲突。</small></div>
      <section class="panel"><div class="panel-header"><div><h3>{{ deploymentMode === 'kubernetes' ? 'Kubernetes 工作节点' : '普通主机节点' }}</h3><p>节点 ID 必须与 Worker 上报的 host ID 一致。容量为声明值；本地 Docker/kind 节点共享底层硬件。</p></div><button v-if="canEdit" class="button secondary" @click="addNode"><Plus :size="15" /> 添加节点</button></div>
        <div class="config-card-list"><article v-for="(node, index) in nodes" :key="index" class="config-card"><div class="config-card-title"><strong>节点 {{ index + 1 }}</strong><button v-if="canEdit" class="button text danger" @click="removeNode(node)"><Trash2 :size="14" /> 删除</button></div><div class="config-form-grid"><label>Worker 节点 ID<input v-model.trim="node.id" :disabled="!canEdit" placeholder="与上报 host ID 一致" /></label><label>用途 / 角色<input v-model.trim="node.role" :disabled="!canEdit" placeholder="application / storage / billing" /></label><label class="config-wide">节点描述<input v-model.trim="node.description" :disabled="!canEdit" placeholder="描述业务职责与硬件用途" /></label><label>评分配置<select v-model="node.scoreProfileRef" :disabled="!canEdit"><option v-for="profile in profiles" :key="profile.id" :value="profile.id">{{ profile.id }} · {{ profile.scenario }}</option></select></label><label>CPU 核数<input v-model.number="node.capacity.cpuCores" :disabled="!canEdit" type="number" min="1" step="1" /></label><label>内存 MiB<input :value="Math.round(node.capacity.memoryBytes / 1048576)" :disabled="!canEdit" type="number" min="1" @input="setMemory(node, $event)" /></label><label>存储容量 GiB<input :value="Math.round((node.capacity.storage[0]?.capacityBytes || 0) / 1073741824)" :disabled="!canEdit" type="number" min="1" @input="setDisk(node, $event)" /></label><label>存储挂载点<input v-model.trim="node.capacity.storage[0]!.mount" :disabled="!canEdit" /></label><label>存储介质<input v-model.trim="node.capacity.storage[0]!.medium" :disabled="!canEdit" placeholder="ssd / hdd / virtual" /></label></div></article><div v-if="!nodes.length" class="inline-empty">当前模式还没有节点。</div></div>
      </section>
      <section class="panel"><div class="panel-header"><div><h3>节点评分配置</h3><p>权重总和必须为 1；同一配置可被多个节点引用。修改共享配置会影响所有引用节点。</p></div><button v-if="canEdit" class="button secondary" @click="addProfile"><Plus :size="15" /> 添加配置</button></div><div class="config-card-list"><article v-for="profile in profiles" :key="profile.id" class="config-card"><div class="config-form-grid"><label>配置 ID<input v-model.trim="profile.id" :disabled="!canEdit" /></label><label>场景<select v-model="profile.scenario" :disabled="!canEdit"><option value="general">通用</option><option value="cpu">CPU 密集</option><option value="application_io">应用 I/O</option><option value="storage">存储</option><option value="network">网络</option></select></label></div><div class="config-weight-grid"><label v-for="item in dimensions" :key="item.key">{{ item.title }}<input v-model.number="profile.weights[item.key]" :disabled="!canEdit" type="number" min="0" max="1" step="0.05" /></label></div><small :class="Math.abs(profileWeight(profile) - 1) > 0.000001 ? 'text-danger' : 'text-success'">权重合计 {{ profileWeight(profile).toFixed(2) }}</small></article></div></section>
      <section class="panel"><div class="panel-header"><div><h3>服务归属</h3><p>{{ deploymentMode === 'kubernetes' ? '以命名空间和 Deployment 等稳定工作负载身份匹配 Pod。' : '以主机 ID 与 Linux comm 匹配进程；进程重启后 PID 可变化。' }}</p></div><button v-if="canEdit" class="button secondary" @click="addService"><Plus :size="15" /> 添加服务</button></div><div class="config-card-list"><article v-for="(service, index) in services" :key="index" class="config-card"><div class="config-card-title"><strong>服务 {{ index + 1 }}</strong><button v-if="canEdit" class="button text danger" @click="removeService(service)"><Trash2 :size="14" /> 删除</button></div><div class="config-form-grid"><label>服务 ID<input v-model.trim="service.id" :disabled="!canEdit" /></label><label>重要性<select v-model="service.criticality" :disabled="!canEdit"><option value="critical">关键</option><option value="high">高</option><option value="medium">中</option><option value="low">低</option></select></label><label class="config-wide">描述<input v-model.trim="service.description" :disabled="!canEdit" /></label><template v-if="service.match.type === 'kubernetes-workload'"><label>命名空间<input v-model.trim="service.match.namespace" :disabled="!canEdit" /></label><label>工作负载类型<input v-model.trim="service.match.kind" :disabled="!canEdit" /></label><label>工作负载名称<input v-model.trim="service.match.name" :disabled="!canEdit" /></label></template><template v-else-if="service.match.type === 'host-process'"><label>承载主机<select v-model="service.match.hostId" :disabled="!canEdit"><option v-for="node in nodes" :key="node.id" :value="node.id">{{ node.id }}</option></select></label><label>进程 comm<input v-model.trim="service.match.comm" :disabled="!canEdit" maxlength="15" /></label></template></div></article><div v-if="!services.length" class="inline-empty">当前模式还没有服务。</div></div></section>
    </template>
  </div>
</template>
