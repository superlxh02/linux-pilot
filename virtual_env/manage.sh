#!/usr/bin/env bash
set -Eeuo pipefail

# 本地三节点实验环境与 Web Compose 分离；不会更改用户当前 kubectl context。
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
name="linux-pilot-sim"
context="kind-$name"
kubectl_bin="${KUBECTL_BIN:-/opt/homebrew/bin/kubectl}"
if [[ ! -x "$kubectl_bin" ]]; then kubectl_bin="$(command -v kubectl)"; fi

token_from_env() {
  python3 - "$root/.env" "$1" <<'PY'
import sys
for line in open(sys.argv[1], encoding="utf-8"):
    if line.startswith(sys.argv[2] + "="):
        print(line.split("=", 1)[1].rstrip("\n"), end="")
        break
PY
}

ensure_secret() {
  [[ -f "$root/.env" ]] || "$root/web/scripts/init-env.sh"
  local generated
  generated="$(mktemp -d)"
  trap 'rm -rf "$generated"' RETURN
  token_from_env AGENT_TOKEN > "$generated/agent-token"
  token_from_env OPERATOR_TOKEN > "$generated/operator-token"
  if [[ ! -s "$generated/agent-token" || ! -s "$generated/operator-token" ]]; then
    echo '.env 缺少 AGENT_TOKEN 或 OPERATOR_TOKEN' >&2; exit 1
  fi
  "$kubectl_bin" --context "$context" -n linux-pilot-demo create secret generic pilot-credentials \
    --from-file=agent-token="$generated/agent-token" \
    --from-file=operator-token="$generated/operator-token" \
    --dry-run=client -o yaml | "$kubectl_bin" --context "$context" apply -f -
}

case "${1:-}" in
  up)
    command -v kind >/dev/null
    docker info >/dev/null
    if ! kind get clusters | grep -qx "$name"; then
      kind create cluster --name "$name" --config "$root/virtual_env/kind.yaml" --wait 5m
    fi
    docker compose -f "$root/compose.yaml" up -d --build
    docker build -t linux-pilot-demo:local "$root/virtual_env/services"
    docker build -t linux-pilot-worker:local "$root/worker"
    kind load docker-image linux-pilot-demo:local linux-pilot-worker:local --name "$name"
    "$kubectl_bin" --context "$context" apply -f "$root/virtual_env/k8s.yaml"
    "$kubectl_bin" --context "$context" -n linux-pilot-demo create configmap pilot-discovery-code \
      --from-file=discovery.py="$root/virtual_env/discovery.py" --dry-run=client -o yaml |
      "$kubectl_bin" --context "$context" apply -f -
    ensure_secret
    "$kubectl_bin" --context "$context" apply -f "$root/virtual_env/agents.yaml"
    "$kubectl_bin" --context "$context" -n linux-pilot-demo rollout status deployment/catalog --timeout=180s
    "$kubectl_bin" --context "$context" -n linux-pilot-demo rollout status deployment/orders --timeout=180s
    "$kubectl_bin" --context "$context" -n linux-pilot-demo rollout status daemonset/pilot-worker --timeout=180s
    "$kubectl_bin" --context "$context" -n linux-pilot-demo rollout status deployment/pilot-discovery --timeout=180s
    "$kubectl_bin" --context "$context" -n linux-pilot-demo exec deployment/catalog -- python -c \
      'import json,urllib.request; r=urllib.request.Request("http://127.0.0.1:8080/items",json.dumps({"sku":"pilot-demo","quantity":1000000}).encode(),{"Content-Type":"application/json"},method="POST"); print(urllib.request.urlopen(r).read().decode())'
    # 配置版本只在首次导入；以后由用户显式增加 revision，避免覆盖调整。
    PILOT_OPERATOR_TOKEN="$(token_from_env OPERATOR_TOKEN)" python3 - "$root/virtual_env/manifest.json" <<'PY'
import json, os, sys, urllib.error, urllib.request
base = "http://127.0.0.1:3000/api/v1/topology/manifest"
headers = {"Authorization": "Bearer " + os.environ["PILOT_OPERATOR_TOKEN"], "Content-Type": "application/json"}
try:
    with urllib.request.urlopen(urllib.request.Request(base, headers=headers), timeout=10) as response:
        existing = json.load(response)
    if existing is not None:
        print("已有拓扑配置；保留当前 revision，未覆盖")
        sys.exit(0)
    body = open(sys.argv[1], "rb").read()
    with urllib.request.urlopen(urllib.request.Request(base, body, headers, method="PUT"), timeout=10) as response:
        print("拓扑配置已导入:", response.read().decode())
except urllib.error.URLError as error:
    raise SystemExit(f"拓扑导入失败：{error}")
PY
    echo '集群就绪：http://127.0.0.1:18080/health；控制台：http://127.0.0.1:3000'
    ;;
  status)
    "$kubectl_bin" --context "$context" get nodes -o wide
    "$kubectl_bin" --context "$context" -n linux-pilot-demo get pods -o wide
    ;;
  load)
    shift
    python3 "$root/virtual_env/load.py" "$@"
    ;;
  down)
    kind delete cluster --name "$name"
    ;;
  *)
    echo '用法：virtual_env/manage.sh up|status|load [--duration 60 --rate 4 --mode mixed]|down' >&2
    exit 2
    ;;
esac
