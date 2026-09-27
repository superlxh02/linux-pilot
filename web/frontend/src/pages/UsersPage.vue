<script setup lang="ts">
/**
 * 管理员账号视图。页面只显示公开资料；权限校验始终由后端会话完成。
 * 每次角色变更后重取列表和活动记录，不在浏览器里猜测数据库状态。
 */
import { onMounted, ref } from 'vue'
import { Clock3, RefreshCw, ShieldCheck, UsersRound } from '@lucide/vue'
import { request } from '../api'
import { timestamp } from '../catalog'

interface ManagedUser {
  id: string
  username: string | null
  email: string
  display_name: string
  role: 'viewer' | 'operator' | 'admin'
  created_ms: number
  last_login_ms: number | null
}
interface Activity {
  id: string
  action: string
  target: string
  actor_name: string | null
  details: Record<string, unknown>
  created_ms: number
}

const users = ref<ManagedUser[]>([])
const activity = ref<Activity[]>([])
const selected = ref<ManagedUser | null>(null)
const loading = ref(false)
const savingId = ref('')
const error = ref('')
const notice = ref('')
const offset = ref(0)
const pageSize = 30

const roles: Record<ManagedUser['role'], string> = {
  viewer: '只读', operator: '操作员', admin: '管理员'
}
const actions: Record<string, string> = {
  'auth.register': '注册账号',
  'auth.login': '密码登录',
  'auth.login.oauth': '第三方登录',
  'auth.logout': '退出登录',
  'admin.role.update': '调整用户权限',
  'profile.start': '启动性能剖析',
  'process.watch': '固定监控进程',
  'process.unwatch': '取消进程监控',
  'alert.create': '创建告警规则',
  'alert.update': '修改告警规则'
}

async function loadUsers() {
  loading.value = true
  error.value = ''
  try {
    users.value = await request<ManagedUser[]>(`/api/v1/admin/users?limit=${pageSize}&offset=${offset.value}`)
    if (selected.value) selected.value = users.value.find((user) => user.id === selected.value?.id) || null
    if (!selected.value && users.value.length) await selectUser(users.value[0])
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { loading.value = false }
}

async function selectUser(user: ManagedUser) {
  selected.value = user
  activity.value = []
  try { activity.value = await request<Activity[]>(`/api/v1/admin/users/${encodeURIComponent(user.id)}/activity`) }
  catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}

async function changeRole(user: ManagedUser, event: Event) {
  const select = event.target as HTMLSelectElement
  const role = select.value as ManagedUser['role']
  if (role === user.role) return
  savingId.value = user.id
  error.value = ''
  notice.value = ''
  try {
    await request(`/api/v1/admin/users/${encodeURIComponent(user.id)}/role`, {
      method: 'PATCH', body: JSON.stringify({ role })
    })
    notice.value = `${user.display_name} 的权限已改为${roles[role]}`
    await loadUsers()
    if (selected.value?.id === user.id) await selectUser(selected.value)
  } catch (cause) {
    select.value = user.role
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally { savingId.value = '' }
}

async function changePage(direction: -1 | 1) {
  offset.value = Math.max(0, offset.value + direction * pageSize)
  selected.value = null
  await loadUsers()
}

onMounted(loadUsers)
</script>

<template>
  <div class="page-stack">
    <div class="page-intro"><div><h2>用户管理</h2><p>查看注册账号、分配平台角色并追踪每个账号的操作记录。</p></div><button class="button subtle" :disabled="loading" @click="loadUsers"><RefreshCw :size="16" /> 刷新</button></div>
    <div v-if="error" class="notice error">{{ error }}</div>
    <div v-if="notice" class="notice success">{{ notice }}</div>
    <div class="admin-summary"><span><UsersRound :size="19" /> 当前页 {{ users.length }} 个账号</span><span><ShieldCheck :size="19" /> 角色变更立即生效</span><span><Clock3 :size="19" /> 展示最近 100 条账号操作</span></div>
    <div class="admin-layout">
      <section class="panel admin-users-panel">
        <div class="panel-header"><div><h3>账号列表</h3><p>新注册账号默认为只读，内置 admin 账号不能在此修改。</p></div></div>
        <div class="admin-table-wrap"><table class="data-table"><thead><tr><th>用户</th><th>注册时间</th><th>最近登录</th><th>权限</th></tr></thead>
          <tbody>
            <tr v-for="user in users" :key="user.id" :class="{ 'admin-selected': selected?.id === user.id }" @click="selectUser(user)">
              <td><strong>{{ user.display_name }}</strong><small class="admin-secondary">{{ user.username || user.email }}</small></td>
              <td>{{ timestamp(user.created_ms) }}</td>
              <td>{{ user.last_login_ms ? timestamp(user.last_login_ms) : '尚未登录' }}</td>
              <td><span v-if="user.username === 'admin'" class="badge">管理员</span><select v-else class="role-select" :value="user.role" :disabled="savingId === user.id" :aria-label="`修改 ${user.display_name} 的权限`" @click.stop @change="changeRole(user, $event)"><option value="viewer">只读</option><option value="operator">操作员</option><option value="admin">管理员</option></select></td>
            </tr>
            <tr v-if="!users.length"><td colspan="4" class="table-empty">{{ loading ? '正在加载账号…' : '没有账号记录' }}</td></tr>
          </tbody></table></div>
        <div class="admin-pagination"><span>第 {{ Math.floor(offset / pageSize) + 1 }} 页</span><div><button class="button subtle" :disabled="offset === 0 || loading" @click="changePage(-1)">上一页</button><button class="button subtle" :disabled="users.length < pageSize || loading" @click="changePage(1)">下一页</button></div></div>
      </section>
      <section class="panel admin-activity-panel">
        <div class="panel-header"><div><h3>操作记录</h3><p>{{ selected ? `${selected.display_name} · ${roles[selected.role]}` : '选择左侧用户查看' }}</p></div></div>
        <div v-if="!activity.length" class="quiet-state"><Clock3 :size="23" /><span>当前账号没有可查看的操作记录</span></div>
        <div v-for="entry in activity" :key="entry.id" class="admin-activity-row"><span class="admin-activity-dot"></span><div><strong>{{ actions[entry.action] || entry.action }}</strong><small>{{ timestamp(entry.created_ms) }} · {{ entry.actor_name || '系统' }}</small><code v-if="entry.target && entry.target !== selected?.id">{{ entry.target }}</code><span v-if="entry.action === 'admin.role.update' && entry.details.to">{{ entry.details.from }} → {{ entry.details.to }}</span></div></div>
      </section>
    </div>
  </div>
</template>
