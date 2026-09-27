<script setup lang="ts">
import { computed, ref } from 'vue'
import { storeToRefs } from 'pinia'
import { CheckCircle2, Search, Server, WifiOff } from '@lucide/vue'
import { useRouter } from 'vue-router'
import { formatValue, timestamp } from '../catalog'
import { usePlatform } from '../store'

const router = useRouter()
const platform = usePlatform()
const { visibleHosts, hostId, deploymentMode } = storeToRefs(platform)
const search = ref('')
const filter = ref<'all' | 'online' | 'offline'>('all')
const rows = computed(() => visibleHosts.value.filter((host) =>
  (filter.value === 'all' || (filter.value === 'online') === host.online)
  && `${host.hostname} ${host.id}`.toLowerCase().includes(search.value.toLowerCase())))
const onlineCount = computed(() => visibleHosts.value.filter((host) => host.online).length)
function openHost(id: string) { platform.setHost(id); router.push(deploymentMode.value === 'kubernetes' ? '/node-overview' : '/overview') }
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><h2>{{ deploymentMode === 'kubernetes' ? 'Kubernetes 节点' : '主机节点' }}</h2><p>仅列出当前模式拓扑配置中的节点。一个 Kubernetes Node 可以承载多个服务的 Pod。</p></div><button class="button secondary" @click="platform.loadHosts">刷新节点</button></div>
    <div class="summary-grid three">
      <div class="summary-card"><span>当前模式节点</span><strong>{{ visibleHosts.length }}</strong><Server :size="21" /></div>
      <div class="summary-card"><span>在线</span><strong class="text-success">{{ onlineCount }}</strong><CheckCircle2 :size="21" /></div>
      <div class="summary-card"><span>离线</span><strong :class="visibleHosts.length - onlineCount ? 'text-danger' : ''">{{ visibleHosts.length - onlineCount }}</strong><WifiOff :size="21" /></div>
    </div>
    <section class="panel">
      <div class="panel-header"><div><h3>主机</h3><p>最近 15 秒有采集数据的节点视为在线</p></div><div class="table-tools"><div class="search-field"><Search :size="16" /><input v-model="search" placeholder="搜索主机名或 ID" /></div><select v-model="filter"><option value="all">全部状态</option><option value="online">在线</option><option value="offline">离线</option></select></div></div>
      <div class="table-scroll"><table class="data-table hosts-table"><thead><tr><th>节点</th><th>状态</th><th class="align-right">CPU</th><th class="align-right">内存</th><th class="align-right">综合分</th><th>能力</th><th>最近上报</th></tr></thead><tbody>
        <tr v-for="host in rows" :key="host.id" class="clickable-row" :class="{ 'selected-row': host.id === hostId }" @click="openHost(host.id)">
          <td><strong>{{ host.hostname }}</strong><small class="mono secondary-text">{{ host.id }}</small></td>
          <td><span class="badge" :class="host.online ? 'badge-success' : 'badge-muted'"><span class="status-dot" :class="{ offline: !host.online }"></span>{{ host.online ? '在线' : '离线' }}</span></td>
          <td class="align-right">{{ formatValue('cpu.busy_pct', host.cpu_pct) }}</td><td class="align-right">{{ formatValue('mem.used_pct', host.mem_pct) }}</td>
          <td class="align-right"><strong>{{ host.health_score == null ? '—' : host.health_score.toFixed(0) }}</strong></td>
          <td><div class="capability-tags"><span :class="{ unavailable: !host.capabilities.ebpf }">eBPF</span><span :class="{ unavailable: !host.capabilities.perf }">perf</span><span :class="{ unavailable: !host.capabilities.cgroup_v2 }">cgroup v2</span></div></td>
          <td class="secondary-text">{{ timestamp(host.last_seen_ms) }}</td>
        </tr><tr v-if="!rows.length"><td colspan="7" class="table-empty">{{ visibleHosts.length ? '没有匹配的节点' : '等待 Worker 接入' }}</td></tr>
      </tbody></table></div>
    </section>
  </div>
</template>
