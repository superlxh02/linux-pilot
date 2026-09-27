# Linux-Pilot 本地双模式实验环境

`virtual_env/` 提供两套可同时运行的 Linux 模拟环境。两套环境共享根目录 `compose.yaml` 部署的 Web、PostgreSQL 和拓扑配置，但各自拥有独立的示例服务、Worker 和负载脚本。

| 模式 | 本地实例 | Worker | 业务入口 | 验证重点 |
| --- | --- | --- | --- | --- |
| 普通主机集群 | 3 个 Docker 容器，各有独立 PID/cgroup 命名空间 | 每个容器 1 个，与单体服务同容器 | `18081` billing、`18082` search、`18083` reports | 主机、进程、单体服务归属和资源曲线 |
| Kubernetes 集群 | kind：1 个 control-plane、2 个 worker Node | **每个 Node 1 个** DaemonSet Pod，包括 control-plane | `18080` orders | Node、Pod、跨 Node 服务聚合 |

这些容器都运行在 Mac 的同一个 Docker Linux VM 中，**不等同独立物理服务器**。它们能够验证部署和归属链路，无法提供三台物理机的性能基准。Kubernetes Node 在真实环境中可以是物理机或虚拟机；一个 Node 可承载多个服务的 Pod，一个服务也可有跨多个 Node 的副本。kind 为方便本地测试，将每个 Node 实现为容器。

## 准备

- macOS 的 Docker Engine/Compose v2。两套环境同时运行时建议 Docker VM 至少 8 GiB 内存。
- Kubernetes 模式还需 `kind` 与兼容版本的 `kubectl`，例如 `brew install kind kubernetes-cli`。
- 首次启动自动通过 `web/scripts/init-env.sh` 创建根目录 `.env`；不要提交该文件。
- 端口 `3000`、`50051`、`18080`～`18083` 空闲。

Web 的 Compose 始终只有前端、后端和数据库；以下脚本会先启动它，再启动对应的实验环境。可以先启动任意一个模式，再启动另一个模式，并在网页左侧随时切换。

控制台使用 <http://localhost:3000>。WebSocket 握手按 `PO_PUBLIC_URL` 检查 Origin；默认配置为 `localhost`，请用同一主机名访问，避免实时连接反复重试。

## 普通主机模式

```bash
./virtual_env/standalone/manage.sh up
./virtual_env/standalone/manage.sh status
./virtual_env/standalone/manage.sh load --duration 60 --rate 6 --service all --mode mixed
./virtual_env/standalone/manage.sh down
```

三个容器分别运行独立的 Python HTTP 单体应用及 Rust Worker。业务请求直接产生 CPU 计算、SQLite 事务、磁盘写入或内存工作集，Worker 从实际 Linux `/proc` 和 cgroup 读取结果。`load` 的 `--service` 可选 `all|billing|search|reports`，`--mode` 可选 `mixed|transaction|cpu|io|memory`，`--rate` 最多 30 次/秒。

```bash
curl http://127.0.0.1:18081/health
curl -X POST 'http://127.0.0.1:18082/work/cpu?ms=500' -d '{}'
curl -X POST 'http://127.0.0.1:18083/work/io?kib=512' -d '{}'
curl http://127.0.0.1:18083/stats
```

在控制台选择「普通主机」后，可看到三个节点、每台机器上的单体服务、节点分、进程清单、进程详细资源与 perf。普通主机服务用 `hostId + Linux comm` 选择当前进程，不绑定会在重启后变化的 PID；当前实例的指标查询仍用 `PID + start_ticks` 防止复用误归因。

`down` 仅停止这三个模拟主机；Docker 命名卷中的 SQLite 和 Worker 待确认批次会保留。若需要清除实验数据，可自行删除 `linux-pilot-standalone` 项目的卷。

## Kubernetes 模式

```bash
./virtual_env/manage.sh up
./virtual_env/manage.sh status
./virtual_env/manage.sh load --duration 60 --rate 4 --mode mixed
./virtual_env/manage.sh down
```

