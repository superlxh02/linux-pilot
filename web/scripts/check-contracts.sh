#!/usr/bin/env bash
set -Eeuo pipefail

# 两套独立 Cargo 工作区必须能单独构建，因此协议契约各保留一份。
# 修改指标模型或 Protobuf 后同步两侧副本；此检查防止独立部署时出现协议漂移。
root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
for relative in model/Cargo.toml model/src/lib.rs wire/Cargo.toml wire/src/lib.rs wire/build.rs wire/proto/agent.proto; do
  if ! diff -u "$root_dir/worker/contracts/$relative" "$root_dir/web/contracts/$relative"; then
    echo "契约不一致：$relative" >&2
    exit 1
  fi
done
echo "Worker 与 Web 的契约一致"
