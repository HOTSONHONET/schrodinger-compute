"""One session catalog for distributed files, byte-addressed RAM and compute."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import sys
import tempfile
import uuid

from protocol import CHUNK, NODE, check_token, document, origin, request


def logical_name(value):
    path = PurePosixPath(value)
    if path.is_absolute() or str(path) != value or value == "." or ".." in path.parts:
        raise ValueError("use a relative logical path without . or .. components")
    return value


@contextmanager
def staged_output(path, *, replace=False):
    path = Path(path)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            yield stream
            stream.flush()
            os.fsync(stream.fileno())
            stream.close()
            if replace:
                os.replace(temporary, path)
            else:
                os.link(temporary, path)  # Existing output files are preserved.
        finally:
            temporary.unlink(missing_ok=True)


class Session:
    def __init__(self, state, token):
        self.path = Path(state)
        check_token(token)
        self.token = token

    @contextmanager
    def locked(self):
        with self.path.with_name(self.path.name + ".lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            yield

    def start(self, workers):
        if self.path.exists():
            raise ValueError("session catalog already exists; use a different --state path")
        urls = [origin(url) for url in workers]
        if len(urls) < 2 or len(set(urls)) != len(urls):
            raise ValueError("provide at least two distinct worker addresses")
        members = {}
        for url in urls:
            node = document(request(url, self.token, "GET", "/v1/node")).get("node")
            if not isinstance(node, str) or not NODE.fullmatch(node) or node in members:
                raise ValueError("workers must advertise distinct valid node IDs")
            members[node] = url
        self.state = {"session": uuid.uuid4().hex, "workers": members,
                      "disk": {}, "ram": {}, "closing": False}
        self.save()
        return {"session": self.state["session"], "workers": list(members)}

    def load(self):
        self.state = document(self.path.read_bytes())

    def save(self):
        with staged_output(self.path, replace=True) as stream:
            stream.write(json.dumps(self.state, indent=2).encode() + b"\n")

    def call(self, node, method, suffix="", body=None):
        path = "/v1/sessions/" + self.state["session"] + suffix
        return request(self.state["workers"][node], self.token, method, path,
                       body=body, node=node)

    def upload(self, tier, node, data, staged):
        shard = {"id": uuid.uuid4().hex, "node": node, "bytes": len(data),
                 "sha256": hashlib.sha256(data).hexdigest()}
        staged.append(shard)  # Also clean up when the worker wrote but reply was lost.
        result = document(self.call(node, "PUT", self.suffix(tier, shard), data))
        if result.get("sha256") != shard["sha256"]:
            raise RuntimeError("worker acknowledged a different shard digest")
        return shard

    @staticmethod
    def suffix(tier, shard):
        return f"/{tier}/{shard['id']}"

    def download(self, tier, shard):
        data = self.call(shard["node"], "GET", self.suffix(tier, shard))
        if len(data) != shard["bytes"] or hashlib.sha256(data).hexdigest() != shard["sha256"]:
            raise RuntimeError(f"corrupt shard on {shard['node']}")
        return data

    def cleanup(self, tier, shards):
        for shard in shards:
            try:
                self.call(shard["node"], "DELETE", self.suffix(tier, shard))
            except (OSError, RuntimeError) as error:
                print(f"cleanup deferred to session close: {error}", file=sys.stderr)

    def publish(self, tier, name, size, shards):
        self.state[tier][name] = {"size": size, "shards": shards}
        self.save()

    def create(self, tier, name, blocks):
        name = logical_name(name)
        if name in self.state[tier]:
            raise ValueError(f"{name} already exists")
        workers = list(self.state["workers"])
        staged = []
        try:
            for index, data in enumerate(blocks):
                self.upload(tier, workers[index % len(workers)], data, staged)
            size = sum(part["bytes"] for part in staged)
            self.publish(tier, name, size, staged)
        except Exception:
            self.cleanup(tier, staged)
            raise
        return self.describe(tier, name)

    def put_file(self, name, source):
        with Path(source).open("rb") as stream:
            return self.create("disk", name, iter(lambda: stream.read(CHUNK), b""))

    def allocate(self, name, size):
        if size <= 0:
            raise ValueError("RAM allocation must be positive")
        return self.create("ram", name,
                           (bytes(min(CHUNK, size - start)) for start in range(0, size, CHUNK)))

    def entry(self, tier, name):
        return self.state[tier][logical_name(name)]

    @staticmethod
    def span(entry, offset, length):
        if offset < 0 or length < 0 or offset + length > entry["size"]:
            raise ValueError("range is outside the allocation")
        end = offset + length
        if not length:
            return
        start = offset // CHUNK
        for i in range(start, (end - 1) // CHUNK + 1):
            shard = entry["shards"][i]
            base = i * CHUNK
            yield i, shard, max(0, offset - base), min(shard["bytes"], end - base)

    def read(self, tier, name, output, offset=0, length=None):
        entry = self.entry(tier, name)
        length = entry["size"] if length is None else length
        with staged_output(output) as stream:
            for _, shard, lo, hi in self.span(entry, offset, length):
                stream.write(self.download(tier, shard)[lo:hi])
        return {"bytes": length, "output": str(output)}

    def write_ram(self, name, offset, source):
        entry = self.entry("ram", name)
        shards = entry["shards"].copy()
        staged, replaced = [], []
        try:
            with Path(source).open("rb") as stream:
                size = os.fstat(stream.fileno()).st_size
                for i, old, lo, hi in self.span(entry, offset, size):
                    block = bytearray(self.download("ram", old))
                    data = stream.read(hi - lo)
                    if len(data) != hi - lo:
                        raise ValueError("source size changed during write")
                    block[lo:hi] = data
                    shards[i] = self.upload("ram", old["node"], bytes(block), staged)
                    replaced.append(old)
                if stream.read(1):
                    raise ValueError("source size changed during write")
            self.publish("ram", name, entry["size"], shards)
        except Exception:
            self.cleanup("ram", staged)
            raise
        self.cleanup("ram", replaced)
        return self.describe("ram", name)

    def describe(self, tier, name):
        entry = self.entry(tier, name)
        by_node = {}
        for shard in entry["shards"]:
            node = shard["node"]
            by_node[node] = by_node.get(node, 0) + shard["bytes"]
        return {"name": name, "tier": tier, "bytes": entry["size"], "bytes_by_node": by_node}

    def remove(self, tier, name):
        entry = self.entry(tier, name)
        del self.state[tier][name]
        self.save()
        self.cleanup(tier, entry["shards"])
        return {"removed": name}

    def compute_sum(self, tier, name):
        entry = self.entry(tier, name)
        groups = {}
        for shard in entry["shards"]:
            groups.setdefault(shard["node"], []).append(shard)

        def reduce_node(group):
            node, shards = group
            total = count = 0
            for shard in shards:
                result = document(self.call(node, "POST", self.suffix(tier, shard) + "/sum"))
                partial = result.get("sum")
                if (result.get("node") != node or result.get("bytes") != shard["bytes"]
                        or result.get("sha256") != shard["sha256"]
                        or type(partial) is not int or not 0 <= partial <= 255 * shard["bytes"]):
                    raise RuntimeError(f"invalid partial from {node}")
                total += partial
                count += shard["bytes"]
            return {"node": node, "bytes": count, "sum": total}

        with ThreadPoolExecutor(max_workers=min(8, max(1, len(groups)))) as pool:
            parts = list(pool.map(reduce_node, groups.items()))
        return {"bytes": entry["size"], "sum": sum(p["sum"] for p in parts), "parts": parts}

    def close(self):
        self.state["closing"] = True
        self.save()
        errors = []
        for node in self.state["workers"]:
            try:
                self.call(node, "DELETE")
            except (OSError, RuntimeError) as error:
                errors.append(str(error))
        if errors:
            raise RuntimeError("close incomplete; retry close: " + "; ".join(errors))
        self.path.unlink()
        return {"closed": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state", default=".sc-session.json")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("start").add_argument("workers", nargs="+")
    commands.add_parser("list")
    commands.add_parser("close")
    for command, fields in (
        ("put", [("name", str), ("source", Path)]),
        ("get", [("name", str), ("output", Path)]),
        ("alloc", [("name", str), ("size", int)]),
        ("write", [("name", str), ("offset", int), ("source", Path)]),
        ("read", [("name", str), ("offset", int), ("length", int), ("output", Path)]),
        ("sum", [("tier", str), ("name", str)]),
        ("rm", [("tier", str), ("name", str)]),
    ):
        sub = commands.add_parser(command)
        for field, kind in fields:
            if field == "tier":
                sub.add_argument(field, choices=("disk", "ram"))
            else:
                sub.add_argument(field, type=kind)
    args = parser.parse_args()
    try:
        session = Session(args.state, os.environ.get("SC_TOKEN", ""))
        with session.locked():
            if args.command != "start":
                session.load()
                if session.state["closing"] and args.command != "close":
                    raise ValueError("session is closing; retry close")
            match args.command:
                case "start":
                    result = session.start(args.workers)
                case "list":
                    result = [session.describe(tier, name) for tier in ("disk", "ram")
                              for name in session.state[tier]]
                case "put":
                    result = session.put_file(args.name, args.source)
                case "get":
                    result = session.read("disk", args.name, args.output)
                case "alloc":
                    result = session.allocate(args.name, args.size)
                case "write":
                    result = session.write_ram(args.name, args.offset, args.source)
                case "read":
                    result = session.read("ram", args.name, args.output, args.offset, args.length)
                case "sum":
                    result = session.compute_sum(args.tier, args.name)
                case "rm":
                    result = session.remove(args.tier, args.name)
                case "close":
                    result = session.close()
    except (OSError, RuntimeError, ValueError, KeyError) as error:
        parser.exit(1, f"unified session: {error}\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
