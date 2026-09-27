<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{ folded: string }>()
interface Node { name: string; count: number; children: Map<string, Node> }
interface Rect { x: number; y: number; width: number; name: string; count: number; color: string }
const width = 1000
const rowHeight = 25
const graph = computed(() => {
  const root: Node = { name: 'root', count: 0, children: new Map() }
  for (const line of props.folded.split('\n')) {
    const split = line.lastIndexOf(' ')
    if (split < 0) continue
    const count = Number(line.slice(split + 1))
    if (!Number.isFinite(count) || count <= 0) continue
    const frames = line.slice(0, split).split(';')
    root.count += count
    let node = root
    for (const name of frames) {
      if (!node.children.has(name)) node.children.set(name, { name, count: 0, children: new Map() })
      node = node.children.get(name)!
      node.count += count
    }
  }
  const rectangles: Rect[] = []
  let maxDepth = 0
  function visit(node: Node, x: number, y: number, allocated: number) {
    let cursor = x
    maxDepth = Math.max(maxDepth, y)
    for (const child of [...node.children.values()].sort((a, b) => b.count - a.count)) {
      const itemWidth = allocated * child.count / Math.max(node.count, 1)
      const hue = [...child.name].reduce((hash, char) => hash + char.charCodeAt(0), 0) % 55 + 190
      rectangles.push({ x: cursor, y, width: itemWidth, name: child.name, count: child.count, color: `hsl(${hue} 62% 70%)` })
      visit(child, cursor, y + 1, itemWidth)
      cursor += itemWidth
    }
  }
  visit(root, 0, 0, width)
  return { rectangles, height: (maxDepth + 1) * rowHeight + 8, samples: root.count }
})
</script>

<template>
  <div class="flame-scroll">
    <svg :viewBox="`0 0 ${width} ${graph.height}`" role="img" aria-label="CPU 火焰图">
      <g v-for="(rect, index) in graph.rectangles" :key="index">
        <rect :x="rect.x" :y="graph.height - (rect.y + 1) * rowHeight"
          :width="Math.max(rect.width - 1, 0)" :height="rowHeight - 2"
          :fill="rect.color" rx="2">
          <title>{{ rect.name }} · {{ rect.count }} 样本</title>
        </rect>
        <text v-if="rect.width > 65" :x="rect.x + 5"
          :y="graph.height - (rect.y + 1) * rowHeight + 16"
          fill="#24415f" font-size="11">{{ rect.name.slice(0, Math.floor(rect.width / 8)) }}</text>
      </g>
    </svg>
  </div>
</template>
