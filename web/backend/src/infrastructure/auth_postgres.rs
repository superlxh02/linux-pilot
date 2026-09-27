//! 账号仓储：验证码、会话和 OAuth 状态均持久化，多个后端副本可共享认证状态。
//!
//! 频率限制与一次性状态依赖 PostgreSQL 的唯一键和原子 UPSERT/DELETE，
//! 不能改为进程内 HashMap；否则多个后端副本会各自放行，重启也会重置限制。
//! 查询使用参数绑定，不把密码哈希带入普通会话请求。

use crate::application::auth::{AuthStore, AuthUser, ManagedUser, OAuthProfile, UserActivity};
use anyhow::Context;
use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement, TransactionTrait,
};

/// 将认证应用端口映射到 PostgreSQL；可共享已有 Web 连接池。
pub struct PostgresAuthStore {
    db: DatabaseConnection,
}

impl PostgresAuthStore {
    /// 克隆 SeaORM 连接句柄，不新建第二个数据库连接池。
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn stmt(sql: &str, values: impl IntoIterator<Item = sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn user(row: QueryResult) -> anyhow::Result<AuthUser> {
    Ok(AuthUser {
        id: row.try_get("", "id")?,
        email: row.try_get("", "email")?,
        display_name: row.try_get("", "display_name")?,
        role: row.try_get("", "role")?,
        password_hash: row.try_get("", "password_hash")?,
    })
}

#[async_trait]
impl AuthStore for PostgresAuthStore {
    async fn find_username(&self, username: &str) -> anyhow::Result<Option<AuthUser>> {
        self.db
            .query_one(stmt(
                "SELECT id,email,display_name,role,password_hash FROM users WHERE username=$1",
                [username.into()],
            ))
            .await?
            .map(user)
            .transpose()
    }

    async fn bootstrap_admin(&self, password_hash: &str, now: i64) -> anyhow::Result<()> {
        // 配置更换时更新既有 admin 哈希，并撤销该账号旧会话。事务提交前
        // 不会出现“新密码已经生效、旧会话还保留”的中间状态。
        let tx = self.db.begin().await?;
        tx.execute(stmt(
            "INSERT INTO users(id,username,email,display_name,password_hash,role,created_ms) VALUES($1,'admin','admin@linux-pilot.invalid','系统管理员',$2,'admin',$3) ON CONFLICT(username) WHERE username IS NOT NULL DO UPDATE SET password_hash=EXCLUDED.password_hash,role='admin'",
            [uuid::Uuid::new_v4().to_string().into(), password_hash.into(), now.into()],
        )).await?;
        tx.execute(stmt(
            "DELETE FROM auth_sessions WHERE user_id=(SELECT id FROM users WHERE username='admin')",
            std::iter::empty::<sea_orm::Value>(),
        ))
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn list_users(&self, limit: i64, offset: i64) -> anyhow::Result<Vec<ManagedUser>> {
        let rows = self.db.query_all(stmt(
            "SELECT id,username,email,display_name,role,created_ms,last_login_ms FROM users ORDER BY created_ms DESC,id LIMIT $1 OFFSET $2",
            [limit.into(), offset.into()],
        )).await?;
        rows.into_iter()
            .map(|row| {
                Ok(ManagedUser {
                    id: row.try_get("", "id")?,
                    username: row.try_get("", "username")?,
                    email: row.try_get("", "email")?,
                    display_name: row.try_get("", "display_name")?,
                    role: row.try_get("", "role")?,
                    created_ms: row.try_get("", "created_ms")?,
                    last_login_ms: row.try_get("", "last_login_ms")?,
                })
            })
            .collect()
    }

    async fn user_activity(&self, user_id: &str, limit: i64) -> anyhow::Result<Vec<UserActivity>> {
        let rows = self.db.query_all(stmt(
            "SELECT e.id,e.action,e.target,e.details::text AS details,e.created_ms,u.display_name AS actor_name FROM audit_events e LEFT JOIN users u ON u.id=e.actor_user_id WHERE e.actor_user_id=$1 OR (e.target=$1 AND e.action='admin.role.update') ORDER BY e.created_ms DESC,e.id DESC LIMIT $2",
            [user_id.into(), limit.into()],
        )).await?;
        rows.into_iter()
            .map(|row| {
                let details: String = row.try_get("", "details")?;
                Ok(UserActivity {
                    id: row.try_get("", "id")?,
                    action: row.try_get("", "action")?,
                    target: row.try_get("", "target")?,
                    actor_name: row.try_get("", "actor_name")?,
                    details: serde_json::from_str(&details)?,
                    created_ms: row.try_get("", "created_ms")?,
                })
            })
            .collect()
    }

    async fn set_role(
        &self,
        actor_id: &str,
        user_id: &str,
        role: &str,
        now: i64,
    ) -> anyhow::Result<bool> {
        // 后端再次核对管理员身份；即使接口层未来改路由，也不能仅靠
        // 浏览器是否显示菜单来授权。内置 admin 不允许被改角色。
        let tx = self.db.begin().await?;
        let actor = tx
            .query_one(stmt(
                "SELECT id FROM users WHERE id=$1 AND role='admin'",
                [actor_id.into()],
            ))
            .await?;
        anyhow::ensure!(actor.is_some(), "只有管理员可修改角色");
        let previous = tx
            .query_one(stmt(
                "SELECT role FROM users WHERE id=$1 AND username IS NULL FOR UPDATE",
                [user_id.into()],
            ))
            .await?;
        let Some(previous) = previous else {
            tx.rollback().await?;
            return Ok(false);
        };
        let old_role: String = previous.try_get("", "role")?;
        tx.execute(stmt(
            "UPDATE users SET role=$2 WHERE id=$1",
            [user_id.into(), role.into()],
        ))
        .await?;
        let details = serde_json::json!({"from":old_role,"to":role}).to_string();
        tx.execute(stmt(
            "INSERT INTO audit_events(id,action,target,actor_user_id,details,created_ms) VALUES($1,'admin.role.update',$2,$3,$4::jsonb,$5)",
            [uuid::Uuid::new_v4().to_string().into(), user_id.into(), actor_id.into(), details.into(), now.into()],
        )).await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn record_action(
        &self,
        actor_id: &str,
        action: &str,
        target: &str,
        details: serde_json::Value,
        now: i64,
    ) -> anyhow::Result<()> {
        // 审计写入失败时调用方会收到错误，不会显示“操作成功但没有记录”。
        let tx = self.db.begin().await?;
        tx.execute(stmt(
            "INSERT INTO audit_events(id,action,target,actor_user_id,details,created_ms) VALUES($1,$2,$3,$4,$5::jsonb,$6)",
            [uuid::Uuid::new_v4().to_string().into(), action.into(), target.into(), actor_id.into(), details.to_string().into(), now.into()],
        )).await?;
        if action.starts_with("auth.login") {
            tx.execute(stmt(
                "UPDATE users SET last_login_ms=$2 WHERE id=$1",
                [actor_id.into(), now.into()],
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn find_user(&self, email: &str) -> anyhow::Result<Option<AuthUser>> {
        self.db
            .query_one(stmt(
                "SELECT id,email,display_name,role,password_hash FROM users WHERE email=$1",
                [email.into()],
            ))
            .await?
            .map(user)
            .transpose()
    }

    async fn reserve_code(&self, email: &str, hash: &str, now: i64) -> anyhow::Result<bool> {
        // 单条 UPSERT 同时处理 60 秒冷却和每小时最多 5 次；邮箱唯一键
        // 使并发请求在数据库中串行化。返回 false 时不发送邮件。
        let inserted = self.db.query_one(stmt(
            "INSERT INTO email_codes(email,code_hash,expires_ms,sent_ms,window_start_ms,sent_count,attempts) \
             VALUES($1,$2,$3,$4,$4,1,0) ON CONFLICT(email) DO UPDATE SET \
             code_hash=EXCLUDED.code_hash,expires_ms=EXCLUDED.expires_ms,sent_ms=EXCLUDED.sent_ms, \
             window_start_ms=CASE WHEN email_codes.window_start_ms <= $4-3600000 THEN $4 ELSE email_codes.window_start_ms END, \
             sent_count=CASE WHEN email_codes.window_start_ms <= $4-3600000 THEN 1 ELSE email_codes.sent_count+1 END,attempts=0 \
             WHERE email_codes.sent_ms <= $4-60000 AND (email_codes.window_start_ms <= $4-3600000 OR email_codes.sent_count<5) RETURNING email",
            [email.into(), hash.into(), (now+600_000).into(), now.into()],
        )).await?.is_some();
        Ok(inserted)
    }

    async fn remove_code(&self, email: &str, hash: &str) -> anyhow::Result<()> {
        self.db
            .execute(stmt(
                "DELETE FROM email_codes WHERE email=$1 AND code_hash=$2",
                [email.into(), hash.into()],
            ))
            .await?;
        Ok(())
    }

    async fn attempt_code(&self, email: &str, now: i64) -> anyhow::Result<Option<String>> {
        // 在验证前先原子增加尝试次数。即使用户提交错误码，尝试也会消耗，
        // 避免并发猜测反复读取同一个六位验证码。
        self.db.query_one(stmt(
            "UPDATE email_codes SET attempts=attempts+1 WHERE email=$1 AND expires_ms>$2 AND attempts<5 RETURNING code_hash",
            [email.into(), now.into()],
        )).await?.map(|row| row.try_get("", "code_hash").map_err(Into::into)).transpose()
    }

    async fn register(
        &self,
        account: &AuthUser,
        code_hash: &str,
        now: i64,
    ) -> anyhow::Result<bool> {
        // 锁住验证码行，然后在同一事务里插入账号并删除验证码。
        // 两个并发注册请求只能有一个成功；中途失败会完整回滚。
        let tx = self.db.begin().await?;
        let valid = tx.query_one(stmt(
            "SELECT email FROM email_codes WHERE email=$1 AND code_hash=$2 AND expires_ms>$3 AND attempts<=5 FOR UPDATE",
            [account.email.clone().into(), code_hash.into(), now.into()],
        )).await?.is_some();
        if !valid {
            tx.rollback().await?;
            return Ok(false);
        }
        let inserted = tx.query_one(stmt(
            "INSERT INTO users(id,email,display_name,password_hash,role,created_ms,last_login_ms) VALUES($1,$2,$3,$4,$5,$6,$6) ON CONFLICT(email) DO NOTHING RETURNING id",
            [account.id.clone().into(), account.email.clone().into(), account.display_name.clone().into(), account.password_hash.clone().into(), account.role.clone().into(), now.into()],
        )).await?.is_some();
        if inserted {
            tx.execute(stmt(
                "DELETE FROM email_codes WHERE email=$1",
                [account.email.clone().into()],
            ))
            .await?;
            tx.execute(stmt("INSERT INTO audit_events(id,action,target,actor_user_id,created_ms) VALUES($1,'auth.register',$2,$2,$3)",
                [uuid::Uuid::new_v4().to_string().into(), account.id.clone().into(), now.into()])).await?;
        }
        tx.commit().await?;
        Ok(inserted)
    }

    async fn login_allowed(&self, email: &str, now: i64) -> anyhow::Result<bool> {
        // 尝试次数先以原子 UPSERT 预留，避免并发猜测同时绕过上限；成功后清零。
        let row = self
            .db
            .query_one(stmt(
                "INSERT INTO login_attempts(email,window_start_ms,attempts) VALUES($1,$2,1) ON CONFLICT(email) DO UPDATE SET \
                 window_start_ms=CASE WHEN login_attempts.window_start_ms <= $2-900000 THEN $2 ELSE login_attempts.window_start_ms END, \
                 attempts=CASE WHEN login_attempts.window_start_ms <= $2-900000 THEN 1 ELSE login_attempts.attempts+1 END RETURNING attempts",
                [email.into(), now.into()],
            ))
            .await?
            .context("登录限流记录缺失")?;
        Ok(row.try_get::<i32>("", "attempts")? <= 10)
    }

    async fn clear_login_failures(&self, email: &str) -> anyhow::Result<()> {
        self.db
            .execute(stmt(
                "DELETE FROM login_attempts WHERE email=$1",
                [email.into()],
            ))
            .await?;
        Ok(())
    }

    async fn create_session(
        &self,
        hash: &str,
        user_id: &str,
        now: i64,
        expires: i64,
    ) -> anyhow::Result<()> {
        self.db.execute(stmt(
            "INSERT INTO auth_sessions(token_hash,user_id,created_ms,expires_ms) VALUES($1,$2,$3,$4)",
            [hash.into(), user_id.into(), now.into(), expires.into()],
        )).await?;
        Ok(())
    }

    async fn session_user(&self, hash: &str, now: i64) -> anyhow::Result<Option<AuthUser>> {
        // 会话热路径只需账号 ID、邮箱、昵称和角色。密码哈希用 NULL 占位，
        // 不从数据库读取到普通 API 请求内存，更不会意外进入响应序列化。
        self.db.query_one(stmt(
            "SELECT u.id,u.email,u.display_name,u.role,NULL::text AS password_hash FROM auth_sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=$1 AND s.expires_ms>$2",
            [hash.into(), now.into()],
        )).await?.map(user).transpose()
    }

    async fn revoke_session(&self, hash: &str) -> anyhow::Result<()> {
        self.db
            .execute(stmt(
                "DELETE FROM auth_sessions WHERE token_hash=$1",
                [hash.into()],
            ))
            .await?;
        Ok(())
    }

    async fn save_oauth_state(
        &self,
        hash: &str,
        provider: &str,
        verifier: &str,
        expires: i64,
    ) -> anyhow::Result<()> {
        self.db.execute(stmt(
            "INSERT INTO oauth_states(state_hash,provider,verifier,expires_ms) VALUES($1,$2,$3,$4)",
            [hash.into(), provider.into(), verifier.into(), expires.into()],
        )).await?;
        Ok(())
    }

    async fn consume_oauth_state(
        &self,
        hash: &str,
        provider: &str,
        now: i64,
    ) -> anyhow::Result<Option<String>> {
        // DELETE ... RETURNING 在数据库层原子消费 state，回调重放无效。
        self.db.query_one(stmt(
            "DELETE FROM oauth_states WHERE state_hash=$1 AND provider=$2 AND expires_ms>$3 RETURNING verifier",
            [hash.into(), provider.into(), now.into()],
        )).await?.map(|row| row.try_get("", "verifier").map_err(Into::into)).transpose()
    }

    async fn oauth_user(
        &self,
        provider: &str,
        profile: &OAuthProfile,
        role: &str,
        now: i64,
    ) -> anyhow::Result<Option<AuthUser>> {
        // OAuth 身份使用 (provider, provider_user_id) 稳定主键，而非邮箱。
        // 邮箱可能在提供方侧改变，不能据此接管本地密码账号。
        let tx = self.db.begin().await?;
        if let Some(row) = tx.query_one(stmt(
            "SELECT u.id,u.email,u.display_name,u.role,u.password_hash FROM oauth_identities i JOIN users u ON u.id=i.user_id WHERE i.provider=$1 AND i.provider_user_id=$2",
            [provider.into(), profile.provider_user_id.clone().into()],
        )).await? {
            tx.commit().await?;
            return Ok(Some(user(row)?));
        }
        // 不根据邮箱自动绑定已有账号，防止身份提供方邮箱变更导致接管。
        if tx
            .query_one(stmt(
                "SELECT id FROM users WHERE email=$1",
                [profile.email.clone().into()],
            ))
            .await?
            .is_some()
        {
            tx.rollback().await?;
            return Ok(None);
        }
        let account = AuthUser {
            id: uuid::Uuid::new_v4().to_string(),
            email: profile.email.clone(),
            display_name: profile.display_name.clone(),
            role: role.into(),
            password_hash: None,
        };
        tx.execute(stmt(
            "INSERT INTO users(id,email,display_name,password_hash,role,created_ms) VALUES($1,$2,$3,NULL,$4,$5)",
            [account.id.clone().into(), account.email.clone().into(), account.display_name.clone().into(), account.role.clone().into(), now.into()],
        )).await.context("创建 OAuth 用户失败")?;
        tx.execute(stmt(
            "INSERT INTO oauth_identities(provider,provider_user_id,user_id) VALUES($1,$2,$3)",
            [
                provider.into(),
                profile.provider_user_id.clone().into(),
                account.id.clone().into(),
            ],
        ))
        .await?;
        tx.execute(stmt(
            "INSERT INTO audit_events(id,action,target,actor_user_id,created_ms) VALUES($1,$2,$3,$3,$4)",
            [
                uuid::Uuid::new_v4().to_string().into(),
                format!("auth.oauth.{provider}").into(),
                account.id.clone().into(),
                now.into(),
            ],
        ))
        .await?;
        tx.commit().await?;
        Ok(Some(account))
    }

    async fn cleanup(&self, now: i64) -> anyhow::Result<()> {
        for (sql, cutoff) in [
            ("DELETE FROM auth_sessions WHERE expires_ms<$1", now),
            ("DELETE FROM email_codes WHERE expires_ms<$1", now),
            ("DELETE FROM oauth_states WHERE expires_ms<$1", now),
            (
                "DELETE FROM login_attempts WHERE window_start_ms<$1",
                now - 900_000,
            ),
        ] {
            self.db.execute(stmt(sql, [cutoff.into()])).await?;
        }
        Ok(())
    }
}
