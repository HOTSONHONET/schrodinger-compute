"""Experimental worker: fixed computation and session-scoped workspace probes only."""
import argparse
import hmac
import json
import os
from pathlib import Path
import re
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MAX_COUNT = 2_000_000
SESSION = re.compile(r"[0-9a-f]{32}\Z")


class WorkerServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address, node_id, workspace, token):
        if not node_id or len(token) < 32:
            raise ValueError("node ID and a token of at least 32 characters are required")
        self.node_id = node_id
        self.workspace = Path(workspace).resolve(strict=True)
        if not self.workspace.is_dir():
            raise ValueError("workspace must be an existing directory")
        self.token = token
        self.busy = threading.Lock()
        super().__init__(address, Handler)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass  # Do not log credentials or workspace payloads.

    def reply(self, status, value):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_POST(self):
        self.connection.settimeout(10)
        expected = "Bearer " + self.server.token
        if not hmac.compare_digest(self.headers.get("Authorization", ""), expected):
            self.reply(401, {"error": "unauthorized"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 4096:
                raise ValueError("request body must be 1..4096 bytes")
            body = json.loads(self.rfile.read(length))
            if not isinstance(body, dict):
                raise ValueError("request body must be an object")
            session = body.get("session", "")
            if not isinstance(session, str) or not SESSION.fullmatch(session):
                raise ValueError("invalid session ID")
            if self.path == "/v1/status":
                self.reply(200, {"node_id": self.server.node_id, "pid": os.getpid()})
            elif self.path in ("/v1/probe/write", "/v1/probe/read", "/v1/probe/delete"):
                self.probe(session, body)
            elif self.path == "/v1/reduce":
                start, end = body.get("start"), body.get("end")
                if type(start) is not int or type(end) is not int or not 0 <= start < end <= MAX_COUNT:
                    raise ValueError("expected integer range 0 <= start < end <= 2000000")
                if not self.server.busy.acquire(blocking=False):
                    self.reply(409, {"error": "worker busy"})
                    return
                try:
                    # Actual CPU work in each worker; no closed-form shortcut.
                    total = sum(i * i for i in range(start, end))
                    self.reply(200, {"node_id": self.server.node_id, "session": session,
                                     "start": start, "end": end, "sum": total})
                finally:
                    self.server.busy.release()
            else:
                self.reply(404, {"error": "unknown operation"})
        except (ValueError, TypeError, json.JSONDecodeError) as error:
            self.reply(400, {"error": str(error)})
        except (OSError, TimeoutError):
            self.reply(503, {"error": "workspace or transport unavailable"})

    def probe(self, session, body):
        # Only this fixed session-scoped file can be accessed. Never expose a
        # general host filesystem API. Workspace owners must still be trusted.
        path = self.server.workspace / (".schrodinger-probe-" + session)
        if self.path.endswith("/write"):
            marker = body.get("marker")
            if not isinstance(marker, str) or not SESSION.fullmatch(marker):
                raise ValueError("invalid probe marker")
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            with os.fdopen(fd, "w") as stream:
                stream.write(marker)
                stream.flush()
                os.fsync(stream.fileno())
            self.reply(200, {"written": True})
        elif self.path.endswith("/read"):
            fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
            with os.fdopen(fd) as stream:
                marker = stream.read(65)
            self.reply(200, {"marker": marker})
        else:
            path.unlink(missing_ok=True)
            self.reply(200, {"deleted": True})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--listen", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=7351)
    args = parser.parse_args()
    server = WorkerServer((args.listen, args.port), os.environ["SC_NODE_ID"],
                          os.environ.get("SC_WORKSPACE", "/workspace"), os.environ["SC_TOKEN"])
    print(json.dumps({"node_id": server.node_id, "port": server.server_port}), flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
