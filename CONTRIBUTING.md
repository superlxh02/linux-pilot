# 参与贡献

感谢关注 Linux-Pilot。请先在仓库 Issue 中描述缺陷或需求，说明 Linux 内核版本、部署方式、相关日志和复现步骤；日志中请删除令牌、Cookie、邮箱地址及其他敏感信息。

## 本地检查

1. Web 后端：`cd web && cargo fmt --all --check && cargo test --locked --workspace && cargo clippy --locked --workspace --all-targets -- -D warnings`。
2. Worker：`cd worker && cargo fmt --all --check && cargo test --locked --workspace`。eBPF/perf 行为需要在 Linux 上用独立 Worker 镜像验证。
3. 前端：`cd web/frontend && npm ci && npm run build`。
4. 修改 `contracts/` 时，同步 Worker 与 Web 的模型、Protobuf 和生成配置，并运行 `bash web/scripts/check-contracts.sh`。

## 代码约定

- 优先写清晰的类型、函数名和错误上下文。公共接口使用中文 `///` 文档注释，复杂分支用中文解释约束和原因；避免逐行复述代码。
- 保持 DDD 依赖方向：接口和基础设施依赖应用层端口，评分领域不依赖 HTTP 或数据库。
- 新指标必须说明单位、来源、累计/瞬时口径和聚合方式，并更新 [指标字典](web/docs/metrics.md)。缺失值不可当作零参与评分。
- 不在源码、配置示例或测试记录中提交真实密钥、邮箱验证码和生产环境数据。
- 每个变更应能分别从 `worker/` 和 `web/` 构建；不要恢复根目录 Cargo 工作区或让 Web Docker 构建读取 Worker 目录。
