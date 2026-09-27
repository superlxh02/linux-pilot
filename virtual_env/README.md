# Linux-Pilot 三节点 Kubernetes 实验环境

`virtual_env/` 提供一个可重复创建的 **kind 集群**：1 个控制节点、2 个工作节点。节点是 Docker Desktop/OrbStack Linux VM 内的容器，能够验证 Kubernetes 调度、Pod UID 归属、跨节点网络和服务聚合；它们**共享同一台 Mac 的 CPU、内存与磁盘**，不能用于对比独立物理机性能。

## 前提与启动

- macOS 上可运行的 Docker Engine/Compose v2；建议给 Docker Linux VM 至少 8 GiB 内存。
- `kind` 和与集群相近版本的 `kubectl`。例如 `brew install kind kubernetes-cli`。
- 项目根目录的 `.env`。若缺失，脚本会调用 `web/scripts/init-env.sh` 生成随机本地口令。
- 端口 `3000`、`50051`、`18080` 未被其他服务占用。

```bash
./virtual_env/manage.sh up
./virtual_env/manage.sh status
./virtual_env/manage.sh load --duration 60 --rate 4 --mode mixed
./virtual_env/manage.sh down
```

`up` 会构建 Web、Worker 与示例服务镜像；创建 `linux-pilot-sim` 集群；导入镜像；部署服务、发现器和每工作节点一个 Worker；初始化库存；首次导入 [拓扑配置](manifest.json)。重复执行保留已修改的配置版本。`down` 删除整个 kind 集群及其中的演示数据，**不会**删除项目 Web 的 PostgreSQL 数据卷或正在运行的 Web Compose 服务。

Web 控制台在 <http://127.0.0.1:3000>，订单入口在 <http://127.0.0.1:18080>。`kubectl` 命令可加 `--context kind-linux-pilot-sim`，脚本不会修改你原本的上下文。

## 实际部署内容

| 组件 | 位置 | 行为 |
| --- | --- | --- |
| `orders` Deployment | 两个副本，分别放在两个工作节点 | 接收订单，跨 Pod/节点调用 `catalog` 扣减库存，然后写入本地 SQLite；还可产生 CPU、磁盘与内存负载 |
| `catalog` Deployment | 第二个工作节点 | SQLite 库存、原子扣减和读取 |
| `pilot-worker` DaemonSet | 每个工作节点 | 以节点名为 `host_id` 发送实际 `/proc`、cgroup 和 Pod 网络命名空间指标至 Web gRPC；不注入假指标 |
| `pilot-discovery` Deployment | 集群内 | 每 15 秒用只读 RBAC 发现 Pod、ReplicaSet、Deployment 归属，向 Web 拓扑 API 上报 Pod UID 映射 |

Worker 为了读取节点内核探针使用 `privileged`、`hostPID` 和 `/sys` 挂载，**仅适用于本地实验**。生产部署应单独评估所需 Linux capabilities、seccomp、内核版本、BTF 和数据保留策略。Worker 缓冲目录挂在 kind 节点的 `/var/lib/linux-pilot-worker-sim`，DaemonSet 滚动重启后仍保留待确认批次与递增序号；删除 kind 集群后节点容器和这份缓冲一同删除，新集群会生成新的传输 epoch。示例 SQLite 位于 `emptyDir`，Pod 重建后演示数据会丢失；重跑 `up` 会重新初始化库存。

kind 节点本身位于 Docker 的嵌套 PID 命名空间。当前 eBPF 进程 Socket 探针使用内核初始命名空间的 TGID，而本实验进程清单使用 kind 节点中的 PID；两者可能无法对应，因此进程 Socket 卡片可能显示“—”。节点级探针调用次数和 Pod 网络命名空间吞吐仍可验证。部署在真实 Linux 主机 PID 命名空间的 Worker 没有这层 kind PID 映射，但进程 Socket 指标仍只覆盖前述四类系统调用。

## 手动操作与负载

```bash
curl http://127.0.0.1:18080/health
curl -X POST http://127.0.0.1:18080/orders \
  -H 'Content-Type: application/json' -d '{"sku":"pilot-demo","quantity":1}'
curl -X POST 'http://127.0.0.1:18080/work/cpu?ms=500' -d '{}'
curl -X POST 'http://127.0.0.1:18080/work/io?kib=512' -d '{}'
curl -X POST 'http://127.0.0.1:18080/work/memory?mib=8' -d '{}'
```

`load` 的 `--mode` 可选 `mixed`、`orders`、`cpu`、`io`、`memory`；`--rate` 最多 30 次/秒，单个 CPU 请求最多运行 2 秒，内存工作集最多保留 64 MiB。运行前可在页面“集群与服务”观察基线；运行期间查看订单服务跨两个节点的 CPU、内存、磁盘与 Pod 网络吞吐，固定监控一个 Python 进程后可查看 PSS、FD 和线程调度。

## 拓扑 JSON 与评分口径

[manifest.json](manifest.json) 是可导入/导出的 `linux-pilot.io/v1alpha1` 契约。`metadata.revision` 首次为 1，每次更新加 1。`spec.nodes` 定义稳定 `id`、功能、描述、申报的 CPU/内存/存储容量和 `scoreProfileRef`；`scoreProfiles` 选择确定性的 `rule-based/v1` 与五维权重；`services[].match` 以命名空间、Deployment 等稳定工作负载身份匹配动态 Pod。Pod UID、Pod 名、实际采集指标**不写进**配置文件。后端提供：

| API | 权限 | 用途 |
| --- | --- | --- |
| `POST /api/v1/topology/validate` | 操作员 | 只校验 JSON，不保存 |
| `PUT /api/v1/topology/manifest` | 操作员 | 按 revision 乐观锁应用 |
| `GET /api/v1/topology/manifest` | 登录用户 | 导出当前配置 |
| `POST /api/v1/topology/pods` | 操作员 | 发现器上报 Pod 归属，运行时事实 |
| `GET /api/v1/topology/snapshot` | 登录用户 | 当前节点、服务与 Pod 跨节点资源快照 |

节点评分使用所选 Profile 的权重。服务基础设施分只用已收到的 Pod cgroup 资源与 Ready 状态；父 Pod cgroup 已包含子容器，查询只取父层一次。任一已发现 Pod 缺失有效评分指标时，该服务分留空；关键服务缺分时，集群基础设施分也留空。**业务整体分目前返回 `null`**，因为端到端请求成功率、延迟、业务 SLO 尚未接入；界面将它显示为“待接入”，避免把 CPU 健康误认为业务健康。Pod 网络速率来自共享网络命名空间，不能等同于某个进程的网络消耗。`systemd-unit` 选择器形状已预留，但当前版本在导入时明确拒绝，待实现 unit 发现后启用。

## 排查

```bash
./virtual_env/manage.sh status
kubectl --context kind-linux-pilot-sim -n linux-pilot-demo logs deployment/pilot-discovery --tail=50
kubectl --context kind-linux-pilot-sim -n linux-pilot-demo logs daemonset/pilot-worker --tail=50 --prefix=true
kubectl --context kind-linux-pilot-sim -n linux-pilot-demo describe pod -l app=pilot-worker
```

若 Pod 已运行但服务覆盖率仍为 0，检查发现器上报、Worker 与 Web gRPC 连通、`/sys/fs/cgroup` 是否为 cgroup v2，以及内核是否允许读取相应指标。macOS 主机进程不会出现在 kind Worker 的进程列表里。
