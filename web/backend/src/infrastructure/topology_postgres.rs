//! TopologyManifest 与 Pod 发现记录的 PostgreSQL 适配器。
//!
//! SQL 参数均通过 SeaORM 绑定；Pod 指标查询选用最浅的 Pod cgroup，
//! 因为 cgroup v2 的父层统计已包含子容器，重复累加会高估服务用量。

use crate::application::ports::TopologyRepository;
use anyhow::{Context, ensure};
use async_trait::async_trait;
use linux_pilot_scoring::topology::{PodObservation, TopologyManifest};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use std::collections::BTreeMap;

pub struct PostgresTopologyRepository {
    db: DatabaseConnection,
}

impl PostgresTopologyRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn stmt(sql: &str, values: impl IntoIterator<Item = sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

#[async_trait]
impl TopologyRepository for PostgresTopologyRepository {
    async fn manifest(&self) -> anyhow::Result<Option<TopologyManifest>> {
        let row = self
            .db
            .query_one(stmt(
                "SELECT payload::text AS payload FROM topology_manifest WHERE id='active'",
                std::iter::empty::<sea_orm::Value>(),
            ))
            .await?;
        row.map(|row| {
            let payload: String = row.try_get("", "payload")?;
            serde_json::from_str(&payload).context("解析已存储的拓扑配置失败")
        })
        .transpose()
    }

    async fn apply_manifest(&self, manifest: &TopologyManifest) -> anyhow::Result<()> {
        let revision = i64::try_from(manifest.metadata.revision).context("配置版本过大")?;
        let payload = serde_json::to_string(manifest)?;
        // 单 SQL 的 WHERE 条件实现乐观锁。并发写只有一个版本能成功；
        // 客户端必须读取最新 revision 并提交 +1，避免覆盖他人修改。
        let applied = self.db.query_one(stmt(
            "INSERT INTO topology_manifest(id,revision,payload,applied_ms) SELECT 'active',$1,$2::jsonb,$3 WHERE $1=1 OR EXISTS(SELECT 1 FROM topology_manifest WHERE id='active') ON CONFLICT(id) DO UPDATE SET revision=EXCLUDED.revision,payload=EXCLUDED.payload,applied_ms=EXCLUDED.applied_ms WHERE topology_manifest.revision=EXCLUDED.revision-1 RETURNING revision",
            [revision.into(), payload.into(), chrono::Utc::now().timestamp_millis().into()],
        )).await?;
        ensure!(
            applied.is_some(),
            "配置版本冲突：首次 revision 必须为 1，之后从当前版本递增 1"
        );
        Ok(())
    }

    async fn upsert_pods(&self, observations: &[PodObservation]) -> anyhow::Result<()> {
        for pod in observations {
            self.db.execute(stmt(
                "INSERT INTO pod_observations(cluster_id,pod_uid,pod_name,node_id,namespace,workload_kind,workload_name,ready,observed_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(cluster_id,pod_uid) DO UPDATE SET pod_name=EXCLUDED.pod_name,node_id=EXCLUDED.node_id,namespace=EXCLUDED.namespace,workload_kind=EXCLUDED.workload_kind,workload_name=EXCLUDED.workload_name,ready=EXCLUDED.ready,observed_ms=EXCLUDED.observed_ms WHERE pod_observations.observed_ms <= EXCLUDED.observed_ms",
                [pod.cluster_id.clone().into(),pod.pod_uid.clone().into(),pod.pod_name.clone().into(),pod.node_id.clone().into(),pod.namespace.clone().into(),pod.workload_kind.clone().into(),pod.workload_name.clone().into(),pod.ready.into(),pod.observed_ms.into()],
            )).await?;
        }
        Ok(())
    }

    async fn pods(&self, cluster_id: &str, since_ms: i64) -> anyhow::Result<Vec<PodObservation>> {
        let rows = self.db.query_all(stmt(
            "SELECT cluster_id,pod_uid,pod_name,node_id,namespace,workload_kind,workload_name,ready,observed_ms FROM pod_observations WHERE cluster_id=$1 AND observed_ms>=$2 ORDER BY namespace,workload_name,pod_name LIMIT 2000",
            [cluster_id.into(),since_ms.into()],
        )).await?;
        rows.into_iter()
            .map(|row| {
                Ok(PodObservation {
                    cluster_id: row.try_get("", "cluster_id")?,
                    pod_uid: row.try_get("", "pod_uid")?,
                    pod_name: row.try_get("", "pod_name")?,
                    node_id: row.try_get("", "node_id")?,
                    namespace: row.try_get("", "namespace")?,
                    workload_kind: row.try_get("", "workload_kind")?,
                    workload_name: row.try_get("", "workload_name")?,
                    ready: row.try_get("", "ready")?,
                    observed_ms: row.try_get("", "observed_ms")?,
                })
            })
            .collect()
    }

    async fn pod_metrics(
        &self,
        host_id: &str,
        pod_uid: &str,
        since_ms: i64,
    ) -> anyhow::Result<BTreeMap<String, f64>> {
        let rows = self.db.query_all(stmt(
            "SELECT DISTINCT ON (labels->>'cgroup',name) labels->>'cgroup' AS cgroup,name,value FROM metrics WHERE host_id=$1 AND labels->>'pod_uid'=$2 AND time_ms>=$3 AND name LIKE 'cgroup.%' AND labels ? 'cgroup' ORDER BY labels->>'cgroup',name,time_ms DESC LIMIT 500",
            [host_id.into(),pod_uid.into(),since_ms.into()],
        )).await?;
        let mut groups: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
        for row in rows {
            let group: String = row.try_get("", "cgroup")?;
            let name: String = row.try_get("", "name")?;
            let value: f64 = row.try_get("", "value")?;
            groups.entry(group).or_default().insert(name, value);
        }
        // 最浅的 Pod cgroup 汇总了其子容器。若只看到了容器层，则返回
        // 最短路径并在 API 标识低覆盖；绝不把父子层同时加到服务总量。
        let mut selected = groups
            .into_iter()
            .min_by_key(|(path, _)| path.len())
            .map(|(_, metrics)| metrics)
            .unwrap_or_default();
        // Pod 网卡计数独立于 cgroup v2，按 pod_uid 聚合后追加，不与
        // 同一 Pod 的每进程网络数据相加。
        let network = self.db.query_all(stmt(
            "SELECT DISTINCT ON (name) name,value FROM metrics WHERE host_id=$1 AND labels->>'pod_uid'=$2 AND time_ms>=$3 AND name LIKE 'pod.net_%' ORDER BY name,time_ms DESC LIMIT 8",
            [host_id.into(),pod_uid.into(),since_ms.into()],
        )).await?;
        for row in network {
            selected.insert(row.try_get("", "name")?, row.try_get("", "value")?);
        }
        Ok(selected)
    }
}
