import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

from protocol import CHUNK, request
from session import Session
from worker import Worker

TOKEN = "test-" + "a" * 32


class UnifiedTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.workers = {}
        self.urls = [self.launch(node) for node in ("beast", "legion")]
        self.session = Session(self.root / "catalog.json", TOKEN)
        self.session.start(self.urls)

    def launch(self, node, port=0):
        worker = Worker(("127.0.0.1", port), node, self.root / node, TOKEN)
        thread = threading.Thread(target=worker.serve_forever, kwargs={"poll_interval": 0.01})
        thread.start()
        self.workers[node] = worker

        def stop():
            if thread.is_alive():
                worker.shutdown()
                thread.join()
                worker.server_close()

        self.addCleanup(stop)
        return f"http://127.0.0.1:{worker.server_port}"

    def stop(self, node):
        worker = self.workers[node]
        worker.shutdown()
        worker.server_close()

    def source(self, data):
        path = self.root / "input"
        path.write_bytes(data)
        return path

    def test_dfs_shards_on_two_disks_and_round_trips(self):
        data = b"a" * CHUNK + b"b" * 123
        receipt = self.session.put_file("inputs/dataset", self.source(data))
        self.assertEqual(receipt["bytes_by_node"], {"beast": CHUNK, "legion": 123})
        for shard in self.session.entry("disk", "inputs/dataset")["shards"]:
            path = self.root / shard["node"] / self.session.state["session"] / shard["id"]
            self.assertEqual(path.stat().st_size, shard["bytes"])
        out = self.root / "out"
        self.session.read("disk", "inputs/dataset", out)
        self.assertEqual(out.read_bytes(), data)
        result = self.session.compute_sum("disk", "inputs/dataset")
        self.assertEqual(result["sum"], sum(data))
        self.assertEqual({p["node"] for p in result["parts"]}, {"beast", "legion"})

    def test_ram_write_and_read_cross_boundary(self):
        receipt = self.session.allocate("buffer", CHUNK + 4)
        self.assertEqual(receipt["bytes_by_node"], {"beast": CHUNK, "legion": 4})
        self.session.write_ram("buffer", CHUNK - 2, self.source(b"ABCD"))
        out = self.root / "read"
        self.session.read("ram", "buffer", out, CHUNK - 3, 7)
        self.assertEqual(out.read_bytes(), b"\x00ABCD\x00\x00")
        self.assertEqual(self.session.compute_sum("ram", "buffer")["sum"], sum(b"ABCD"))
        for node, worker in self.workers.items():
            self.assertEqual(len(worker.ram), 1)
            self.assertEqual(list((self.root / node).iterdir()), [])
        with self.assertRaisesRegex(ValueError, "outside"):
            self.session.write_ram("buffer", CHUNK + 1, self.source(b"ABCD"))
        with self.assertRaisesRegex(ValueError, "outside"):
            self.session.read("ram", "buffer", self.root / "invalid", 0, CHUNK + 5)
        self.assertFalse((self.root / "invalid").exists())

    def test_failed_ram_write_keeps_old_catalog_and_data(self):
        self.session.allocate("buffer", CHUNK + 2)
        old_state = self.session.path.read_bytes()
        call = self.session.call

        def fail_legion(node, method, suffix="", body=None):
            if node == "legion" and method == "PUT":
                raise RuntimeError("simulated failed write")
            return call(node, method, suffix, body)

        with patch.object(self.session, "call", side_effect=fail_legion):
            with self.assertRaisesRegex(RuntimeError, "failed write"):
                self.session.write_ram("buffer", CHUNK - 1, self.source(b"AB"))
        self.assertEqual(self.session.path.read_bytes(), old_state)
        self.assertEqual(self.session.compute_sum("ram", "buffer")["sum"], 0)
        self.assertEqual([len(w.ram) for w in self.workers.values()], [1, 1])

    def test_failed_file_upload_is_not_published(self):
        call = self.session.call

        def fail_legion(node, method, suffix="", body=None):
            if node == "legion" and method == "PUT":
                raise RuntimeError("simulated failed write")
            return call(node, method, suffix, body)

        with patch.object(self.session, "call", side_effect=fail_legion):
            with self.assertRaises(RuntimeError):
                self.session.put_file("blob", self.source(b"x" * (CHUNK + 1)))
        self.session.load()
        self.assertEqual(self.session.state["disk"], {})
        self.assertEqual(list((self.root / "beast" / self.session.state["session"]).iterdir()), [])

    def test_worker_restart_preserves_disk_but_not_ram(self):
        self.session.put_file("blob", self.source(b"a" * (CHUNK + 1)))
        self.session.allocate("buffer", CHUNK + 1)
        port = self.workers["legion"].server_port
        self.stop("legion")
        self.launch("legion", port)
        self.assertEqual(self.session.compute_sum("disk", "blob")["sum"], 97 * (CHUNK + 1))
        with self.assertRaisesRegex(RuntimeError, "shard missing"):
            self.session.compute_sum("ram", "buffer")
        out = self.root / "failed-read"
        with self.assertRaisesRegex(RuntimeError, "shard missing"):
            self.session.read("ram", "buffer", out)
        self.assertFalse(out.exists())

    def test_close_retry_and_cleanup(self):
        self.session.put_file("blob", self.source(b"a" * (CHUNK + 1)))
        self.session.allocate("buffer", CHUNK + 1)
        port = self.workers["legion"].server_port
        self.stop("legion")
        with self.assertRaisesRegex(RuntimeError, "retry close"):
            self.session.close()
        self.session.load()
        self.assertTrue(self.session.state["closing"])
        self.launch("legion", port)
        self.assertTrue(self.session.close()["closed"])
        self.assertFalse(self.session.path.exists())
        for worker in self.workers.values():
            self.assertEqual(worker.ram, {})
            self.assertEqual(list(worker.root.iterdir()), [])

    def test_auth_identity_and_traversal(self):
        url = self.urls[0]
        with self.assertRaisesRegex(RuntimeError, "HTTP 401"):
            request(url, "wrong", "GET", "/v1/node")
        with self.assertRaisesRegex(RuntimeError, "HTTP 409"):
            request(url, TOKEN, "DELETE", "/v1/sessions/" + "a" * 32, node="legion")
        with self.assertRaisesRegex(RuntimeError, "HTTP 404"):
            request(url, TOKEN, "GET", "/v1/sessions/../disk/" + "a" * 32, node="beast")
        with self.assertRaises(ValueError):
            self.session.put_file("../escape", self.source(b"data"))

    def test_output_is_never_overwritten_and_namespaces_are_separate(self):
        file = self.source(b"data")
        self.session.put_file("same-name", file)
        self.session.allocate("same-name", 4)
        with self.assertRaises(FileExistsError):
            self.session.read("disk", "same-name", file)
        self.assertEqual(file.read_bytes(), b"data")
        self.session.remove("disk", "same-name")
        self.assertEqual(self.session.compute_sum("ram", "same-name")["sum"], 0)

    def test_cli_creates_persistent_session_and_runs_compute(self):
        tool = Path(__file__).with_name("session.py")
        command = [sys.executable, "-B", str(tool), "--state", str(self.root / "cli.json")]

        def run(*args):
            return subprocess.run([*command, *args], env={**os.environ, "SC_TOKEN": TOKEN},
                                  capture_output=True, text=True, check=True, timeout=10).stdout

        run("start", *self.urls)
        run("put", "inputs/blob", str(self.source(b"\x01" * (CHUNK + 2))))
        result = run("sum", "disk", "inputs/blob")
        self.assertIn(f'"sum": {CHUNK + 2}', result)
        run("close")


if __name__ == "__main__":
    unittest.main()
