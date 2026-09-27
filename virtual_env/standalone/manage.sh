#!/usr/bin/env bash
set -Eeuo pipefail

# 普通主机模式：三个容器分别承载一个单体应用和一个 Worker，不使用 kind。
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
compose_file="$root/virtual_env/standalone/compose.yaml"

token_from_env() {
  python3 - "$root/.env" "$1" <<'PY'
import sys
for line in open(sys.argv[1], encoding="utf-8"):
    if line.startswith(sys.argv[2] + "="):
        print(line.split("=", 1)[1].rstrip("\n"), end="")
        break
PY
}

case "${1:-}" in
  up)
    docker info >/dev/null
    [[ -f "$root/.env" ]] || "$root/web/scripts/init-env.sh"
    [[ -n "$(token_from_env AGENT_TOKEN)" && -n "$(token_from_env OPERATOR_TOKEN)" ]] || {
      echo '.env 缺少 AGENT_TOKEN 或 OPERATOR_TOKEN' >&2; exit 1;
    }
    docker compose --env-file "$root/.env" -f "$root/compose.yaml" up -d --build
    docker build -t linux-pilot-worker:local "$root/worker"
    # 三个实例使用同一镜像：只构建一次，避免 Compose 并行构建时
    # 多个服务竞相写入同一个本地镜像标签。
    docker build -t linux-pilot-standalone:local "$root/virtual_env/standalone"
    docker compose --env-file "$root/.env" -f "$compose_file" up -d --no-build
    for port in 18081 18082 18083; do
      ready=0
      for _ in {1..40}; do
        if curl -fsS "http://127.0.0.1:$port/health" >/dev/null; then ready=1; break; fi
        sleep 1
      done
      [[ "$ready" == 1 ]] || { echo "单体节点 $port 启动超时" >&2; exit 1; }
    done
    PILOT_OPERATOR_TOKEN="$(token_from_env OPERATOR_TOKEN)" \
      python3 "$root/virtual_env/import_manifest.py" "$root/virtual_env/manifest.json"
    echo '单体集群就绪：billing 18081，search 18082，reports 18083；控制台 http://localhost:3000'
    ;;
  status)
    docker compose --env-file "$root/.env" -f "$compose_file" ps
    ;;
  load)
    shift
    python3 "$root/virtual_env/standalone/load.py" "$@"
    ;;
  down)
    docker compose --env-file "$root/.env" -f "$compose_file" down
    ;;
  *)
    echo '用法：virtual_env/standalone/manage.sh up|status|load|down' >&2
    exit 2
    ;;
esac
