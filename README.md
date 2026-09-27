<p align="center"><img src="web/site/assets/logo.svg" width="76" height="76" alt="Linux-Pilot 图标"></p>

<h1 align="center">Linux-Pilot</h1>

<p align="center">开源 Linux 性能观测平台 · Rust Worker + Rust Web 后端 + Vue 3 控制台</p>

<p align="center"><a href="web/docs/architecture.md">架构说明</a> · <a href="web/docs/metrics.md">指标字典</a> · <a href="web/docs/admin.md">管理员说明</a> · <a href="web/docs/email.md">真实邮箱配置</a> · <a href="CONTRIBUTING.md">参与贡献</a></p>

Linux-Pilot 在目标 Linux 主机上持续采集性能指标和内核信号，将数据可靠地送往中心服务，并在浅色 Web 工作台中提供实时总览、分类查询、按需 perf 剖析、告警与可解释的场景评分。

> **项目状态：早期版本。** 单实例中心端和分布式 Worker 已可运行；规则评分不等于基准测试，AI 分析和多租户尚未实现。上线前请完成容量、备份和安全配置评估。

## 功能

| 方向 | 当前能力 |
| --- | --- |
| 主机采集 | CPU、内存、PSI、块设备、文件系统、网络、进程 Top N、cgroup v2；主机汇总默认每秒采样 |
| 内核与剖析 | Aya eBPF 的 TCP、块 I/O 与调度信号；按需 perf CPU 调用栈和火焰图 |
| 可靠传输 | Worker 主动建立 gRPC/HTTP2 双向流；本地磁盘缓冲、ACK、断线回放和批次去重 |
| 观测控制台 | 节点总览、指标探索、五场景评分、告警和性能剖析；WebSocket 实时更新 |
| 账号 | 邮箱验证码注册、Argon2id 密码、HttpOnly 会话；管理员用户管理、角色分配和操作记录 |

指标的单位、来源、含义及诊断用途见 [完整指标字典](web/docs/metrics.md)。该字典也标出了尚未实现的候选指标；页面会对缺失数据标明缺失，不会显示假零值。

## 项目结构

```text
worker/                    独立 Cargo 工作区、采集端镜像和 manage.sh
  contracts/               Worker 自用的模型与 Protobuf 契约
  ebpf/                    eBPF 源码与当前编译对象
web/                       独立 Cargo 工作区
  backend/                 Axum + SeaORM，按 DDD 分层
    domain/                纯规则评分器
    src/application/       用例与仓储/邮件/OAuth 端口
    src/infrastructure/    PostgreSQL、SMTP、OAuth 适配器
    src/interfaces/        HTTP、WebSocket、gRPC 适配器
    appliaction.yaml       层级配置文件（沿用项目指定拼写）
  contracts/               Web 自用的模型与 Protobuf 契约
  frontend/                Vue 3 控制台
  site/                    项目官网静态页面
  docs/                    指标与认证文档
compose.yaml               只包含 Web 前端、Web 后端和 PostgreSQL
```

Worker 与 Web **没有根目录 Cargo 工作区**，可分别拷贝、构建和部署。为保证独立性，版本化契约在两边各保留一份；修改协议后运行 `bash web/scripts/check-contracts.sh` 检查副本一致。Protobuf 的 `po.agent.v1` 命名空间为兼容现有 v1 数据流保留，产品名称已改为 Linux-Pilot。

## 快速开始

需要 Docker Engine/Compose v2 和 OpenSSL（用于首次生成随机口令）。Web 默认 Compose 只有前端、后端和 PostgreSQL；本地验证邮箱注册时叠加 Mailpit：

```bash
./web/scripts/init-env.sh
docker compose -f compose.yaml -f web/compose.dev.yaml up -d --build
curl http://localhost:3000/health/ready
```

