<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { storeToRefs } from 'pinia'
import { Bell, Plus, Search, ShieldAlert } from '@lucide/vue'
import { request, type AlertEvent, type AlertRule } from '../api'
import { formatValue, metricTitle, timestamp } from '../catalog'
import { usePlatform } from '../store'

const platform = usePlatform()
const { hostId, role, hosts } = storeToRefs(platform)
const rules = ref<AlertRule[]>([])
const events = ref<AlertEvent[]>([])
const metric = ref('cpu.runnable_per_core')
const comparison = ref('above')
const threshold = ref(2)
const duration = ref(10)
const targetHost = ref('')
const filter = ref('')
const error = ref('')
const busy = ref(false)
let timer: number | undefined
const filteredRules = computed(() => rules.value.filter((item) => `${item.metric} ${item.host_id}`.toLowerCase().includes(filter.value.toLowerCase())))
const filteredEvents = computed(() => events.value.filter((item) => !hostId.value || item.host_id === hostId.value))
const activeEvents = computed(() => filteredEvents.value.filter((item) => item.resolved_ms === null).length)

async function load() {
  try {
    const [newRules, newEvents] = await Promise.all([
      request<AlertRule[]>('/api/v1/alerts'), request<AlertEvent[]>('/api/v1/alert-events')
    ])
    rules.value = newRules; events.value = newEvents; error.value = ''
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}

async function create() {
  busy.value = true
  error.value = ''
  try {
    await request('/api/v1/alerts', { method: 'POST', body: JSON.stringify({
      host_id: targetHost.value || hostId.value, metric: metric.value.trim(),
      comparison: comparison.value, threshold: Number(threshold.value), duration_s: Number(duration.value)
    }) })
    await load()
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { busy.value = false }
}

async function toggle(rule: AlertRule) {
  busy.value = true
  try {
    await request(`/api/v1/alerts/${encodeURIComponent(rule.id)}`, {
      method: 'PATCH', body: JSON.stringify({ enabled: !rule.enabled })
    })
    await load()
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { busy.value = false }
}

watch(hostId, (id) => { if (!targetHost.value) targetHost.value = id }, { immediate: true })
onMounted(() => { load(); timer = window.setInterval(load, 20_000) })
onUnmounted(() => { if (timer) window.clearInterval(timer) })
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><h2>告警中心</h2><p>按指标或场景分设置阈值；条件持续满足后触发，恢复时自动关闭事件。</p></div><span class="subtle-meta">当前节点 {{ activeEvents }} 个进行中事件</span></div>
    <div v-if="error" class="notice error">{{ error }}</div>
    <section class="panel"><div class="panel-header"><div><h3>创建阈值规则</h3><p>评分指标示例：score.general、score.storage</p></div><Bell :size="19" /></div>
      <div class="form-grid alert-form"><label>目标节点<select v-model="targetHost"><option v-for="host in hosts" :key="host.id" :value="host.id">{{ host.hostname }}</option><option value="*">所有节点</option></select></label><label>指标名<input v-model="metric" placeholder="cpu.runnable_per_core" /></label><label>条件<select v-model="comparison"><option value="above">高于</option><option value="below">低于</option></select></label><label>阈值<input v-model.number="threshold" type="number" step="any" /></label><label>持续时间（秒）<input v-model.number="duration" type="number" min="0" max="3600" /></label><button class="button primary" :disabled="role === 'viewer' || busy || !targetHost" @click="create"><Plus :size="16" /> 添加规则</button></div>
      <div v-if="role === 'viewer'" class="inline-info"><ShieldAlert :size="16" /> 当前账号为只读，需要操作员或管理员权限才能修改规则。</div>
    </section>
    <section class="panel"><div class="panel-header"><div><h3>规则列表</h3><p>{{ rules.length }} 条规则</p></div><div class="search-field"><Search :size="16" /><input v-model="filter" placeholder="搜索指标或节点" /></div></div><div class="table-scroll"><table class="data-table"><thead><tr><th>指标</th><th>目标</th><th>条件</th><th>持续时间</th><th>状态</th><th>操作</th></tr></thead><tbody>
      <tr v-for="rule in filteredRules" :key="rule.id"><td><strong>{{ metricTitle(rule.metric) }}</strong><small class="mono secondary-text">{{ rule.metric }}</small></td><td>{{ rule.host_id === '*' ? '所有节点' : hosts.find((host) => host.id === rule.host_id)?.hostname || rule.host_id }}</td><td>{{ rule.comparison === 'above' ? '高于' : '低于' }} {{ formatValue(rule.metric, rule.threshold) }}</td><td>{{ rule.duration_s }} 秒</td><td><span class="badge" :class="rule.enabled ? 'badge-success' : 'badge-muted'">{{ rule.enabled ? '启用' : '停用' }}</span></td><td><button v-if="role === 'operator' || role === 'admin'" class="button text" :disabled="busy" @click="toggle(rule)">{{ rule.enabled ? '停用' : '启用' }}</button></td></tr><tr v-if="!filteredRules.length"><td colspan="6" class="table-empty">暂无匹配规则</td></tr>
    </tbody></table></div></section>
    <section class="panel"><div class="panel-header"><div><h3>事件记录</h3><p>当前选中节点 · 最近 200 条</p></div><span class="badge" :class="activeEvents ? 'badge-danger' : 'badge-success'">{{ activeEvents }} 个进行中</span></div><div class="table-scroll"><table class="data-table"><thead><tr><th>指标</th><th>触发值</th><th>触发时间</th><th>恢复时间</th><th>状态</th></tr></thead><tbody>
      <tr v-for="item in filteredEvents" :key="item.id"><td>{{ metricTitle(item.metric) }}<small class="mono secondary-text">{{ item.metric }}</small></td><td>{{ formatValue(item.metric, item.value) }}</td><td>{{ timestamp(item.triggered_ms) }}</td><td>{{ item.resolved_ms ? timestamp(item.resolved_ms) : '—' }}</td><td><span class="badge" :class="item.resolved_ms ? 'badge-muted' : 'badge-danger'">{{ item.resolved_ms ? '已恢复' : '进行中' }}</span></td></tr><tr v-if="!filteredEvents.length"><td colspan="5" class="table-empty">当前节点没有告警记录</td></tr>
    </tbody></table></div></section>
  </div>
</template>
