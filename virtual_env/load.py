"""向真实订单/库存 Pod 发起请求，按比例制造 CPU、磁盘、内存和网络负载。"""

import argparse
import concurrent.futures
import json
import random
import time
import urllib.error
import urllib.request


def call(base, path, data=None):
    body = json.dumps(data).encode() if data is not None else None
    request = urllib.request.Request(
        base + path,
        body,
        {"Content-Type": "application/json"} if body is not None else {},
        method="POST" if body is not None or path.startswith("/work/") else "GET",
    )
    with urllib.request.urlopen(request, timeout=8) as response:
        return response.status, json.load(response)


def exercise(base, operation, index):
    if operation == "orders":
        return call(base, "/orders", {"sku": "pilot-demo", "quantity": 1})
    if operation == "cpu":
        return call(base, "/work/cpu?ms=250", {})
    if operation == "io":
        return call(base, "/work/io?kib=256", {})
    if operation == "memory":
        return call(base, "/work/memory?mib=4", {})
    return call(base, "/stats")


def main():
    parser = argparse.ArgumentParser(description="Linux-Pilot 本地 Kubernetes 负载模拟")
    parser.add_argument("--url", default="http://127.0.0.1:18080")
    parser.add_argument("--duration", type=int, default=60, help="运行秒数")
    parser.add_argument("--rate", type=float, default=4, help="每秒请求数")
    parser.add_argument("--mode", choices=["mixed", "orders", "cpu", "io", "memory"], default="mixed")
    args = parser.parse_args()
    if args.duration < 1 or not 0 < args.rate <= 30:
        parser.error("duration 必须为正数；rate 须在 0～30 之间")
    call(args.url, "/health")
    # 库存初始化直接在集群内访问 catalog；运行脚本无需暴露第二个外部端口。
    # Catalog 初始库存由部署脚本执行 kubectl 命令创建。
    choices = ["orders", "orders", "orders", "cpu", "io", "memory", "stats"]
    deadline = time.monotonic() + args.duration
    index = 0
    futures = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        while time.monotonic() < deadline:
            operation = random.choice(choices) if args.mode == "mixed" else args.mode
            futures.append(pool.submit(exercise, args.url, operation, index))
            index += 1
            time.sleep(1 / args.rate)
        success = 0
        failed = 0
        for future in concurrent.futures.as_completed(futures):
            try:
                future.result()
                success += 1
            except (urllib.error.URLError, ValueError, TimeoutError) as error:
                failed += 1
                if failed <= 5:
                    print(f"请求失败: {error}")
    print(f"完成 {success + failed} 次请求：成功 {success}，失败 {failed}")


if __name__ == "__main__":
    main()
