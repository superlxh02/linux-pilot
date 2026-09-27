//! Worker 与中心端之间的版本化 gRPC 消息。
//!
//! 产品名称改为 Linux-Pilot 后仍保留 `po.agent.v1` wire namespace，
//! 让已有 v1 Worker 能与升级后的中心端滚动部署。修改字段时优先新增
//! 字段号，不能重用旧字段号或改变现有字段含义。
pub mod agent {
    tonic::include_proto!("po.agent.v1");
}
