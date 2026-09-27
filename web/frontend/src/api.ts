/**
 * 与服务端一致的最小数据契约。
 * 主机汇总与带标签指标分开查询，避免图表把设备明细误加到整机汇总上。
 * 额外字段由详情页按需读取，不在所有页面长期缓存。
 */
export interface Host {
  id: string
  hostname: string
  online: boolean
  last_seen_ms: number
  capabilities: { ebpf?: boolean; perf?: boolean; cgroup_v2?: boolean }
  cpu_pct: number | null
  mem_pct: number | null
  health_score: number | null
}
export interface Metric {
  name: string
  value: number
  time_ms: number
  labels: Record<string, string>
  source: string
}
export interface ScoreFactor {
  metric: string
  dimension: string
  value: number
  penalty: number
  weight: number
  good: number
  bad: number
}
export interface Score {
  host_id: string
  time_ms: number
  scenario: string
  value: number | null
  coverage: number
  profile_version: string
  dimension_scores: Record<string, number | null>
  factors: ScoreFactor[]
  missing: string[]
}

export interface ProfileJob {
  id: string; host_id: string; pid: number; duration_s: number; frequency_hz: number
  status: string; error: string | null; folded: string | null; sample_count: number | null
  created_ms: number; finished_ms: number | null
}
export interface AlertRule {
  id: string; host_id: string; metric: string; comparison: string
  threshold: number; duration_s: number; enabled: boolean
}
export interface AlertEvent {
  id: string; rule_id: string; host_id: string; metric: string
  value: number; triggered_ms: number; resolved_ms: number | null
}

export function token(): string {
  return sessionStorage.getItem('po-ui-token') || ''
}

export function setToken(value: string) {
  if (value) sessionStorage.setItem('po-ui-token', value)
  else sessionStorage.removeItem('po-ui-token')
}

export async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  // 同源 Cookie 由浏览器携带，前端代码无法读取其值；Authorization 仅保留
  // 给尚未迁移的脚本客户端，正常账号流程中的 token() 为空。
  const response = await fetch(path, {
    ...options,
    credentials: 'same-origin',
    headers: {
      'Content-Type': 'application/json',
      ...(token() ? { Authorization: `Bearer ${token()}` } : {}),
      ...(options.headers || {})
    }
  })
  if (!response.ok) {
    if (response.status === 401) throw new Error('用户名、邮箱或密码错误，或登录会话已过期')
    if (response.status === 403) throw new Error('当前账号没有执行此操作的权限')
    throw new Error(await response.text() || `HTTP ${response.status}`)
  }
  if (response.status === 204) return undefined as T
  return response.json() as Promise<T>
}
