#!/usr/bin/env bash
set -Eeuo pipefail

# Worker 单独部署脚本。Web Compose 不管理采集端；每台 Linux 主机运行自己的容器。
# 缓冲区使用独立 Docker 卷，stop/restart 只移除容器，不删除未 ACK 的采集批次。
worker_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
image="${PILOT_WORKER_IMAGE:-linux-pilot-worker:local}"
container="${PILOT_WORKER_CONTAINER:-linux-pilot-worker}"
data_volume="${PILOT_WORKER_VOLUME:-linux-pilot-worker-data}"
env_file="${PILOT_WORKER_ENV_FILE:-$worker_dir/.env}"

usage() {
  cat <<'EOF'
用法: ./manage.sh build|start|stop|restart|status|logs

  build    在当前 worker/ 目录构建独立镜像
  start    使用 worker/.env 启动采集容器，缓冲数据放在持久卷
  stop     停止并移除容器，保留缓冲数据卷
  restart  重新创建容器，使修改后的配置或镜像生效
  status   查看容器运行状态
  logs     持续查看采集日志

可通过 PILOT_WORKER_ENV_FILE、PILOT_WORKER_IMAGE、PILOT_WORKER_CONTAINER
和 PILOT_WORKER_VOLUME 覆盖默认值。自建 CA 可设置 PILOT_WORKER_CA_DIR，
脚本会把宿主机该目录只读挂载为容器的 /etc/linux-pilot。
EOF
}

require_docker() {
  if ! command -v docker >/dev/null 2>&1; then
    echo "未找到 docker，请先安装 Docker Engine。" >&2
    exit 1
  fi
  docker info >/dev/null
}

require_config() {
  if [[ ! -f "$env_file" ]]; then
    echo "缺少 $env_file；请复制 worker/.env.example 后填写中心地址和令牌。" >&2
    exit 1
  fi
  if ! grep -Eq '^PO_AGENT__SERVER_URL=.+$' "$env_file" ||
     ! grep -Eq '^PO_AGENT__TOKEN=.+$' "$env_file"; then
    echo "配置必须包含 PO_AGENT__SERVER_URL 与 PO_AGENT__TOKEN。" >&2
    exit 1
  fi
}

build() {
  docker build -f "$worker_dir/Dockerfile" -t "$image" "$worker_dir"
}

start() {
  require_config
  if docker container inspect "$container" >/dev/null 2>&1; then
    if [[ "$(docker inspect -f '{{.State.Running}}' "$container")" == true ]]; then
      echo "$container 已在运行"
      return
    fi
    docker container rm "$container" >/dev/null
  fi
  if ! docker image inspect "$image" >/dev/null 2>&1; then
    build
  fi
  mounts=(
    --mount type=bind,source=/sys,target=/sys,readonly
    --mount "type=volume,source=$data_volume,target=/var/lib/linux-pilot-worker"
  )
  if [[ -n "${PILOT_WORKER_CA_DIR:-}" ]]; then
    if [[ ! -d "$PILOT_WORKER_CA_DIR" ]]; then
      echo "CA 目录不存在：$PILOT_WORKER_CA_DIR" >&2
      exit 1
    fi
    mounts+=(--mount "type=bind,source=$PILOT_WORKER_CA_DIR,target=/etc/linux-pilot,readonly")
  fi
  # eBPF、perf 和宿主机进程观测需要 Linux 内核权限；采集端只主动连接中心。
  docker run -d \
    --name "$container" \
    --restart unless-stopped \
    --privileged \
    --pid host \
    --network host \
    "${mounts[@]}" \
    --env-file "$env_file" \
    "$image" >/dev/null
  echo "$container 已启动；运行 ./manage.sh logs 查看连接状态"
}

stop() {
  if docker container inspect "$container" >/dev/null 2>&1; then
    docker container rm -f "$container" >/dev/null
    echo "$container 已停止；$data_volume 保留"
  else
    echo "$container 未运行"
  fi
}

require_docker
case "${1:-}" in
  build) build ;;
  start) start ;;
  stop) stop ;;
  restart) stop; start ;;
  status) docker container inspect -f '{{.Name}}: {{.State.Status}}' "$container" 2>/dev/null || echo "$container 未运行" ;;
  logs) docker logs -f --tail 100 "$container" ;;
  *) usage; exit 2 ;;
esac
