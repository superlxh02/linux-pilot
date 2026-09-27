<script setup lang="ts">
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from 'vue'
import { storeToRefs } from 'pinia'
import { CircleHelp, Gauge, Info, ShieldCheck } from '@lucide/vue'
import { type Metric, type Score } from '../api'
import { formatValue, metricDetails, metricTitle, timestamp } from '../catalog'
import { usePlatform } from '../store'

const LineChart = defineAsyncComponent(() => import('../components/LineChart.vue'))
const platform = usePlatform()
const { hostId, range, scores } = storeToRefs(platform)
const scenario = ref('general')
const history = ref<Score[]>([])
const error = ref('')
const loading = ref(false)
let timer: number | undefined

const scenarios = [
  { id: 'general', label: '通用', hint: '均衡观察五类资源' },
  { id: 'cpu', label: 'CPU 密集', hint: '强调排队与调度等待' },
  { id: 'application_io', label: '应用 I/O', hint: '强调任务阻塞和主机 I/O 代理' },
  { id: 'storage', label: '存储密集', hint: '强调块请求延迟与队列' },
  { id: 'network', label: '网络密集', hint: '强调重传、丢包与建连' }
]
const dimensions: Record<string, string> = { cpu: 'CPU', memory: '内存', application_io: '应用 I/O', storage: '存储', network: '网络' }
const current = computed(() => scores.value[scenario.value]?.host_id === hostId.value
  ? scores.value[scenario.value] : history.value.at(-1))
const factors = computed(() => [...(current.value?.factors || [])]
  .sort((a, b) => b.penalty * b.weight - a.penalty * a.weight))
const scorePoints = computed<Metric[]>(() => history.value.filter((item) => item.value != null).map((item) => ({
  name: '场景评分', value: item.value!, time_ms: item.time_ms, labels: {}, source: 'score'
})))

async function load() {
  if (!hostId.value) return
  const id = hostId.value
  loading.value = true
  try {
    const result = await platform.queryScores(scenario.value)
    if (id === hostId.value) history.value = result
    error.value = ''
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { loading.value = false }
}
watch([hostId, range, scenario], () => { history.value = []; load() }, { immediate: true })
onMounted(() => { timer = window.setInterval(load, 20_000) })
onUnmounted(() => { if (timer) window.clearInterval(timer) })
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><h2>场景评分</h2><p>规则评分基于最近 60 秒指标。不同业务场景改变维度权重，所有扣分均可追溯。</p></div><span class="subtle-meta">{{ current?.profile_version || 'rules-v1' }}</span></div>
    <div class="scenario-tabs"><button v-for="item in scenarios" :key="item.id" :class="{ active: scenario === item.id }" @click="scenario = item.id"><strong>{{ item.label }}</strong><small>{{ item.hint }}</small></button></div>
    <div v-if="error" class="notice error">{{ error }}</div>
    <div class="score-grid">
      <section class="panel current-score"><div class="panel-header"><div><h3>当前得分</h3><p>{{ scenarios.find((item) => item.id === scenario)?.label }}场景 · 实时更新</p></div><Gauge :size="20" /></div>
        <div class="score-reading"><strong :class="current?.value == null ? 'neutral' : current.value < 60 ? 'danger' : current.value < 80 ? 'warning' : 'healthy'">{{ current?.value == null ? '—' : current.value.toFixed(0) }}</strong><span>/ 100</span></div>
        <p class="score-caption">{{ current?.value == null ? '数据覆盖不足，暂不计算总分' : '当前指标窗口的性能退化风险' }}</p>
        <div class="coverage-line"><span>指标覆盖率</span><strong>{{ current ? (current.coverage * 100).toFixed(0) : '—' }}%</strong></div><div class="progress-track"><i :style="{ width: `${(current?.coverage || 0) * 100}%` }"></i></div>
        <div class="score-note"><Info :size="15" /> 分数反映当前压力与退化信号，不用于跨硬件绝对性能排名。</div>
      </section>
      <section class="panel"><div class="panel-header"><div><h3>资源维度</h3><p>当前场景下的分项结果</p></div></div>
        <div class="dimension-list"><div v-for="(value, name) in current?.dimension_scores || {}" :key="name" class="dimension-item"><div><strong>{{ dimensions[name] || name }}</strong><span>{{ value == null ? '无数据' : `${value.toFixed(0)} 分` }}</span></div><div class="progress-track"><i :style="{ width: `${value || 0}%` }"></i></div></div></div>
      </section>
    </div>
    <section class="panel"><div class="panel-header"><div><h3>评分趋势</h3><p>每 10 秒保存一次历史分；当前分每次上报更新</p></div><span class="subtle-meta">{{ loading ? '查询中…' : `${history.length} 个历史点` }}</span></div><LineChart :points="scorePoints" :names="['场景评分']" :height="275" /></section>
    <section class="panel"><div class="panel-header"><div><h3>评分依据</h3><p>原始值、健康阈值、严重阈值与加权扣分</p></div><ShieldCheck :size="19" /></div>
      <div class="table-scroll"><table class="data-table"><thead><tr><th>指标</th><th>维度</th><th class="align-right">观测值</th><th class="align-right">健康阈值</th><th class="align-right">严重阈值</th><th class="align-right">影响</th></tr></thead><tbody>
        <tr v-for="factor in factors" :key="`${factor.dimension}-${factor.metric}`"><td><strong>{{ metricTitle(factor.metric) }}</strong><small class="mono secondary-text">{{ factor.metric }}</small></td><td>{{ dimensions[factor.dimension] || factor.dimension }}</td><td class="align-right">{{ formatValue(factor.metric, factor.value, 2) }}</td><td class="align-right">{{ formatValue(factor.metric, factor.good) }}</td><td class="align-right">{{ formatValue(factor.metric, factor.bad) }}</td><td class="align-right"><span :class="factor.penalty > 0 ? 'text-danger' : 'text-success'">{{ factor.penalty > 0 ? `−${(factor.penalty * factor.weight * 100).toFixed(1)}` : '正常' }}</span></td></tr>
        <tr v-if="!factors.length"><td colspan="6" class="table-empty">等待评分输入</td></tr>
      </tbody></table></div>
    </section>
    <section v-if="current?.missing.length" class="panel missing-panel"><div class="panel-header"><div><h3>未覆盖指标</h3><p>这些指标没有有效采样，评分不会把它们当成 0 或满分。</p></div><CircleHelp :size="18" /></div><div class="missing-tags"><span v-for="name in current.missing" :key="name" :title="metricDetails(name)?.meaning || name">{{ metricTitle(name) }} <small>{{ name }}</small></span></div></section>
    <div class="subtle-meta">{{ current ? `最近计算：${timestamp(current.time_ms)}` : '等待 Worker 上报' }}</div>
  </div>
</template>
