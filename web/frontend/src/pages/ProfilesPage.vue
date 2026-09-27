<script setup lang="ts">
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from 'vue'
import { storeToRefs } from 'pinia'
import { Activity, Play, ShieldAlert } from '@lucide/vue'
import { request, type Metric, type ProfileJob } from '../api'
import { timestamp } from '../catalog'
import { usePlatform } from '../store'

const FlameGraph = defineAsyncComponent(() => import('../components/FlameGraph.vue'))
const platform = usePlatform()
const { hostId, role, selectedHost } = storeToRefs(platform)
// PID 1 往往是空闲的 init 进程，不能作为默认剖析对象。让用户从当前
// 节点近期的 CPU 热点进程中选取，也允许手动输入其他宿主机 PID。
const pid = ref<number | null>(null)
const duration = ref(15)
const frequency = ref(49)
const processes = ref<{ pid: number; name: string; cpu: number }[]>([])
const job = ref<ProfileJob | null>(null)
const jobs = ref<ProfileJob[]>([])
const busy = ref(false)
const error = ref('')
let timer: number | undefined
const canStart = computed(() => (role.value === 'operator' || role.value === 'admin') && !!selectedHost.value?.online && !!selectedHost.value?.capabilities.perf && Number.isInteger(pid.value) && Number(pid.value) > 0)

