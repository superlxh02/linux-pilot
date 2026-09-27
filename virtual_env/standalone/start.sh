#!/usr/bin/env bash
set -Eeuo pipefail

# 同一模拟节点中的业务进程与 Worker 一起生存；任一进程退出，容器退出并
# 交由 Compose 重启。Worker 的缓冲卷在重启后仍保留 ACK 序号和传输 epoch。
python /opt/linux-pilot/service.py &
service_pid=$!
linux-pilot-worker &
worker_pid=$!
shutdown() {
  kill "$service_pid" "$worker_pid" 2>/dev/null || true
  wait "$service_pid" "$worker_pid" 2>/dev/null || true
}
trap shutdown EXIT TERM INT
wait -n "$service_pid" "$worker_pid" || true
exit 1
