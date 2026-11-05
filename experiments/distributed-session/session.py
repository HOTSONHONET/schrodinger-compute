"""Coordinate one verified reduction across exactly two experimental workers."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
import time
from urllib.parse import urlsplit
from urllib.request import Request, build_opener, HTTPHandler, HTTPSHandler, ProxyHandler, HTTPRedirectHandler
import uuid

from worker import MAX_COUNT


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None  # Never forward a worker credential to a redirect target.


def call(url, operation, body, token, timeout):
    parsed = urlsplit(url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username or parsed.query or parsed.fragment or parsed.path not in ("", "/"):
        raise ValueError("worker URL must be an HTTP(S) origin without credentials")
    request = Request(url.rstrip("/") + operation, json.dumps(body).encode(),
                      {"Authorization": "Bearer " + token, "Content-Type": "application/json"})
    # Explicit endpoints only: do not leak lab credentials via environment proxies.
    opener = build_opener(ProxyHandler({}), HTTPHandler(), HTTPSHandler(), NoRedirect())
    with opener.open(request, timeout=timeout) as response:
        raw = response.read(16_385)
    if len(raw) > 16_384:
        raise ValueError("worker response too large")
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise ValueError("invalid worker response")
    return value


def run(workers, count, token, timeout=10, rpc=call):
    if len(workers) != 2 or len(set(workers)) != 2:
        raise ValueError("exactly two different worker URLs are required")
    if type(count) is not int or not 2 <= count <= MAX_COUNT:
        raise ValueError("count must be an integer between 2 and 2000000")
    if len(token) < 32 or timeout <= 0:
        raise ValueError("a token of at least 32 characters and positive timeout are required")
    session = uuid.uuid4().hex
    marker = uuid.uuid4().hex
    base = {"session": session}
    identities = [rpc(url, "/v1/status", base, token, timeout)["node_id"] for url in workers]
    if any(not isinstance(node, str) or not node for node in identities) or len(set(identities)) != 2:
        raise ValueError("workers must advertise different nonempty node IDs")
    ranges = [(0, count // 2), (count // 2, count)]
    started = time.monotonic()
    try:
        # Bidirectional visibility check rejects two unrelated local directories.
        # This is a preflight check, not a filesystem consistency certification.
        for writer, reader in (workers, workers[::-1]):
            rpc(writer, "/v1/probe/write", {**base, "marker": marker}, token, timeout)
            observed = rpc(reader, "/v1/probe/read", base, token, timeout)
            if observed.get("marker") != marker:
                raise ValueError("workspace probe mismatch")
            rpc(writer, "/v1/probe/delete", base, token, timeout)
        with ThreadPoolExecutor(max_workers=2) as pool:
            futures = [pool.submit(rpc, url, "/v1/reduce", {**base, "start": start, "end": end}, token, timeout)
                       for url, (start, end) in zip(workers, ranges)]
            parts = [future.result() for future in futures]
        for identity, (start, end), part in zip(identities, ranges, parts):
            expected = prefix(end) - prefix(start)
            if (part.get("node_id") != identity or part.get("session") != session
                    or part.get("start") != start or part.get("end") != end
                    or type(part.get("sum")) is not int or part["sum"] != expected):
                raise ValueError("worker result failed identity/range/correctness verification")
        total = sum(part["sum"] for part in parts)
        if total != prefix(count):
            raise ValueError("aggregate verification failed")
        return {"session": session, "workspace_verified": True, "count": count,
                "sum": total, "verified": True, "parts": parts,
                "elapsed_seconds": round(time.monotonic() - started, 6)}
    finally:
        # Attempt both cleanups even if a write succeeded but its reply was lost.
        # Cleanup failure never replaces the original computation failure.
        for url in workers:
            try:
                rpc(url, "/v1/probe/delete", base, token, timeout)
            except Exception:
                pass


def prefix(n):
    return (n - 1) * n * (2 * n - 1) // 6


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workers", nargs=2, required=True)
    parser.add_argument("--count", type=int, default=1_000_000)
    parser.add_argument("--timeout", type=float, default=10)
    args = parser.parse_args()
    try:
        result = run(args.workers, args.count, os.environ["SC_TOKEN"], args.timeout)
    except Exception as error:
        parser.exit(1, f"session failed (no successful aggregate): {error}\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
