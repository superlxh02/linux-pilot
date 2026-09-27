#!/usr/bin/env bash
set -Eeuo pipefail

# 首次部署生成本机 .env。数据库、Worker 和验证码口令使用操作系统
# 随机源；管理员使用文档公开的固定初始密码，便于首次登录。
root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
example="$root_dir/.env.example"
target="$root_dir/.env"

if [[ -e "$target" ]]; then
  echo "$target 已存在；脚本不会覆盖已有密钥或数据库密码。" >&2
  exit 1
fi
if ! command -v openssl >/dev/null 2>&1; then
  echo "需要 openssl 生成密码，请先安装。" >&2
  exit 1
fi

umask 077
temporary="$(mktemp "$root_dir/.env.XXXXXX")"
trap 'rm -f "$temporary"' EXIT
while IFS= read -r line || [[ -n "$line" ]]; do
  case "$line" in
    POSTGRES_PASSWORD=*) printf 'POSTGRES_PASSWORD=%s\n' "$(openssl rand -hex 24)" ;;
    AGENT_TOKEN=*) printf 'AGENT_TOKEN=%s\n' "$(openssl rand -hex 24)" ;;
    VIEWER_TOKEN=*) printf 'VIEWER_TOKEN=%s\n' "$(openssl rand -hex 24)" ;;
    OPERATOR_TOKEN=*) printf 'OPERATOR_TOKEN=%s\n' "$(openssl rand -hex 24)" ;;
    PO_CODE_PEPPER=*) printf 'PO_CODE_PEPPER=%s\n' "$(openssl rand -hex 24)" ;;
    *) printf '%s\n' "$line" ;;
  esac
done < "$example" > "$temporary"
mv "$temporary" "$target"
trap - EXIT
echo "已生成 ${target}（权限 0600）。管理员用户名：admin；密码请在本机查看 .env 的 ADMIN_PASSWORD。"
