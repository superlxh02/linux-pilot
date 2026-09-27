/**
 * 控制台会话与实时数据状态。
 * 历史查询以 HTTP/数据库为事实来源；WebSocket 只负责在当前页面增量更新。
 * 浏览器不读取 HttpOnly 会话 Cookie，所有认证请求由同源 fetch 自动携带。
 */
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { request, setToken, token, type Host, type Metric, type Score } from './api'

const rangeLength: Record<string, number> = { '15m': 900_000, '1h': 3_600_000, '6h': 21_600_000, '24h': 86_400_000 }

export const usePlatform = defineStore('platform', () => {
  const hosts = ref<Host[]>([])
  const hostId = ref(localStorage.getItem('po-host-id') || '')
  const latest = ref<Record<string, number>>({})
  const scores = ref<Record<string, Score>>({})
  const role = ref<'viewer' | 'operator' | 'admin' | ''>('')
  const account = ref<{ email: string | null, display_name: string | null }>({ email: null, display_name: null })
  const connection = ref('未连接')
  const range = ref('1h')
  const error = ref('')
  const authenticated = computed(() => role.value !== '')
  const selectedHost = computed(() => hosts.value.find((item) => item.id === hostId.value))
  let socket: WebSocket | null = null
  let retryTimer: number | undefined

  function timeWindow() {
    const to = Date.now()
    const duration = rangeLength[range.value] || rangeLength['1h']
    return { from: to - duration, to, step_ms: Math.max(1000, Math.ceil(duration / 180_000) * 1000) }
  }

  async function initializeSession(result: { role: 'viewer' | 'operator' | 'admin', email?: string | null, display_name?: string | null }) {
    // 先建立角色，再加载节点与连接实时流。若节点查询失败，调用方会
    // 呈现错误，后端会话仍由 Cookie 独立管理。
    role.value = result.role
    account.value = { email: result.email || null, display_name: result.display_name || null }
    error.value = ''
    await loadHosts()
    connect()
  }

  async function login(email: string, password: string) {
    try {
      const result = await request<{ role: 'viewer' | 'operator' | 'admin', email: string, display_name: string }>('/api/v1/auth/login',
        { method: 'POST', body: JSON.stringify({ email, password }) })
      await initializeSession(result)
    } catch (cause) {
      role.value = ''
      throw cause
    }
  }

  async function register(email: string, display_name: string, password: string, code: string) {
    const result = await request<{ role: 'viewer' | 'operator' | 'admin', email: string, display_name: string }>('/api/v1/auth/register',
      { method: 'POST', body: JSON.stringify({ email, display_name, password, code }) })
    await initializeSession(result)
  }

  async function sendCode(email: string) {
    return request<{ message: string }>('/api/v1/auth/email-code', { method: 'POST', body: JSON.stringify({ email }) })
  }

  async function restore() {
    // 旧版控制台曾在 sessionStorage 保存静态访问令牌；升级后清除，浏览器只使用 HttpOnly 会话。
    setToken('')
    try {
      const result = await request<{ role: 'viewer' | 'operator' | 'admin', email: string | null, display_name: string | null }>('/api/v1/session')
      await initializeSession(result)
    } catch { role.value = '' }
  }

  async function logout() {
    // 等服务端撤销会话并清除 Cookie 后再允许下一次登录，避免旧 logout
    // 响应晚于新 login 响应到达，误删新账号的会话 Cookie。
    try { await request<void>('/api/v1/auth/logout', { method: 'POST' }) }
    catch (cause) { error.value = cause instanceof Error ? cause.message : String(cause) }
    setToken('')
    role.value = ''
    account.value = { email: null, display_name: null }
    hosts.value = []
    latest.value = {}
    scores.value = {}
    if (retryTimer) window.clearTimeout(retryTimer)
    socket?.close()
    socket = null
    connection.value = '未连接'
  }

  async function loadHosts() {
    hosts.value = await request<Host[]>('/api/v1/hosts')
    // 浏览器可能记住已经离线的节点。首次打开优先展示在线数据，用户手动切换后仍可查看离线历史。
    const saved = hosts.value.find((host) => host.id === hostId.value)
    if (!saved || !saved.online) setHost(hosts.value.find((host) => host.online)?.id || hosts.value[0]?.id || '')
  }

  function setHost(id: string) {
    hostId.value = id
    localStorage.setItem('po-host-id', id)
    latest.value = {}
    scores.value = {}
    if (id) void refreshOverview().catch((cause) => {
      error.value = cause instanceof Error ? cause.message : String(cause)
    })
  }

  async function refreshOverview() {
    if (!hostId.value) return
    const id = hostId.value
    const result = await request<{ latest: Record<string, number> }>(`/api/v1/hosts/${encodeURIComponent(id)}/overview`)
    if (id === hostId.value) latest.value = result.latest
  }

  async function queryMetrics(category: string, aggregateOnly = true): Promise<Metric[]> {
    if (!hostId.value) return []
    const window = timeWindow()
    const params = new URLSearchParams({ host_id: hostId.value, category, from: String(window.from),
      to: String(window.to), limit: '10000' })
    if (aggregateOnly) { params.set('aggregate_only', 'true'); params.set('step_ms', String(window.step_ms)) }
    return request<Metric[]>(`/api/v1/metrics?${params}`)
  }

  async function queryScores(scenario: string): Promise<Score[]> {
    if (!hostId.value) return []
    const { from, to } = timeWindow()
    const params = new URLSearchParams({ host_id: hostId.value, scenario, from: String(from), to: String(to) })
    return request<Score[]>(`/api/v1/scores?${params}`)
  }

  function connect() {
    // 账号登录依赖自动附带的同源 Cookie。旧版 API Token 仍可通过
    // WebSocket 子协议参与脚本兼容，但登录 UI 不保存静态令牌。
    if (!authenticated.value) return
    socket?.close()
    const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:'
    // 浏览器 WebSocket 不支持自定义 Authorization 头；将令牌编码到握手子协议头，避免进入 URL/访问日志。
    const encoded = [...new TextEncoder().encode(token())].map((byte) => byte.toString(16).padStart(2, '0')).join('')
    const protocols = token() ? ['po-v1', `auth.${encoded}`] : ['po-v1']
    const current = new WebSocket(`${scheme}//${location.host}/api/v1/stream`, protocols)
    socket = current
    current.onopen = () => { connection.value = '实时连接' }
    current.onerror = () => { connection.value = '连接异常' }
    current.onclose = () => {
      if (socket !== current || !authenticated.value) return
      connection.value = '重连中'
      retryTimer = window.setTimeout(connect, 3000)
    }
    current.onmessage = (event) => {
      try {
        const message = JSON.parse(event.data)
        if (message.type === 'metrics' && message.host_id === hostId.value) {
          for (const metric of message.metrics as Metric[]) {
            if (Object.keys(metric.labels).length === 0) latest.value[metric.name] = metric.value
          }
        }
        if (message.type === 'score' && message.score.host_id === hostId.value) {
          const score = message.score as Score
          scores.value[score.scenario] = score
        }
      } catch { /* 丢弃无效帧；HTTP 历史查询仍是数据来源。 */ }
    }
  }

  return { hosts, hostId, latest, scores, role, account, connection, range, error, authenticated,
    selectedHost, timeWindow, login, register, sendCode, restore, logout, loadHosts, setHost, refreshOverview,
    queryMetrics, queryScores, connect }
})
