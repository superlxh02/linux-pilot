<script setup lang="ts">
import * as echarts from 'echarts/core'
import { LineChart } from 'echarts/charts'
import { GridComponent, LegendComponent, TooltipComponent } from 'echarts/components'
import { CanvasRenderer } from 'echarts/renderers'
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { Metric } from '../api'
import { formatValue, metricTitle } from '../catalog'

const props = defineProps<{ points: Metric[]; names: string[]; height?: number }>()
const element = ref<HTMLElement | null>(null)
let chart: echarts.ECharts | null = null
let observer: ResizeObserver | null = null

// 只注册折线图所需模块，避免将 ECharts 全量图表打进首屏包。
echarts.use([LineChart, GridComponent, LegendComponent, TooltipComponent, CanvasRenderer])

function render() {
  if (!chart) return
  const series = props.names.map((name) => ({
    name: metricTitle(name),
    type: 'line' as const,
    smooth: true,
    showSymbol: false,
    // 数据契约继续使用稳定的英文键；面向用户的图例、悬浮值统一走
    // 中文指标字典，避免总览图露出 cpu.busy_pct 这样的实现字段。
    tooltip: { valueFormatter: (value: unknown) => formatValue(name, Number(Array.isArray(value) ? value[1] : value)) },
    data: props.points.filter((point) => point.name === name && !Object.keys(point.labels).length)
      .map((point) => [point.time_ms, point.value])
  }))
  chart.setOption({
    backgroundColor: 'transparent',
    color: ['#2c67d8', '#42a88a', '#e0a24e', '#8d72cc', '#d97278'],
    tooltip: { trigger: 'axis', backgroundColor: '#ffffff', borderColor: '#dfe6ef', textStyle: { color: '#34445b' } },
    legend: { data: props.names.map(metricTitle), textStyle: { color: '#66758c', fontSize: 12 }, top: 0 },
    grid: { left: 47, right: 19, top: 42, bottom: 30 },
    xAxis: { type: 'time', axisLine: { lineStyle: { color: '#dfe6ee' } }, axisLabel: { color: '#a0adbd' }, splitLine: { show: false } },
    yAxis: { type: 'value', axisLine: { show: false }, axisLabel: { color: '#a0adbd', formatter: (value: number) => formatValue(props.names[0] || '', value, 0) }, splitLine: { lineStyle: { color: '#eef2f6' } } },
    series
  }, true)
}
onMounted(() => {
  if (!element.value) return
  chart = echarts.init(element.value)
  observer = new ResizeObserver(() => chart?.resize())
  observer.observe(element.value)
  render()
})
watch(() => [props.points, props.names], render, { deep: true })
onBeforeUnmount(() => { observer?.disconnect(); chart?.dispose() })
</script>

<template>
  <div ref="element" class="chart" :style="{ height: `${height || 310}px` }"></div>
</template>