`up` 创建 `linux-pilot-sim` kind 集群，部署 `orders` 两个副本、`catalog` 一个副本、Pod 发现器及 Worker DaemonSet。两个 `orders` Pod 分散在两个工作 Node，调用 `catalog` 的 SQLite 库存服务。DaemonSet 容忍控制面的污点，所以三个 Node 各有一个 Worker。示例业务只放在工作 Node。脚本使用显式 `--context kind-linux-pilot-sim`，不修改现有 kubectl context。

```bash
curl http://127.0.0.1:18080/health
curl -X POST http://127.0.0.1:18080/orders -H 'Content-Type: application/json' -d '{"sku":"pilot-demo","quantity":1}'
curl -X POST 'http://127.0.0.1:18080/work/cpu?ms=500' -d '{}'
```

在控制台选择「Kubernetes」后，先看到服务与副本，再看 Node；「微服务」页面按 Pod 父 cgroup 展示 CPU、内存、I/O 以及按 Pod 网络命名空间统计的收发速率。Kubernetes 模式没有进程监控入口。工作负载选择器使用 Namespace、Kind、Name 这组稳定身份，Pod UID 由发现器动态上报。

`down` 删除 kind 集群与其中的示例 SQLite 数据，但不删除 Web PostgreSQL 卷，也不影响普通主机模式。

## 可视化拓扑和 JSON

[manifest.json](manifest.json) 使用 `linux-pilot.io/v1alpha1` 契约，定义两个 cluster、六个 Node、五个 Service 和可重用的场景评分配置。Web「拓扑配置」页面可编辑节点描述、角色、CPU/内存/存储申报容量、评分 Profile 与权重，以及服务到 Pod 或主机进程的匹配规则。它支持 JSON 导入、导出、验证和按 revision 乐观锁保存；用户改过的配置不会被启动脚本自动覆盖。

| API | 用途 |
| --- | --- |
| `POST /api/v1/topology/validate` | 只校验配置 |
| `PUT /api/v1/topology/manifest` | 保存配置，revision 必须递增 |
| `GET /api/v1/topology/manifest` | 读取当前配置 |
| `POST /api/v1/topology/pods` | 发现器上报 Pod 归属 |
| `GET /api/v1/topology/snapshot` | 两类集群的节点、服务和资源快照 |

Node 分由场景 Profile 的五维规则权重计算。Kubernetes 集群资源分来自已覆盖的服务 Pod；普通主机集群资源分来自完整的 Node 分。缺测保留空值。业务整体分仍为 `null`，因为请求成功率、延迟与 SLO 尚未接入；资源分不等于业务健康分。Pod 网络数据属于共享网络命名空间，不能直接归因于单个进程。`systemd-unit` 选择器尚未实现，导入时会明确拒绝。

## 采集边界与排查

本地 Worker 使用 `privileged` 读取内核数据，仅为实验方便。kind 节点嵌套 PID 命名空间可能让 eBPF 初始命名空间的 TGID 与节点内进程 PID 不一致，因此进程 Socket 归因可能为空。Docker 模拟主机的 `/proc/stat` 等内核全局计数器也可能反映整个共享 VM，而非容器独占物理硬件；进程与 cgroup 归属更适合本实验验收。

```bash
./virtual_env/standalone/manage.sh status
./virtual_env/manage.sh status
kubectl --context kind-linux-pilot-sim -n linux-pilot-demo logs daemonset/pilot-worker --tail=50 --prefix=true
kubectl --context kind-linux-pilot-sim -n linux-pilot-demo logs deployment/pilot-discovery --tail=50
```

若 Pod 已运行但服务覆盖率仍为 0，检查发现器、Worker 到 Web gRPC 的连接，以及 cgroup v2 挂载。macOS 主机进程不会出现在这两套 Linux 模拟环境的进程列表。
