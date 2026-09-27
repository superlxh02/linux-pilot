<script setup lang="ts">
import { computed, defineAsyncComponent, onMounted, onUnmounted, ref, watch } from 'vue'
import { storeToRefs } from 'pinia'
import { Download, Info, Search } from '@lucide/vue'
import { request, type Metric } from '../api'
import { categories, formatValue, metricDetails, metricTitle, timestamp } from '../catalog'
import { usePlatform } from '../store'

const LineChart = defineAsyncComponent(() => import('../components/LineChart.vue'))
const platform = usePlatform()
const { hostId, range, latest } = storeToRefs(platform)
const category = ref<(typeof categories)[number]['id']>('cpu')
const query = ref('')
const chartMetric = ref<string>('cpu.busy_pct')
const chartPoints = ref<Metric[]>([])
const rawPoints = ref<Metric[]>([])
const loading = ref(false)
const error = ref('')
const page = ref(1)
let timer: number | undefined
const selected = computed(() => categories.find((item) => item.id === category.value) || categories[0])
const latestRows = computed(() => {
  const map = new Map<string, Metric>()
  for (const point of rawPoints.value) {
    const identity = `${point.name}|${JSON.stringify(point.labels)}`
    if (!map.has(identity) || map.get(identity)!.time_ms < point.time_ms) map.set(identity, point)
  }
  return [...map.values()].filter((point) =>
    `${point.name} ${metricTitle(point.name)} ${JSON.stringify(point.labels)}`.toLowerCase().includes(query.value.toLowerCase()))
    .sort((a, b) => a.name.localeCompare(b.name) || JSON.stringify(a.labels).localeCompare(JSON.stringify(b.labels)))
})
const pageCount = computed(() => Math.max(1, Math.ceil(latestRows.value.length / 40)))
const visibleRows = computed(() => latestRows.value.slice((page.value - 1) * 40, page.value * 40))
const chartNames = computed(() => [...new Set(chartPoints.value.map((point) => point.name))].sort())

async function load() {
  if (!hostId.value) return
  const id = hostId.value
  loading.value = true
  error.value = ''
  try {
    const now = Date.now()
    const params = new URLSearchParams({ host_id: id, category: category.value,
      from: String(now - 15_000), to: String(now), limit: '10000' })
    const [chart, raw] = await Promise.all([platform.queryMetrics(category.value), request<Metric[]>(`/api/v1/metrics?${params}`)])
    if (id !== hostId.value) return
    chartPoints.value = chart
    rawPoints.value = raw
    if (!chartNames.value.includes(chartMetric.value)) chartMetric.value = selected.value.chart[0]
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { loading.value = false }
}

function labelsOf(point: Metric) {
  const labels = Object.entries(point.labels)
  return labels.length ? labels.map(([key, value]) => `${key}=${value}`).join(' · ') : '主机汇总'
}

function csvField(value: string) { return `"${value.replaceAll('"', '""')}"` }
function exportCsv() {
  const rows = [['指标名','指标含义','数值','单位','维度','来源','时间'].join(',')]
  for (const point of latestRows.value) {
    rows.push([point.name, metricDetails(point.name)?.meaning || '', String(point.value),
      metricDetails(point.name)?.unit || '', labelsOf(point), point.source, timestamp(point.time_ms)]
      .map(csvField).join(','))
  }
  const url = URL.createObjectURL(new Blob(['\ufeff', rows.join('\n')], { type: 'text/csv;charset=utf-8' }))
  const link = document.createElement('a')
  link.href = url; link.download = `linux-pilot-${category.value}-${Date.now()}.csv`; link.click()
  window.setTimeout(() => URL.revokeObjectURL(url), 1000)
}

watch(category, () => { chartMetric.value = selected.value.chart[0]; page.value = 1 })
watch(query, () => { page.value = 1 })
watch([hostId, range, category], load, { immediate: true })
onMounted(() => { timer = window.setInterval(load, 20_000) })
onUnmounted(() => { if (timer) window.clearInterval(timer) })
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><h2>指标探索</h2><p>按资源类别筛选时序与当前样本；每条指标展示口径、来源和维度。</p></div><button class="button secondary" :disabled="!latestRows.length" @click="exportCsv"><Download :size="16" /> 导出当前列表</button></div>
    <div v-if="error" class="notice error">{{ error }}</div>
    <div class="category-grid"><button v-for="item in categories" :key="item.id" class="category-tab" :class="{ active: category === item.id }" @click="category = item.id"><strong>{{ item.title }}</strong><small>{{ item.summary }}</small></button></div>
    <section class="panel"><div class="panel-header"><div><h3>{{ selected.title }}趋势</h3><p>主机级数据在数据库端聚合为约 180 个时间桶</p></div><select v-model="chartMetric" aria-label="图表指标"><option v-for="name in chartNames" :key="name" :value="name">{{ metricTitle(name) }}</option></select></div>
      <LineChart :points="chartPoints" :names="[chartMetric]" :height="310" />
      <div v-if="!chartPoints.length" class="inline-empty">当前类别暂无主机级时序。进程、设备或 cgroup 数据可在下表按维度查看。</div>
    </section>
    <section class="panel"><div class="panel-header"><div><h3>最新样本</h3><p>{{ latestRows.length }} 个指标与维度组合 · {{ loading ? '正在更新' : '最近 15 秒' }}</p></div><div class="search-field"><Search :size="16" /><input v-model="query" placeholder="搜索名称、含义或标签" /></div></div>
      <div class="table-scroll"><table class="data-table"><thead><tr><th>指标</th><th>当前值</th><th>维度</th><th>来源</th><th>时间</th></tr></thead><tbody>
        <tr v-for="point in visibleRows" :key="`${point.name}-${JSON.stringify(point.labels)}`"><td class="metric-name"><strong>{{ metricTitle(point.name) }}</strong><small class="mono">{{ point.name }}</small><span :title="metricDetails(point.name)?.meaning || '详见指标字典'" class="description"><Info :size="13" /> {{ metricDetails(point.name)?.meaning || '详见项目指标字典' }}</span></td><td class="number-cell">{{ formatValue(point.name, point.value, 2) }}</td><td class="labels-cell">{{ labelsOf(point) }}</td><td><span class="source-pill">{{ point.source }}</span></td><td class="secondary-text">{{ timestamp(point.time_ms) }}</td></tr>
        <tr v-if="!visibleRows.length"><td colspan="5" class="table-empty">{{ hostId ? '此时间窗没有匹配的指标' : '请先选择节点' }}</td></tr>
      </tbody></table></div>
      <div class="table-pagination"><span>第 {{ page }} / {{ pageCount }} 页</span><div><button class="button text" :disabled="page <= 1" @click="page--">上一页</button><button class="button text" :disabled="page >= pageCount" @click="page++">下一页</button></div></div>
    </section>
  </div>
</template>
