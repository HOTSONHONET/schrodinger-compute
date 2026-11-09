"""Store disk and RAM shards, and compute on their owners."""
import argparse
import hashlib
import hmac
import json
import os
from pathlib import Path
import shutil
import tempfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Lock

from protocol import CHUNK, ID, NODE, check_token


class Worker(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address, node, data_dir, token):
        check_token(token)
        if not NODE.fullmatch(node):
            raise ValueError("invalid SC_NODE_ID")
        self.node = node
        self.root = Path(data_dir).resolve()
        self.root.mkdir(parents=True, exist_ok=True)
        self.token = token.encode()
        self.ram = {}
        self.guard = Lock()
        super().__init__(address, Handler)

    def get(self, tier, session, shard):
        if tier == "ram":
            return self.ram[(session, shard)]
        return (self.root / session / shard).read_bytes()

    def put(self, tier, session, shard, data):
        if tier == "ram":
            key = (session, shard)
            if key in self.ram:
                raise FileExistsError()
            self.ram[key] = data
            return
        folder = self.root / session
        folder.mkdir(exist_ok=True)
        # Publish only complete bytes; link() also refuses to overwrite a shard.
        with tempfile.NamedTemporaryFile(dir=folder) as temp:
            temp.write(data)
            temp.flush()
            os.fsync(temp.fileno())
            os.link(temp.name, folder / shard)

    def delete(self, tier, session, shard):
        if tier == "ram":
            self.ram.pop((session, shard), None)
        else:
            (self.root / session / shard).unlink(missing_ok=True)

    def close_session(self, session):
        directory = self.root / session
        if directory.exists():
            shutil.rmtree(directory)
        for key in list(self.ram):
            if key[0] == session:
                del self.ram[key]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def send(self, code, body):
        if isinstance(body, dict):
            body = json.dumps(body).encode()
            kind = "application/json"
        else:
            kind = "application/octet-stream"
        self.send_response(code)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def handle_operation(self):
        self.connection.settimeout(15)
        server = self.server
        auth = self.headers.get("Authorization", "").encode()
        if not hmac.compare_digest(auth, b"Bearer " + server.token):
            return 401, {"error": "unauthorized"}
        if self.path == "/v1/node" and self.command == "GET":
            return 200, {"node": server.node}
        if self.headers.get("X-SC-Node") != server.node:
            return 409, {"error": "worker identity does not match session"}
        parts = self.path.split("/")
        if len(parts) < 4 or parts[:3] != ["", "v1", "sessions"] or not ID.fullmatch(parts[3]):
            return 404, {"error": "unknown operation"}
        session = parts[3]
        if len(parts) == 4 and self.command == "DELETE":
            with server.guard:
                server.close_session(session)
            return 204, b""
        if len(parts) not in (6, 7):
            return 404, {"error": "unknown operation"}
        tier, shard = parts[4:6]
        if tier not in ("disk", "ram") or not ID.fullmatch(shard):
            return 404, {"error": "unknown operation"}
        if len(parts) == 7:
            if parts[6] != "sum" or self.command != "POST":
                return 404, {"error": "unknown operation"}
            with server.guard:
                data = server.get(tier, session, shard)
            return 200, {"node": server.node, "bytes": len(data), "sum": sum(data),
                         "sha256": hashlib.sha256(data).hexdigest()}
        if self.command == "PUT":
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= CHUNK:
                raise ValueError("invalid shard length")
            data = self.rfile.read(length)
            if len(data) != length:
                raise ValueError("incomplete shard")
            with server.guard:
                server.put(tier, session, shard, data)
            return 201, {"sha256": hashlib.sha256(data).hexdigest()}
        if self.command == "GET":
            with server.guard:
                return 200, server.get(tier, session, shard)
        if self.command == "DELETE":
            with server.guard:
                server.delete(tier, session, shard)
            return 204, b""
        return 405, {"error": "unsupported method"}

    def dispatch(self):
        try:
            code, body = self.handle_operation()
        except (ValueError, TimeoutError):
            code, body = 400, {"error": "invalid or incomplete request"}
        except (KeyError, FileNotFoundError):
            code, body = 404, {"error": "shard missing (RAM is lost on worker restart)"}
        except FileExistsError:
            code, body = 409, {"error": "shard already exists"}
        except (OSError, MemoryError):
            code, body = 503, {"error": "worker storage unavailable"}
        try:
            self.send(code, body)
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            pass

    do_GET = do_PUT = do_POST = do_DELETE = dispatch


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--listen", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=7353)
    args = parser.parse_args()
    with Worker((args.listen, args.port), os.environ["SC_NODE_ID"],
                os.environ.get("SC_DATA_DIR", "/data"), os.environ["SC_TOKEN"]) as server:
        print(f"{server.node} listening on {args.listen}:{server.server_port}", flush=True)
        server.serve_forever()


if __name__ == "__main__":
    main()