async function loadProcesses() {
  const id = hostId.value
  processes.value = []
  if (!id) return
  try {
    const now = Date.now()
    const params = new URLSearchParams({ host_id: id, category: 'proc', from: String(now - 15_000), to: String(now), limit: '6000' })
    const metrics = await request<Metric[]>(`/api/v1/metrics?${params}`)
    if (id !== hostId.value) return
    // 同一 PID 在查询窗口内有多轮采样，只取最新一次 CPU 值。旧样本会
    // 误导用户选择已经退出或已经不再繁忙的进程。
    const latest = new Map<number, { pid: number; name: string; cpu: number; time: number }>()
    for (const metric of metrics) {
      if (metric.name !== 'proc.cpu_pct') continue
      const processId = Number(metric.labels.pid)
      if (!Number.isInteger(processId) || processId <= 0) continue
      const previous = latest.get(processId)
      if (!previous || metric.time_ms > previous.time) {
        latest.set(processId, { pid: processId, name: metric.labels.comm || `PID ${processId}`, cpu: metric.value, time: metric.time_ms })
      }
    }
    processes.value = [...latest.values()].sort((a, b) => b.cpu - a.cpu).slice(0, 20)
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}
const hotspots = computed(() => {
  const counts = new Map<string, number>()
  for (const line of (job.value?.folded || '').split('\n')) {
    const split = line.lastIndexOf(' ')
    if (split < 0) continue
    const count = Number(line.slice(split + 1))
    const functionName = line.slice(0, split).split(';').at(-1)
    if (!functionName || !Number.isFinite(count)) continue
    counts.set(functionName, (counts.get(functionName) || 0) + count)
  }
  const total = job.value?.sample_count || 0
  return [...counts].sort((a, b) => b[1] - a[1]).slice(0, 12)
    .map(([name, count]) => ({ name, count, share: total ? count / total * 100 : 0 }))
})

async function refreshJob() {
  const id = job.value?.id
  if (!id) return
  try { job.value = await request<ProfileJob>(`/api/v1/profiles/${encodeURIComponent(id)}`) }
  catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}

async function loadJobs() {
  const id = hostId.value
  if (!id) { jobs.value = []; job.value = null; return }
  try {
    const result = await request<ProfileJob[]>(`/api/v1/profiles?host_id=${encodeURIComponent(id)}`)
    if (id !== hostId.value) return
    jobs.value = result
    if (job.value?.host_id !== id) job.value = null
    if (!job.value && result.length) await selectJob(result[0].id)
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}

async function selectJob(id: string) {
  try { job.value = await request<ProfileJob>(`/api/v1/profiles/${encodeURIComponent(id)}`) }
  catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}

async function start() {
  if (!hostId.value) return
  busy.value = true
  error.value = ''
  try {
    const result = await request<{ id: string; status: string }>('/api/v1/profiles', {
      method: 'POST', body: JSON.stringify({ host_id: hostId.value, pid: pid.value,
        duration_s: duration.value, frequency_hz: frequency.value })
    })
    await selectJob(result.id)
    await loadJobs()
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { busy.value = false }
}

watch(hostId, () => { pid.value = null; void loadJobs(); void loadProcesses() }, { immediate: true })
onMounted(() => { timer = window.setInterval(() => { if (job.value?.status === 'running') void refreshJob(); void loadJobs() }, 2500) })
onUnmounted(() => { if (timer) window.clearInterval(timer) })
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><h2>性能剖析</h2><p>对指定进程执行短时 perf CPU 采样。任务由 Worker 在目标 Linux 节点运行，结果以折叠调用栈呈现。</p></div></div>
    <div v-if="error" class="notice error">{{ error }}</div>
    <section class="panel"><div class="panel-header"><div><h3>新建 CPU 采样</h3><p>选择当前有 CPU 活动的进程；单节点同时运行一个任务，最长 60 秒</p></div><Activity :size="19" /></div>
      <div class="profile-process-picker"><label>从近期 CPU 热点进程选择<select v-model.number="pid"><option :value="null">请选择进程，或在下方输入 PID</option><option v-for="item in processes" :key="item.pid" :value="item.pid">{{ item.name }} · PID {{ item.pid }} · CPU {{ item.cpu.toFixed(1) }}%</option></select></label><button class="button secondary" type="button" @click="loadProcesses">刷新进程</button></div>
      <div class="form-grid profile-form"><label>目标 PID<input v-model.number="pid" type="number" min="1" step="1" /></label><label>持续时间（秒）<input v-model.number="duration" type="number" min="1" max="60" step="1" /></label><label>采样频率（Hz）<input v-model.number="frequency" type="number" min="1" max="199" step="1" /></label><button class="button primary" :disabled="!canStart || busy" @click="start"><Play :size="16" /> {{ busy ? '提交中…' : '开始采样' }}</button></div>
      <p class="secondary-text">PID 属于所选 Worker 的 Linux 主机。采样期间进程需要持续使用 CPU；空闲进程不会产生火焰图。</p>
      <div v-if="role === 'viewer'" class="inline-info"><ShieldAlert :size="16" /> 当前账号为只读，需要操作员或管理员权限才能执行 perf。</div>
      <div v-else-if="selectedHost && !selectedHost.capabilities.perf" class="inline-info"><ShieldAlert :size="16" /> 目标节点未检测到 perf 命令。命令可用仍需内核权限支持，失败原因会显示在任务结果中。</div>
    </section>
    <section class="panel"><div class="panel-header"><div><h3>最近任务</h3><p>当前节点最近 100 次任务；选择一项查看采样结果</p></div><button v-if="job" class="button text" @click="loadJobs">刷新</button></div>
      <div v-if="!job" class="quiet-state">当前节点尚无性能剖析任务</div>
      <template v-else><div class="job-summary"><div><span>任务 ID</span><strong class="mono">{{ job.id }}</strong></div><div><span>目标 PID</span><strong>{{ job.pid }}</strong></div><div><span>采样参数</span><strong>{{ job.duration_s }} s · {{ job.frequency_hz }} Hz</strong></div><div><span>状态</span><strong><span class="badge" :class="job.status === 'completed' ? 'badge-success' : job.status === 'failed' ? 'badge-danger' : 'badge-warning'">{{ job.status === 'completed' ? '已完成' : job.status === 'failed' ? '失败' : '采样中' }}</span></strong></div></div>
        <div v-if="job.error" class="notice error">{{ job.error }}</div><p class="secondary-text">创建于 {{ timestamp(job.created_ms) }} · 有效样本 {{ job.sample_count ?? '—' }}</p></template>
      <div v-if="jobs.length" class="table-scroll"><table class="data-table"><thead><tr><th>创建时间</th><th>PID</th><th>时长</th><th>状态</th><th class="align-right">样本</th></tr></thead><tbody><tr v-for="item in jobs" :key="item.id" :class="{ 'selected-row': item.id === job?.id }" @click="selectJob(item.id)" style="cursor:pointer"><td>{{ timestamp(item.created_ms) }}</td><td class="mono">{{ item.pid }}</td><td>{{ item.duration_s }} s</td><td>{{ item.status === 'completed' ? '已完成' : item.status === 'failed' ? '失败' : '采样中' }}</td><td class="align-right">{{ item.sample_count ?? '—' }}</td></tr></tbody></table></div>
    </section>
    <section v-if="job?.folded" class="panel"><div class="panel-header"><div><h3>CPU 火焰图</h3><p>每个矩形宽度代表对应调用栈出现的样本占比；悬停查看函数。</p></div></div><FlameGraph :folded="job.folded" /></section>
    <section v-if="hotspots.length" class="panel"><div class="panel-header"><div><h3>热点函数</h3><p>按样本数排序；占比为统计样本比例，不等同精确耗时</p></div></div><div class="table-scroll"><table class="data-table"><thead><tr><th>函数 / 符号</th><th class="align-right">样本</th><th class="align-right">占比</th></tr></thead><tbody><tr v-for="item in hotspots" :key="item.name"><td class="mono">{{ item.name }}</td><td class="align-right">{{ item.count }}</td><td class="align-right">{{ item.share.toFixed(1) }}%</td></tr></tbody></table></div></section>
  </div>
</template>
