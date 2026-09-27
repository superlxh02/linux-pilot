#!/usr/bin/env sh
set -eu

# 从任意工作目录调用都只读写当前 Worker 项目的探针对象。
WORKER_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

# 同一份 ELF 可供 x86_64 和 arm64 Linux Agent 加载。
# macOS 开发机优先使用 Homebrew LLVM，Linux 使用系统 clang。
if [ -x /opt/homebrew/opt/llvm/bin/clang ]; then
  CLANG=/opt/homebrew/opt/llvm/bin/clang
else
  CLANG=clang
fi
"$CLANG" -O2 -g -target bpfel -c "$WORKER_DIR/ebpf/pilot.bpf.c" -o "$WORKER_DIR/ebpf/pilot.bpf.o"
