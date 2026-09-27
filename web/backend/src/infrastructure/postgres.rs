//! 数据持久化。所有外部值通过 SQL 参数传递，避免拼接用户输入。
//!
//! 这里实现应用层仓储端口，不把 SeaORM 的连接或查询结果泄漏给领域层。
//! 指标批次写入、去重键与 ACK 的事务顺序是最关键的可靠性边界。

use crate::application::ports::{
    AlertEvent, AlertRepository, AlertRule, HostView, MetricRange, MetricRepository,
    ProfileCompletion, ProfileJob, ProfileRepository,
};
use anyhow::{Context, ensure};
use async_trait::async_trait;
use linux_pilot_model::{Metric, Scenario, Score};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use serde_json::Value;
use std::collections::BTreeMap;

/// PostgreSQL 适配器；应用层仅持有各仓储端口的 trait object。
pub struct PostgresRepository {
    db: DatabaseConnection,
}

impl PostgresRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn stmt(sql: &str, values: impl IntoIterator<Item = sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// 在事务内按版本初始化 schema；多个后端副本同时启动时由 PostgreSQL 锁串行执行。
///
/// 版本 1 保存观测和任务数据，版本 2 增加账号认证。迁移记录与 DDL
/// 同事务提交，失败后不会留下“已升级但缺表”的中间状态。
pub async fn migrate(db: &DatabaseConnection) -> anyhow::Result<()> {
    let tx = db.begin().await?;
    tx.query_one(stmt(
        "SELECT pg_advisory_xact_lock(7452101)",
        std::iter::empty::<sea_orm::Value>(),
    ))
    .await?;
    tx.execute_unprepared("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_ms BIGINT NOT NULL)").await?;
    let version_one_exists = tx
        .query_one(stmt(
            "SELECT version FROM schema_migrations WHERE version=1",
            std::iter::empty::<sea_orm::Value>(),
        ))
        .await?
        .is_some();
    if !version_one_exists {
        for sql in [
            "CREATE TABLE IF NOT EXISTS hosts (id TEXT PRIMARY KEY, hostname TEXT NOT NULL, last_seen_ms BIGINT NOT NULL, capabilities JSONB NOT NULL DEFAULT '{}'::jsonb)",
            "CREATE TABLE IF NOT EXISTS agent_batches (host_id TEXT NOT NULL, boot_id TEXT NOT NULL, sequence BIGINT NOT NULL, received_ms BIGINT NOT NULL, PRIMARY KEY(host_id,boot_id,sequence))",
            "CREATE TABLE IF NOT EXISTS metrics (host_id TEXT NOT NULL, time_ms BIGINT NOT NULL, name TEXT NOT NULL, value DOUBLE PRECISION NOT NULL, labels JSONB NOT NULL, source TEXT NOT NULL)",
            "CREATE INDEX IF NOT EXISTS metrics_lookup ON metrics(host_id,name,time_ms DESC)",
            "CREATE INDEX IF NOT EXISTS metrics_host_time ON metrics(host_id,time_ms DESC)",
            "CREATE INDEX IF NOT EXISTS metrics_retention ON metrics(time_ms)",
            "CREATE INDEX IF NOT EXISTS batches_retention ON agent_batches(received_ms)",
            "CREATE TABLE IF NOT EXISTS scores (host_id TEXT NOT NULL, time_ms BIGINT NOT NULL, scenario TEXT NOT NULL, value DOUBLE PRECISION, coverage DOUBLE PRECISION NOT NULL, payload JSONB NOT NULL, PRIMARY KEY(host_id,time_ms,scenario))",
            "CREATE INDEX IF NOT EXISTS scores_lookup ON scores(host_id,scenario,time_ms DESC)",
            "CREATE TABLE IF NOT EXISTS profile_jobs (id TEXT PRIMARY KEY, host_id TEXT NOT NULL, pid INTEGER NOT NULL, duration_s INTEGER NOT NULL, frequency_hz INTEGER NOT NULL, status TEXT NOT NULL, error TEXT, folded TEXT, sample_count BIGINT, created_ms BIGINT NOT NULL, finished_ms BIGINT)",
            "CREATE TABLE IF NOT EXISTS alert_rules (id TEXT PRIMARY KEY, host_id TEXT NOT NULL, metric TEXT NOT NULL, comparison TEXT NOT NULL, threshold DOUBLE PRECISION NOT NULL, duration_s INTEGER NOT NULL DEFAULT 0, enabled BOOLEAN NOT NULL DEFAULT TRUE)",
            "CREATE TABLE IF NOT EXISTS alert_events (id TEXT PRIMARY KEY, rule_id TEXT NOT NULL, host_id TEXT NOT NULL, metric TEXT NOT NULL, value DOUBLE PRECISION NOT NULL, triggered_ms BIGINT NOT NULL, resolved_ms BIGINT)",
            "CREATE INDEX IF NOT EXISTS alerts_lookup ON alert_events(host_id,triggered_ms DESC)",
            "CREATE UNIQUE INDEX IF NOT EXISTS alerts_one_open_event ON alert_events(rule_id,host_id) WHERE resolved_ms IS NULL",
            "CREATE TABLE IF NOT EXISTS audit_events (id TEXT PRIMARY KEY, action TEXT NOT NULL, target TEXT NOT NULL, created_ms BIGINT NOT NULL)",
        ] {
            tx.execute_unprepared(sql).await?;
        }
        tx.execute(stmt(
            "INSERT INTO schema_migrations(version,applied_ms) VALUES(1,$1)",
            [chrono::Utc::now().timestamp_millis().into()],
        ))
        .await?;
    }
    let version_two_exists = tx
        .query_one(stmt(
            "SELECT version FROM schema_migrations WHERE version=2",
            std::iter::empty::<sea_orm::Value>(),
        ))
        .await?
        .is_some();
    if !version_two_exists {
        for sql in [
            "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, email TEXT NOT NULL UNIQUE, display_name TEXT NOT NULL, password_hash TEXT, role TEXT NOT NULL CHECK(role IN ('viewer','operator')), created_ms BIGINT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS auth_sessions (token_hash TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, created_ms BIGINT NOT NULL, expires_ms BIGINT NOT NULL)",
            "CREATE INDEX IF NOT EXISTS auth_sessions_expiry ON auth_sessions(expires_ms)",
            "CREATE TABLE IF NOT EXISTS email_codes (email TEXT PRIMARY KEY, code_hash TEXT NOT NULL, expires_ms BIGINT NOT NULL, sent_ms BIGINT NOT NULL, window_start_ms BIGINT NOT NULL, sent_count INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0)",
            "CREATE TABLE IF NOT EXISTS login_attempts (email TEXT PRIMARY KEY, window_start_ms BIGINT NOT NULL, attempts INTEGER NOT NULL)",
            "CREATE TABLE IF NOT EXISTS oauth_states (state_hash TEXT PRIMARY KEY, provider TEXT NOT NULL, verifier TEXT NOT NULL, expires_ms BIGINT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS oauth_identities (provider TEXT NOT NULL, provider_user_id TEXT NOT NULL, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, PRIMARY KEY(provider,provider_user_id))",
        ] {
            tx.execute_unprepared(sql).await?;
        }
        tx.execute(stmt(
            "INSERT INTO schema_migrations(version,applied_ms) VALUES(2,$1)",
            [chrono::Utc::now().timestamp_millis().into()],
        ))
        .await?;
    }
    // v3 引入真正的管理员角色和可归属到账号的审计事件。
    // 老版本的审计行保留，actor_user_id 为 NULL，页面将其显示为“系统/旧记录”。
    let version_three_exists = tx
        .query_one(stmt(
            "SELECT version FROM schema_migrations WHERE version=3",
            std::iter::empty::<sea_orm::Value>(),
        ))
        .await?
        .is_some();
    if !version_three_exists {
        for sql in [
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS username TEXT",
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS last_login_ms BIGINT",
            "CREATE UNIQUE INDEX IF NOT EXISTS users_username_unique ON users(username) WHERE username IS NOT NULL",
            "ALTER TABLE users DROP CONSTRAINT IF EXISTS users_role_check",
            "ALTER TABLE users ADD CONSTRAINT users_role_check CHECK(role IN ('viewer','operator','admin'))",
            "ALTER TABLE audit_events ADD COLUMN IF NOT EXISTS actor_user_id TEXT",
            "ALTER TABLE audit_events ADD COLUMN IF NOT EXISTS details JSONB NOT NULL DEFAULT '{}'::jsonb",
            "CREATE INDEX IF NOT EXISTS audit_actor_time ON audit_events(actor_user_id,created_ms DESC)",
        ] {
            tx.execute_unprepared(sql).await?;
        }
        tx.execute(stmt(
            "INSERT INTO schema_migrations(version,applied_ms) VALUES(3,$1)",
            [chrono::Utc::now().timestamp_millis().into()],
        ))
        .await?;
    }
    let version_four_exists = tx
        .query_one(stmt(
            "SELECT version FROM schema_migrations WHERE version=4",
            std::iter::empty::<sea_orm::Value>(),
        ))
        .await?
        .is_some();
    if !version_four_exists {
        // 用户活动查询还要找到“管理员给该用户分配权限”的记录，按 target
        // 建部分索引，避免审计表增长后在全表扫描上耗费时间。
        tx.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS audit_role_target_time ON audit_events(target,created_ms DESC) WHERE action='admin.role.update'",
        ).await?;
        tx.execute(stmt(
            "INSERT INTO schema_migrations(version,applied_ms) VALUES(4,$1)",
            [chrono::Utc::now().timestamp_millis().into()],
        ))
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 保存批次并用三元组去重。ACK 只在事务提交后发送，断线重传也不会重复写入。
pub async fn save_batch(
    db: &DatabaseConnection,
    host_id: &str,
    boot_id: &str,
    sequence: u64,
    metrics: &[Metric],
    now_ms: i64,
) -> anyhow::Result<bool> {
    let tx = db.begin().await?;
    let inserted = tx.execute(stmt(
        "INSERT INTO agent_batches(host_id,boot_id,sequence,received_ms) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING",
        [host_id.into(), boot_id.into(), (sequence as i64).into(), now_ms.into()],
    )).await?.rows_affected() > 0;
    if inserted && !metrics.is_empty() {
        let payload = serde_json::to_string(metrics)?;
        // jsonb_to_recordset 将整个批次一次性写入，避免每个指标产生一次数据库往返。
        tx.execute(stmt(
            "INSERT INTO metrics(host_id,time_ms,name,value,labels,source) SELECT $1,x.time_ms,x.name,x.value,x.labels,x.source FROM jsonb_to_recordset($2::jsonb) AS x(time_ms bigint,name text,value double precision,labels jsonb,source text)",
            [host_id.into(), payload.into()],
        )).await.context("批量写入指标失败")?;
    }
    tx.commit().await?;
    Ok(inserted)
}

/// 持久化规则版本及完整扣分证据。
pub async fn save_score(db: &DatabaseConnection, score: &Score) -> anyhow::Result<()> {
    let json = serde_json::to_string(score)?;
    db.execute(stmt(
        "INSERT INTO scores(host_id,time_ms,scenario,value,coverage,payload) VALUES($1,$2,$3,$4,$5,$6::jsonb) ON CONFLICT(host_id,time_ms,scenario) DO UPDATE SET value=EXCLUDED.value,coverage=EXCLUDED.coverage,payload=EXCLUDED.payload",
        [score.host_id.clone().into(), score.time_ms.into(), score.scenario.as_str().into(), score.value.into(), score.coverage.into(), json.into()],
    )).await?;
    Ok(())
}

/// 定期删除超出首版保留期的高频原始指标和批次标识。
pub async fn retain(db: &DatabaseConnection, cutoff_ms: i64) -> anyhow::Result<()> {
    db.execute(stmt(
        "DELETE FROM metrics WHERE time_ms < $1",
        [cutoff_ms.into()],
    ))
    .await?;
    db.execute(stmt(
        "DELETE FROM agent_batches WHERE received_ms < $1",
        [cutoff_ms.into()],
    ))
    .await?;
    Ok(())
}

#[async_trait]
impl MetricRepository for PostgresRepository {
    async fn ping(&self) -> anyhow::Result<()> {
        self.db.ping().await.map_err(Into::into)
    }

    async fn register_host(
        &self,
        id: &str,
        hostname: &str,
        capabilities: Value,
        now: i64,
    ) -> anyhow::Result<()> {
        self.db.execute(stmt(
            "INSERT INTO hosts(id,hostname,last_seen_ms,capabilities) VALUES($1,$2,$3,$4::jsonb) ON CONFLICT(id) DO UPDATE SET hostname=EXCLUDED.hostname,last_seen_ms=EXCLUDED.last_seen_ms,capabilities=EXCLUDED.capabilities",
            [id.into(), hostname.into(), now.into(), capabilities.to_string().into()],
        )).await?;
        Ok(())
    }

    async fn touch_host(&self, id: &str, now: i64) -> anyhow::Result<()> {
        self.db
            .execute(stmt(
                "UPDATE hosts SET last_seen_ms=$2 WHERE id=$1",
                [id.into(), now.into()],
            ))
            .await?;
        Ok(())
    }

    async fn store_batch(
        &self,
        host_id: &str,
        boot_id: &str,
        sequence: u64,
        metrics: &[Metric],
        now: i64,
    ) -> anyhow::Result<bool> {
        save_batch(&self.db, host_id, boot_id, sequence, metrics, now).await
    }

    async fn store_score(&self, score: &Score) -> anyhow::Result<()> {
        save_score(&self.db, score).await
    }

    async fn list_hosts(&self, now: i64) -> anyhow::Result<Vec<HostView>> {
        let rows = self.db.query_all(Statement::from_string(DatabaseBackend::Postgres,
            "SELECT h.id,h.hostname,h.last_seen_ms,h.capabilities::text AS capabilities, \
             (SELECT m.value FROM metrics m WHERE m.host_id=h.id AND m.name='cpu.busy_pct' AND m.labels='{}'::jsonb ORDER BY m.time_ms DESC LIMIT 1) AS cpu_pct, \
             (SELECT m.value FROM metrics m WHERE m.host_id=h.id AND m.name='mem.used_pct' AND m.labels='{}'::jsonb ORDER BY m.time_ms DESC LIMIT 1) AS mem_pct, \
             (SELECT s.value FROM scores s WHERE s.host_id=h.id AND s.scenario='general' ORDER BY s.time_ms DESC LIMIT 1) AS health_score \
             FROM hosts h ORDER BY h.hostname")).await?;
        rows.into_iter()
            .map(|row| {
                let last_seen_ms: i64 = row.try_get("", "last_seen_ms")?;
                let capabilities: String = row.try_get("", "capabilities")?;
                Ok(HostView {
                    id: row.try_get("", "id")?,
                    hostname: row.try_get("", "hostname")?,
                    last_seen_ms,
                    online: now - last_seen_ms < 15_000,
                    capabilities: serde_json::from_str(&capabilities)?,
                    cpu_pct: row.try_get("", "cpu_pct")?,
                    mem_pct: row.try_get("", "mem_pct")?,
                    health_score: row.try_get("", "health_score")?,
                })
            })
            .collect()
    }

    async fn overview(&self, host_id: &str, since: i64) -> anyhow::Result<BTreeMap<String, f64>> {
        let rows = self.db.query_all(stmt(
            "SELECT DISTINCT ON (name) name,value FROM metrics WHERE host_id=$1 AND time_ms>$2 AND labels='{}'::jsonb ORDER BY name,time_ms DESC LIMIT 1000",
            [host_id.into(), since.into()],
        )).await?;
        rows.into_iter()
            .map(|row| Ok((row.try_get("", "name")?, row.try_get("", "value")?)))
            .collect()
    }

    async fn metrics(&self, range: MetricRange<'_>) -> anyhow::Result<Vec<Metric>> {
        let prefix = range.category.map(|category| format!("{category}.%"));
        let rows = if let Some(step_ms) = range.step_ms {
            // 长窗口在数据库侧按时间桶聚合，传输量与图表点数均有界。
            self.db.query_all(stmt(
                "SELECT name,AVG(value) AS value,((time_ms / $6::bigint) * $6::bigint) AS time_ms,'{}'::text AS labels,'rollup'::text AS source FROM metrics WHERE host_id=$1 AND time_ms BETWEEN $2 AND $3 AND ($4::text IS NULL OR name LIKE $4) AND labels='{}'::jsonb GROUP BY name,(time_ms / $6::bigint) ORDER BY time_ms DESC LIMIT $5",
                [range.host_id.into(), range.from.into(), range.to.into(), prefix.into(), (range.limit as i64).into(), step_ms.into()],
            )).await?
        } else {
            self.db.query_all(stmt(
                "SELECT name,value,time_ms,labels::text AS labels,source FROM metrics WHERE host_id=$1 AND time_ms BETWEEN $2 AND $3 AND ($4::text IS NULL OR name LIKE $4) AND (NOT $6::bool OR labels='{}'::jsonb) ORDER BY time_ms DESC LIMIT $5",
                [range.host_id.into(), range.from.into(), range.to.into(), prefix.into(), (range.limit as i64).into(), range.aggregate_only.into()],
            )).await?
        };
        let mut values: Vec<Metric> = rows
            .into_iter()
            .map(|row| {
                let labels: String = row.try_get("", "labels")?;
                Ok(Metric {
                    name: row.try_get("", "name")?,
                    value: row.try_get("", "value")?,
                    time_ms: row.try_get("", "time_ms")?,
                    labels: serde_json::from_str(&labels)?,
                    source: row.try_get("", "source")?,
                })
            })
            .collect::<anyhow::Result<_>>()?;
        values.reverse();
        Ok(values)
    }

    async fn scores(
        &self,
        host_id: &str,
        scenario: Scenario,
        from: i64,
        to: i64,
    ) -> anyhow::Result<Vec<Score>> {
        let rows = self.db.query_all(stmt(
            "SELECT payload::text AS payload FROM scores WHERE host_id=$1 AND scenario=$2 AND time_ms BETWEEN $3 AND $4 ORDER BY time_ms DESC LIMIT 1000",
            [host_id.into(), scenario.as_str().into(), from.into(), to.into()],
        )).await?;
        let mut scores: Vec<Score> = rows
            .into_iter()
            .map(|row| {
                let payload: String = row.try_get("", "payload")?;
                serde_json::from_str(&payload).map_err(Into::into)
            })
            .collect::<anyhow::Result<_>>()?;
        scores.reverse();
        Ok(scores)
    }

    async fn retention(&self, cutoff_ms: i64) -> anyhow::Result<()> {
        retain(&self.db, cutoff_ms).await
    }
}

#[async_trait]
impl ProfileRepository for PostgresRepository {
    async fn start(&self, job: &ProfileJob) -> anyhow::Result<()> {
        let tx = self.db.begin().await?;
        // 同主机任务串行化：数据库锁覆盖不同后端副本，避免并发请求同时启动 perf。
        tx.query_one(stmt(
            "SELECT pg_advisory_xact_lock(hashtext($1))",
            [job.host_id.clone().into()],
        ))
        .await?;
        let running = tx
            .query_one(stmt(
                "SELECT COUNT(*) AS count FROM profile_jobs WHERE host_id=$1 AND status='running'",
                [job.host_id.clone().into()],
            ))
            .await?
            .context("查询运行中的采样任务失败")?
            .try_get::<i64>("", "count")?;
        ensure!(running == 0, "该主机已有运行中的采样任务");
        tx.execute(stmt(
            "INSERT INTO profile_jobs(id,host_id,pid,duration_s,frequency_hz,status,created_ms) VALUES($1,$2,$3,$4,$5,'running',$6)",
            [job.id.clone().into(), job.host_id.clone().into(), job.pid.into(), job.duration_s.into(), job.frequency_hz.into(), job.created_ms.into()],
        )).await?;
        tx.execute(stmt(
            "INSERT INTO audit_events(id,action,target,created_ms) VALUES($1,'profile.start',$2,$3)",
            [uuid::Uuid::new_v4().to_string().into(), job.id.clone().into(), job.created_ms.into()],
        )).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<ProfileJob>> {
        let Some(row) = self.db.query_one(stmt(
            "SELECT id,host_id,pid,duration_s,frequency_hz,status,error,folded,sample_count,created_ms,finished_ms FROM profile_jobs WHERE id=$1",
            [id.into()],
        )).await? else { return Ok(None) };
        Ok(Some(ProfileJob {
            id: row.try_get("", "id")?,
            host_id: row.try_get("", "host_id")?,
            pid: row.try_get("", "pid")?,
            duration_s: row.try_get("", "duration_s")?,
            frequency_hz: row.try_get("", "frequency_hz")?,
            status: row.try_get("", "status")?,
            error: row.try_get("", "error")?,
            folded: row.try_get("", "folded")?,
            sample_count: row.try_get("", "sample_count")?,
            created_ms: row.try_get("", "created_ms")?,
            finished_ms: row.try_get("", "finished_ms")?,
        }))
    }

    async fn list_recent(&self, host_id: &str) -> anyhow::Result<Vec<ProfileJob>> {
        let rows = self.db.query_all(stmt(
            "SELECT id,host_id,pid,duration_s,frequency_hz,status,error,NULL::text AS folded,sample_count,created_ms,finished_ms FROM profile_jobs WHERE host_id=$1 ORDER BY created_ms DESC LIMIT 100",
            [host_id.into()],
        )).await?;
        rows.into_iter()
            .map(|row| {
                Ok(ProfileJob {
                    id: row.try_get("", "id")?,
                    host_id: row.try_get("", "host_id")?,
                    pid: row.try_get("", "pid")?,
                    duration_s: row.try_get("", "duration_s")?,
                    frequency_hz: row.try_get("", "frequency_hz")?,
                    status: row.try_get("", "status")?,
                    error: row.try_get("", "error")?,
                    folded: None,
                    sample_count: row.try_get("", "sample_count")?,
                    created_ms: row.try_get("", "created_ms")?,
                    finished_ms: row.try_get("", "finished_ms")?,
                })
            })
            .collect()
    }

    async fn complete(&self, result: ProfileCompletion<'_>) -> anyhow::Result<()> {
        let status = if result.success {
            "completed"
        } else {
            "failed"
        };
        self.db.execute(stmt(
            "UPDATE profile_jobs SET status=$2,error=$3,folded=$4,sample_count=$5,finished_ms=$6 WHERE id=$1 AND host_id=$7 AND status='running'",
            [result.id.into(), status.into(), result.error.into(), result.folded.into(), (result.sample_count as i64).into(), result.finished_ms.into(), result.host_id.into()],
        )).await?;
        Ok(())
    }

    async fn expire_stale(&self, cutoff_ms: i64, now_ms: i64) -> anyhow::Result<u64> {
        let result = self.db.execute(stmt(
            "UPDATE profile_jobs SET status='failed',error='Worker 断线或任务超时',finished_ms=$2 WHERE status='running' AND created_ms<$1",
            [cutoff_ms.into(), now_ms.into()],
        )).await?;
        Ok(result.rows_affected())
    }
}

#[async_trait]
impl AlertRepository for PostgresRepository {
    async fn create_rule(&self, rule: &AlertRule) -> anyhow::Result<()> {
        let tx = self.db.begin().await?;
        tx.execute(stmt(
            "INSERT INTO alert_rules(id,host_id,metric,comparison,threshold,duration_s) VALUES($1,$2,$3,$4,$5,$6)",
            [rule.id.clone().into(), rule.host_id.clone().into(), rule.metric.clone().into(), rule.comparison.clone().into(), rule.threshold.into(), rule.duration_s.into()],
        )).await?;
        tx.execute(stmt(
            "INSERT INTO audit_events(id,action,target,created_ms) VALUES($1,'alert.create',$2,$3)",
            [
                uuid::Uuid::new_v4().to_string().into(),
                rule.id.clone().into(),
                chrono::Utc::now().timestamp_millis().into(),
            ],
        ))
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn set_enabled(&self, id: &str, enabled: bool, now: i64) -> anyhow::Result<bool> {
        let tx = self.db.begin().await?;
        let updated = tx
            .execute(stmt(
                "UPDATE alert_rules SET enabled=$2 WHERE id=$1",
                [id.into(), enabled.into()],
            ))
            .await?
            .rows_affected()
            > 0;
        if updated {
            if !enabled {
                // 停用规则后立即关闭它的所有进行中事件，避免长期挂起。
                tx.execute(stmt("UPDATE alert_events SET resolved_ms=$2 WHERE rule_id=$1 AND resolved_ms IS NULL", [id.into(), now.into()])).await?;
            }
            tx.execute(stmt(
                "INSERT INTO audit_events(id,action,target,created_ms) VALUES($1,$2,$3,$4)",
                [
                    uuid::Uuid::new_v4().to_string().into(),
                    (if enabled {
                        "alert.enable"
                    } else {
                        "alert.disable"
                    })
                    .into(),
                    id.into(),
                    now.into(),
                ],
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(updated)
    }

    async fn list_rules(&self) -> anyhow::Result<Vec<AlertRule>> {
        let rows = self.db.query_all(Statement::from_string(DatabaseBackend::Postgres,
            "SELECT id,host_id,metric,comparison,threshold,duration_s,enabled FROM alert_rules ORDER BY metric")).await?;
        rows.into_iter().map(alert_rule_from_row).collect()
    }

    async fn active_rules(&self, host_id: &str) -> anyhow::Result<Vec<AlertRule>> {
        let rows = self.db.query_all(stmt(
            "SELECT id,host_id,metric,comparison,threshold,duration_s,enabled FROM alert_rules WHERE enabled=true AND (host_id=$1 OR host_id='*')",
            [host_id.into()],
        )).await?;
        rows.into_iter().map(alert_rule_from_row).collect()
    }

    async fn list_events(&self) -> anyhow::Result<Vec<AlertEvent>> {
        let rows = self.db.query_all(Statement::from_string(DatabaseBackend::Postgres,
            "SELECT id,rule_id,host_id,metric,value,triggered_ms,resolved_ms FROM alert_events ORDER BY triggered_ms DESC LIMIT 200")).await?;
        rows.into_iter()
            .map(|row| {
                Ok(AlertEvent {
                    id: row.try_get("", "id")?,
                    rule_id: row.try_get("", "rule_id")?,
                    host_id: row.try_get("", "host_id")?,
                    metric: row.try_get("", "metric")?,
                    value: row.try_get("", "value")?,
                    triggered_ms: row.try_get("", "triggered_ms")?,
                    resolved_ms: row.try_get("", "resolved_ms")?,
                })
            })
            .collect()
    }

    async fn trigger(
        &self,
        rule: &AlertRule,
        host_id: &str,
        value: f64,
        now: i64,
    ) -> anyhow::Result<()> {
        self.db.execute(stmt(
            "INSERT INTO alert_events(id,rule_id,host_id,metric,value,triggered_ms) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING",
            [uuid::Uuid::new_v4().to_string().into(), rule.id.clone().into(), host_id.into(), rule.metric.clone().into(), value.into(), now.into()],
        )).await?;
        Ok(())
    }

    async fn resolve(&self, rule_id: &str, host_id: &str, now: i64) -> anyhow::Result<()> {
        self.db.execute(stmt(
            "UPDATE alert_events SET resolved_ms=$3 WHERE rule_id=$1 AND host_id=$2 AND resolved_ms IS NULL",
            [rule_id.into(), host_id.into(), now.into()],
        )).await?;
        Ok(())
    }
}

fn alert_rule_from_row(row: sea_orm::QueryResult) -> anyhow::Result<AlertRule> {
    Ok(AlertRule {
        id: row.try_get("", "id")?,
        host_id: row.try_get("", "host_id")?,
        metric: row.try_get("", "metric")?,
        comparison: row.try_get("", "comparison")?,
        threshold: row.try_get("", "threshold")?,
        duration_s: row.try_get("", "duration_s")?,
        enabled: row.try_get("", "enabled")?,
    })
}