打开 [Web 控制台](http://localhost:3000)。内置管理员用户名为 **`admin`**，固定初始密码及正式部署的覆盖方法见 [管理员文档](web/docs/admin.md)。普通用户可使用邮箱验证码注册，本地验证码在 [Mailpit 收件箱](http://localhost:8025) 查看。要向真实邮箱发送验证码，请按 [SMTP 配置指南](web/docs/email.md)填写邮箱服务参数，并只运行 `docker compose up -d --build`，不使用 Mailpit 叠加文件。

在需要观测的 **Linux 主机**上单独部署 Worker：

```bash
cd worker
cp .env.example .env
# 修改 .env 中的 PO_AGENT__SERVER_URL 和 PO_AGENT__TOKEN
./manage.sh start
./manage.sh status
./manage.sh logs
```

`./manage.sh build|start|stop|restart|status|logs` 管理独立 Worker 容器。`stop` 会移除容器，但保留存放未确认批次的 Docker 卷。Worker 需要 `privileged`、宿主机 PID/网络命名空间和 `/sys` 读取权限；部署前应评估目标主机的安全策略。macOS 上 Docker 采集到的是 Docker Linux VM，**不会**采集 macOS 宿主机数据。

### 验证 perf 火焰图

在运行 Worker 的 Docker Engine 上创建一个短时 CPU 负载，再把该进程的 **Docker 主机 PID** 填到控制台的「性能剖析」页面。页面也会列出所选节点近期的 CPU 热点进程供选择。若使用 Docker Desktop，以下 PID 属于其 Linux VM，macOS 活动监视器中的 PID 不能用于 Worker 采样。

```bash
docker run -d --rm --name linux-pilot-cpu-demo --entrypoint sh linux-pilot-worker:local -c 'while :; do :; done'
docker inspect -f '{{.State.Pid}}' linux-pilot-cpu-demo
# 在控制台选运行该容器的 Worker 节点，填入上述 PID，采样 10～15 秒、49 Hz
docker rm -f linux-pilot-cpu-demo
```

请在采样任务结束后再移除演示容器。`perf record` 成功但样本数为零通常表示进程在窗口内没有消耗 CPU，或进程已退出；采到事件却没有栈时，应检查目标程序的符号及帧指针。节点显示「perf 已安装」只代表命令可执行，内核权限与实际采样仍以任务结果为准。

目标主机可不开放入站端口；Worker 主动连接中心服务 `50051`。跨机器部署时，把 `PO_AGENT__SERVER_URL` 改成中心端实际可达地址，`PO_AGENT__TOKEN` 必须与 Web `.env` 中的 `AGENT_TOKEN` 相同。公网或非可信网络应为 gRPC 配置 TLS。若采用原生 systemd 安装，可参考 [Worker 服务文件](worker/linux-pilot-worker.service)。

生产环境可叠加 [gRPC TLS 配置示例](web/compose.prod.example.yaml)：设置 `PO_MODE=production`、`PO_PUBLIC_URL=https://实际控制台域名`、真实 SMTP 与 `GRPC_TLS_DIR` 后运行 `docker compose -f compose.yaml -f web/compose.prod.example.yaml up -d --build`。还需由部署环境的 HTTPS 反向代理保护 Web 入口。Worker 若使用自建 CA，设置 `PO_AGENT__CA_CERT_PATH=/etc/linux-pilot/ca.pem`，并在运行 `manage.sh` 时设置 `PILOT_WORKER_CA_DIR=/宿主机/证书目录`。

## 开发

Rust 1.98 或更高版本、Node.js 24 可用于本地开发。两端各自运行测试：

```bash
cd web && cargo fmt --all --check && cargo test --locked --workspace
cd ../worker && cargo fmt --all --check && cargo test --locked --workspace
cd ../web/frontend && npm ci && npm run build
bash ../scripts/check-contracts.sh
```

Worker 的 eBPF/perf 路径必须在 Linux 上验证；macOS Rust 构建只覆盖非 Linux 条件模块。修改 BPF 程序后运行 `worker/build-ebpf.sh`，需要支持 BPF target 的 clang。评分器是无框架依赖的纯领域服务；应用层通过端口连接数据库、邮件与 OAuth，便于将来添加 AI 分析而不改采集协议。

## 当前限制

- PostgreSQL 的指标默认保留 7 天。尚无时间分区、长期聚合和多节点容量压测。
- 实时事件由单个后端进程广播；后端多副本需要共享事件总线和任务路由。
- eBPF 的可用性依赖内核、BTF、权限和容器环境；探针失败时仍上报基础指标并标记能力。
- perf 依赖宿主机权限和符号质量；首版每台主机同一时间只运行一个任务。
- 规则评分用于定位线索，不能替代受控基准测试。AI 根因分析、自动调优和多租户尚未交付。

## 贡献与许可

欢迎提交问题和改进。提交前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)，尤其是契约同步、指标口径与独立构建要求。项目采用 [MIT License](LICENSE)。
