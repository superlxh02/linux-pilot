"""可操作的订单/库存服务，用真实 CPU、SQLite、内存和跨节点 HTTP 产生负载。

这个服务只用于 virtual_env；API 返回实际事务结果，不向 Linux-Pilot 注入假指标。
每个请求都会由内核计入所在容器的 cgroup，订单请求还会调用另一个节点的库存服务。
"""

import hashlib
import json
import os
import sqlite3
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse


ROLE = os.environ.get("SERVICE_ROLE", "orders")
PEER_URL = os.environ.get("PEER_URL", "http://catalog:8080")
DATA_DIR = Path("/data")
DATA_DIR.mkdir(exist_ok=True)
DB_PATH = DATA_DIR / f"{ROLE}.sqlite3"
memory_blocks = []
memory_lock = threading.Lock()


def database():
    """一个请求一个 SQLite 连接，避免不同 HTTP 线程共享连接状态。"""
    connection = sqlite3.connect(DB_PATH, timeout=5)
    connection.execute("PRAGMA journal_mode=WAL")
    connection.execute(
        "CREATE TABLE IF NOT EXISTS items (sku TEXT PRIMARY KEY, quantity INTEGER NOT NULL)"
    )
    connection.execute(
        "CREATE TABLE IF NOT EXISTS orders (id INTEGER PRIMARY KEY AUTOINCREMENT, "
        "sku TEXT NOT NULL, quantity INTEGER NOT NULL, created_at REAL NOT NULL)"
    )
    return connection


class Handler(BaseHTTPRequestHandler):
    def send_json(self, status, payload):
        body = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def read_json(self):
        length = int(self.headers.get("Content-Length", "0"))
        if length > 16_384:
            raise ValueError("请求体过大")
        return json.loads(self.rfile.read(length) or b"{}")

    def do_GET(self):
        path = urlparse(self.path)
        if path.path == "/health":
            self.send_json(200, {"status": "ok", "role": ROLE, "hostname": os.uname().nodename})
        elif path.path == "/stats":
            with database() as db:
                items = db.execute("SELECT COUNT(*) FROM items").fetchone()[0]
                orders = db.execute("SELECT COUNT(*) FROM orders").fetchone()[0]
            with memory_lock:
                held = sum(len(block) for block in memory_blocks)
            self.send_json(200, {"role": ROLE, "items": items, "orders": orders, "held_bytes": held})
        elif path.path == "/items":
            with database() as db:
                rows = db.execute("SELECT sku, quantity FROM items ORDER BY sku LIMIT 100").fetchall()
            self.send_json(200, {"items": [{"sku": sku, "quantity": quantity} for sku, quantity in rows]})
        else:
            self.send_json(404, {"error": "未知路径"})

    def do_POST(self):
        path = urlparse(self.path)
        try:
            if path.path == "/items" and ROLE == "catalog":
                payload = self.read_json()
                sku = str(payload["sku"])[:64]
                quantity = int(payload.get("quantity", 1000))
                if not sku or not 0 < quantity <= 1_000_000:
                    raise ValueError("sku 或 quantity 无效")
                with database() as db:
                    db.execute(
                        "INSERT INTO items(sku,quantity) VALUES(?,?) "
                        "ON CONFLICT(sku) DO UPDATE SET quantity=excluded.quantity",
                        (sku, quantity),
                    )
                self.send_json(200, {"sku": sku, "quantity": quantity})
            elif path.path == "/reserve" and ROLE == "catalog":
                payload = self.read_json()
                sku = str(payload["sku"])[:64]
                quantity = int(payload.get("quantity", 1))
                if not 0 < quantity <= 100:
                    raise ValueError("quantity 无效")
                with database() as db:
                    changed = db.execute(
                        "UPDATE items SET quantity=quantity-? WHERE sku=? AND quantity>=?",
                        (quantity, sku, quantity),
                    ).rowcount
                self.send_json(200 if changed else 409, {"reserved": bool(changed), "sku": sku})
            elif path.path == "/orders" and ROLE == "orders":
                payload = self.read_json()
                sku = str(payload["sku"])[:64]
                quantity = int(payload.get("quantity", 1))
                request = urllib.request.Request(
                    f"{PEER_URL}/reserve",
                    json.dumps({"sku": sku, "quantity": quantity}).encode(),
                    {"Content-Type": "application/json"},
                    method="POST",
                )
                try:
                    with urllib.request.urlopen(request, timeout=3) as response:
                        response.read()
                except urllib.error.HTTPError as error:
                    self.send_json(409, {"error": "库存不足", "upstream_status": error.code})
                    return
                with database() as db:
                    order_id = db.execute(
                        "INSERT INTO orders(sku,quantity,created_at) VALUES(?,?,?)",
                        (sku, quantity, time.time()),
                    ).lastrowid
                self.send_json(201, {"order_id": order_id, "sku": sku, "quantity": quantity})
            elif path.path == "/work/cpu":
                milliseconds = min(max(int(parse_qs(path.query).get("ms", ["100"])[0]), 1), 2000)
                deadline = time.monotonic() + milliseconds / 1000
                digest = b"linux-pilot"
                iterations = 0
                while time.monotonic() < deadline:
                    digest = hashlib.sha256(digest).digest()
                    iterations += 1
                self.send_json(200, {"iterations": iterations, "digest": digest.hex()[:16]})
            elif path.path == "/work/io":
                kib = min(max(int(parse_qs(path.query).get("kib", ["64"])[0]), 1), 4096)
                filename = DATA_DIR / f"io-{threading.get_ident()}.bin"
                with filename.open("wb") as stream:
                    stream.write(os.urandom(kib * 1024))
                    stream.flush()
                    os.fsync(stream.fileno())
                self.send_json(200, {"written_bytes": kib * 1024})
            elif path.path == "/work/memory":
                mebibytes = min(max(int(parse_qs(path.query).get("mib", ["8"])[0]), 1), 32)
                block = bytearray(os.urandom(mebibytes * 1024 * 1024))
                with memory_lock:
                    memory_blocks.append(block)
                    while sum(len(item) for item in memory_blocks) > 64 * 1024 * 1024:
                        memory_blocks.pop(0)
                    held = sum(len(item) for item in memory_blocks)
                self.send_json(200, {"held_bytes": held})
            else:
                self.send_json(404, {"error": "未知路径或服务角色"})
        except (KeyError, ValueError, json.JSONDecodeError) as error:
            self.send_json(400, {"error": str(error)})

    def log_message(self, fmt, *args):
        print(f"{ROLE} {self.address_string()} {fmt % args}", flush=True)


if __name__ == "__main__":
    with database():
        pass
    ThreadingHTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
