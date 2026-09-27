<script setup lang="ts">
/**
 * 顶层布局负责账号入口、页面导航和 15 秒一次的轻量刷新。
 * 各业务页面自行查询对应分类，不把全量指标放进这个根组件。
 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { RouterLink, RouterView, useRoute, useRouter } from 'vue-router'
import { storeToRefs } from 'pinia'
import { Activity, ArrowRight, Bell, ChartNoAxesCombined, CircleHelp, Gauge, LayoutDashboard, LockKeyhole, LogOut, RefreshCw, Server, ShieldCheck, UsersRound, ListTree, Network, Settings2 } from '@lucide/vue'
import { usePlatform } from './store'
import { request } from './api'

const route = useRoute()
const router = useRouter()
const platform = usePlatform()
const { hostId, role, account, connection, range, authenticated, error, deploymentMode, visibleHosts } = storeToRefs(platform)
const authMode = ref<'login' | 'register'>('login')
const authEmail = ref('')
const authPassword = ref('')
const displayName = ref('')
const verificationCode = ref('')
const providers = ref<{ github: boolean, google: boolean, local_mailbox: string | null }>({ github: false, google: false, local_mailbox: null })
const loggingIn = ref(false)
const sendingCode = ref(false)
const resendSeconds = ref(0)
let refreshTimer: number | undefined
let codeTimer: number | undefined

const kubernetesNavigation = [
  { path: '/k8s', label: '集群总览', icon: LayoutDashboard },
  { path: '/services', label: '微服务', icon: Network },
  { path: '/nodes', label: 'Kubernetes 节点', icon: Server },
  { path: '/node-overview', label: '节点详情', icon: Gauge },
  { path: '/metrics', label: '节点指标', icon: Activity },
  { path: '/alerts', label: '告警', icon: Bell },
  { path: '/topology', label: '拓扑配置', icon: Settings2 }
]
const standaloneNavigation = [
  { path: '/standalone', label: '集群总览', icon: LayoutDashboard },
  { path: '/hosts', label: '主机节点', icon: Server },
  { path: '/overview', label: '节点详情', icon: Gauge },
  { path: '/metrics', label: '节点指标', icon: Activity },
  { path: '/processes', label: '进程监控', icon: ListTree },
  { path: '/scores', label: '场景评分', icon: Gauge },
  { path: '/profiles', label: '性能剖析', icon: ChartNoAxesCombined },
  { path: '/alerts', label: '告警', icon: Bell },
  { path: '/topology', label: '拓扑配置', icon: Settings2 }
]
const visibleNavigation = computed(() => {
  const base = deploymentMode.value === 'kubernetes' ? kubernetesNavigation : standaloneNavigation
  return role.value === 'admin' ? [...base, { path: '/users', label: '用户管理', icon: UsersRound }] : base
})
const pageTitle = computed(() => visibleNavigation.value.find((item) => item.path === route.path)?.label || '总览')
const onlineCount = computed(() => visibleHosts.value.filter((host) => host.online).length)
const showNodeSelector = computed(() => ['/overview', '/node-overview', '/metrics', '/processes', '/scores', '/profiles'].includes(route.path))

function selectDeploymentMode(mode: 'kubernetes' | 'standalone') {
  platform.setDeploymentMode(mode)
  void router.push(mode === 'kubernetes' ? '/k8s' : '/standalone')
}

async function submitAccount() {
  // 登录与注册共用表单容器，但注册必须先通过独立邮件通道拿到验证码。
  // 成功后立即清除内存里的明文密码和验证码，再进入观测工作台。
  loggingIn.value = true
  try {
    if (authMode.value === 'login') await platform.login(authEmail.value, authPassword.value)
    else await platform.register(authEmail.value, displayName.value, authPassword.value, verificationCode.value)
    authPassword.value = ''
    verificationCode.value = ''
    await router.replace(deploymentMode.value === 'kubernetes' ? '/k8s' : '/standalone')
  }
  catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { loggingIn.value = false }
}

async function sendCode() {
  // 本地倒计时只减少误触与重复点击；真正的冷却和每小时限额由后端
  // PostgreSQL 原子操作实施，刷新页面也不会绕过服务端限制。
  if (!authEmail.value || sendingCode.value || resendSeconds.value > 0) return
  sendingCode.value = true
  try {
    const result = await platform.sendCode(authEmail.value)
    error.value = ''
    resendSeconds.value = 60
    codeTimer = window.setInterval(() => {
      resendSeconds.value--
      if (resendSeconds.value <= 0 && codeTimer) window.clearInterval(codeTimer)
    }, 1000)
    codeNotice.value = result.message
  } catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
  finally { sendingCode.value = false }
}

const codeNotice = ref('')
function switchMode(mode: 'login' | 'register') {
  authMode.value = mode
  error.value = ''
  codeNotice.value = ''
}

function socialLogin(provider: 'github' | 'google') {
  window.location.assign(`/api/v1/auth/oauth/${provider}/start`)
}

async function signOut() {
  await platform.logout()
  await router.replace('/login')
}

async function refresh() {
  if (!authenticated.value) return
  try { await Promise.all([platform.loadHosts(), platform.refreshOverview()]); error.value = '' }
  catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
}

onMounted(() => {
  void platform.restore().then(() => {
    if (authenticated.value && route.path === '/login') void router.replace(deploymentMode.value === 'kubernetes' ? '/k8s' : '/standalone')
    if (!authenticated.value && route.path !== '/login') void router.replace('/login')
  })
  void request<{ github: boolean, google: boolean, local_mailbox: string | null }>('/api/v1/auth/providers')
    .then((result) => { providers.value = result })
    .catch(() => undefined)
  const reason = route.query.auth_error
  if (reason === 'existing') error.value = '该邮箱已有账号，请使用原登录方式。'
  else if (reason === 'cancelled') error.value = '你已取消第三方授权。'
  else if (reason) error.value = '第三方登录未完成，请重试。'
  refreshTimer = window.setInterval(refresh, 15_000)
})
onUnmounted(() => {
  if (refreshTimer) window.clearInterval(refreshTimer)
  if (codeTimer) window.clearInterval(codeTimer)
})
</script>

<template>
  <div v-if="!authenticated" class="login-page">
    <div class="login-shell">
      <section class="login-story">
        <div class="product-lockup"><div class="product-mark"><img src="/logo.svg" alt="" /></div><div><strong>Linux-Pilot</strong><span>性能观测平台</span></div></div>
        <div class="story-content">
          <span class="story-eyebrow"><span class="live-dot"></span> LINUX PERFORMANCE OBSERVABILITY</span>
          <h1>看清系统每一次<br><em>性能变化。</em></h1>
          <p>从主机指标、内核事件到 CPU 调用栈，在一个工作台中追踪问题，并获得可解释的场景评分。</p>
          <div class="story-features"><span><Activity :size="16" /> 实时指标</span><span><ChartNoAxesCombined :size="16" /> eBPF 与 perf</span><span><Gauge :size="16" /> 场景评分</span></div>
        </div>
        <div class="story-footer">Linux-Pilot · Performance is observable</div>
      </section>
      <section class="login-form-side">
        <div class="login-card">
          <div class="auth-heading"><div class="auth-lock"><LockKeyhole :size="19" /></div><span>安全访问工作台</span></div>
          <h2>{{ authMode === 'login' ? '欢迎回来' : '创建账号' }}</h2>
          <p>{{ authMode === 'login' ? '使用邮箱或管理员账号登录，继续查看你的 Linux 节点。' : '验证邮箱后即可进入平台，开始观察系统状态。' }}</p>
          <div class="auth-tabs" role="tablist" aria-label="账号操作"><button type="button" role="tab" :aria-selected="authMode === 'login'" :class="{ active: authMode === 'login' }" @click="switchMode('login')">登录</button><button type="button" role="tab" :aria-selected="authMode === 'register'" :class="{ active: authMode === 'register' }" @click="switchMode('register')">邮箱注册</button></div>
          <form @submit.prevent="submitAccount">
            <label v-if="authMode === 'register'" for="display-name">昵称</label>
            <input v-if="authMode === 'register'" id="display-name" v-model="displayName" autocomplete="name" placeholder="你的称呼" minlength="2" maxlength="64" required />
            <label for="auth-email">{{ authMode === 'login' ? '邮箱或用户名' : '邮箱地址' }}</label>
            <input id="auth-email" v-model="authEmail" :type="authMode === 'login' ? 'text' : 'email'" :autocomplete="authMode === 'login' ? 'username' : 'email'" :placeholder="authMode === 'login' ? 'admin 或 you@example.com' : 'you@example.com'" required />
            <label for="auth-password">密码</label>
            <input id="auth-password" v-model="authPassword" type="password" :autocomplete="authMode === 'login' ? 'current-password' : 'new-password'" :minlength="authMode === 'register' ? 12 : undefined" :placeholder="authMode === 'register' ? '至少 12 位密码' : '输入密码'" required />
            <template v-if="authMode === 'register'"><label for="auth-code">邮箱验证码</label><div class="code-field"><input id="auth-code" v-model="verificationCode" inputmode="numeric" pattern="[0-9]{6}" maxlength="6" placeholder="6 位验证码" required /><button type="button" :disabled="sendingCode || resendSeconds > 0 || !authEmail" @click="sendCode">{{ sendingCode ? '发送中…' : resendSeconds > 0 ? `${resendSeconds}s 后重发` : '发送验证码' }}</button></div><div v-if="codeNotice" class="auth-hint success">{{ codeNotice }}<a v-if="providers.local_mailbox" :href="providers.local_mailbox" target="_blank" rel="noreferrer">查看本地收件箱</a></div></template>
            <button class="button primary full auth-submit" :disabled="loggingIn">{{ loggingIn ? '请稍候…' : authMode === 'login' ? '登录工作台' : '验证并注册' }} <ArrowRight :size="16" /></button>
          </form>
          <div v-if="error" class="notice error">{{ error }}</div>
          <div class="auth-divider"><span>或者使用以下账号继续</span></div>
          <div class="social-buttons"><button type="button" :disabled="!providers.github" @click="socialLogin('github')"><span class="github-mark">GH</span> GitHub <small v-if="!providers.github">未配置</small></button><button type="button" :disabled="!providers.google" @click="socialLogin('google')"><span class="google-mark">G</span> Google <small v-if="!providers.google">未配置</small></button></div>
          <div class="login-foot"><ShieldCheck :size="15" /> 登录状态保存在安全的 HttpOnly Cookie 中</div>
        </div>
      </section>
    </div>
  </div>

  <div v-else class="app-layout">
    <aside class="sidebar">
      <div class="product-lockup"><div class="product-mark"><img src="/logo.svg" alt="" /></div><div><strong>Linux-Pilot</strong><span>性能观测平台</span></div></div>
      <div class="mode-switch" role="group" aria-label="部署模式">
        <button type="button" :class="{ active: deploymentMode === 'kubernetes' }" @click="selectDeploymentMode('kubernetes')">Kubernetes</button>
        <button type="button" :class="{ active: deploymentMode === 'standalone' }" @click="selectDeploymentMode('standalone')">普通主机</button>
      </div>
      <div class="nav-group-label">工作台</div>
      <nav class="main-nav" aria-label="主导航">
        <RouterLink v-for="item in visibleNavigation" :key="item.path" :to="item.path" :class="{ active: route.path === item.path }">
          <component :is="item.icon" :size="17" :stroke-width="1.9" /><span>{{ item.label }}</span>
        </RouterLink>
      </nav>
      <div class="sidebar-spacer"></div>
      <div class="sidebar-summary">
        <div class="summary-line"><span>在线节点</span><strong>{{ onlineCount }} / {{ visibleHosts.length }}</strong></div>
        <div class="summary-line"><span>数据通道</span><strong :class="connection === '实时连接' ? 'text-success' : 'text-warning'">{{ connection }}</strong></div>
      </div>
      <RouterLink class="sidebar-help" to="/metrics"><CircleHelp :size="16" /> 指标口径</RouterLink>
    </aside>

    <div class="workspace">
      <header class="topbar">
        <div class="page-heading"><div class="breadcrumb">Linux-Pilot / {{ pageTitle }}</div><h1>{{ pageTitle }}</h1></div>
        <div class="toolbar">
          <label v-if="showNodeSelector" class="toolbar-field"><Server :size="15" /><select :value="hostId" aria-label="选择节点" @change="platform.setHost(($event.target as HTMLSelectElement).value)">
            <option value="" disabled>选择节点</option><option v-for="host in visibleHosts" :key="host.id" :value="host.id">{{ host.hostname }}</option>
          </select></label>
          <label class="toolbar-field"><span class="toolbar-label">时间</span><select v-model="range" aria-label="时间范围"><option value="15m">最近 15 分钟</option><option value="1h">最近 1 小时</option><option value="6h">最近 6 小时</option><option value="24h">最近 24 小时</option></select></label>
          <button class="icon-button" title="刷新" aria-label="刷新" @click="refresh"><RefreshCw :size="17" /></button>
          <span class="role-chip" :title="account.email || undefined">{{ account.display_name || (role === 'admin' ? '管理员' : role === 'operator' ? '操作员' : '只读') }}</span>
          <button class="icon-button" title="退出登录" aria-label="退出登录" @click="signOut"><LogOut :size="17" /></button>
        </div>
      </header>
      <div v-if="error" class="notice error global-error"><span>{{ error }}</span><button @click="error = ''">关闭</button></div>
      <main class="content"><RouterView /></main>
      <footer class="footer"><span>Linux-Pilot</span><span><span class="live-dot" :class="{ off: connection !== '实时连接' }"></span> {{ connection }} · 指标时间以节点上报为准</span></footer>
    </div>
  </div>
</template>
