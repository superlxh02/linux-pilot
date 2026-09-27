"""向三台独立单体服务并发发送真实请求，支持按服务与负载类型选择。"""

import argparse
import concurrent.futures
import json
import random
import time
import urllib.error
import urllib.request

PORTS = {"billing": 18081, "search": 18082, "reports": 18083}


def call(service, operation):
    base = f"http://127.0.0.1:{PORTS[service]}"
    paths = {
        "transaction": "/work/transaction",
        "cpu": "/work/cpu?ms=250",
        "io": "/work/io?kib=256",
        "memory": "/work/memory?mib=4",
    }
    request = urllib.request.Request(base + paths[operation], b"{}", {"Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(request, timeout=8) as response:
        return json.load(response)


def main():
    parser = argparse.ArgumentParser(description="Linux-Pilot 三台单体主机负载模拟")
    parser.add_argument("--duration", type=int, default=60)
    parser.add_argument("--rate", type=float, default=6)
    parser.add_argument("--service", choices=["all", *PORTS], default="all")
    parser.add_argument("--mode", choices=["mixed", "transaction", "cpu", "io", "memory"], default="mixed")
    args = parser.parse_args()
    if args.duration < 1 or not 0 < args.rate <= 30:
        parser.error("duration 必须为正数；rate 须在 0～30 之间")
    services = list(PORTS) if args.service == "all" else [args.service]
    for name in services:
        with urllib.request.urlopen(f"http://127.0.0.1:{PORTS[name]}/health", timeout=5):
            pass
    end = time.monotonic() + args.duration
    futures = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=12) as pool:
        while time.monotonic() < end:
            target = random.choice(services)
            operation = random.choice(["transaction", "transaction", "cpu", "io", "memory"]) if args.mode == "mixed" else args.mode
            futures.append(pool.submit(call, target, operation))
            time.sleep(1 / args.rate)
        success = 0
        failures = 0
        for future in concurrent.futures.as_completed(futures):
            try:
                future.result()
                success += 1
            except (OSError, ValueError, urllib.error.URLError) as error:
                failures += 1
                if failures <= 5:
                    print(f"请求失败：{error}")
    print(f"三台单体节点完成 {success + failures} 次请求：成功 {success}，失败 {failures}")


if __name__ == "__main__":
    main()
