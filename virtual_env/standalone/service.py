"""三台模拟主机各自的真实单体服务；负载通过 HTTP 产生而非注入指标。"""

import ctypes
import hashlib
import json
import os
import sqlite3
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse


ROLE = os.environ.get("PILOT_SERVICE_ROLE", "billing")
DATA = Path("/data")
DATA.mkdir(exist_ok=True)
DATABASE = DATA / "service.sqlite3"
HELD = []
LOCK = threading.Lock()

# Linux 的 comm 最长 15 字节；给每个单体服务稳定名称，便于进程清单识别。
ctypes.CDLL(None).prctl(15, f"pilot-{ROLE}".encode()[:15], 0, 0, 0)


def connect():
    db = sqlite3.connect(DATABASE, timeout=5)
    db.execute("CREATE TABLE IF NOT EXISTS operations(id INTEGER PRIMARY KEY, kind TEXT, created REAL)")
    return db


class Handler(BaseHTTPRequestHandler):
    def send_json(self, status, payload):
        body = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = urlparse(self.path)
        if path.path == "/health":
            self.send_json(200, {"status": "ok", "service": ROLE, "node": os.environ.get("PO_AGENT__HOST_ID")})
        elif path.path == "/stats":
            with connect() as db:
                count = db.execute("SELECT COUNT(*) FROM operations").fetchone()[0]
            with LOCK:
                held = sum(map(len, HELD))
            self.send_json(200, {"service": ROLE, "operations": count, "held_bytes": held})
        else:
            self.send_json(404, {"error": "未知路径"})

    def do_POST(self):
        path = urlparse(self.path)
        query = parse_qs(path.query)
        try:
            if path.path == "/work/cpu":
                duration = min(max(int(query.get("ms", ["150"])[0]), 1), 1500)
                deadline = time.monotonic() + duration / 1000
                digest = b"linux-pilot"
                count = 0
                while time.monotonic() < deadline:
                    digest = hashlib.sha256(digest).digest()
                    count += 1
                result = {"iterations": count, "digest": digest.hex()[:16]}
            elif path.path == "/work/io":
                kib = min(max(int(query.get("kib", ["128"])[0]), 1), 2048)
                target = DATA / f"io-{threading.get_ident()}.bin"
                with target.open("wb") as stream:
                    stream.write(os.urandom(kib * 1024))
                    stream.flush()
                    os.fsync(stream.fileno())
                result = {"written_bytes": kib * 1024}
            elif path.path == "/work/memory":
                mib = min(max(int(query.get("mib", ["4"])[0]), 1), 16)
                with LOCK:
                    HELD.append(bytearray(os.urandom(mib * 1024 * 1024)))
                    while sum(map(len, HELD)) > 48 * 1024 * 1024:
                        HELD.pop(0)
                    result = {"held_bytes": sum(map(len, HELD))}
            elif path.path == "/work/transaction":
                with connect() as db:
                    key = db.execute("INSERT INTO operations(kind,created) VALUES(?,?) RETURNING id", (ROLE, time.time())).fetchone()[0]
                result = {"transaction_id": key, "service": ROLE}
            else:
                self.send_json(404, {"error": "未知路径"})
                return
            self.send_json(200, result)
        except (OSError, ValueError) as error:
            self.send_json(400, {"error": str(error)})

    def log_message(self, fmt, *args):
        print(f"{ROLE} {self.address_string()} {fmt % args}", flush=True)


with connect():
    pass
ThreadingHTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
