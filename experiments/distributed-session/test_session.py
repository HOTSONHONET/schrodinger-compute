import json
from pathlib import Path
import tempfile
import threading
import time
import unittest
from urllib.error import HTTPError

from session import call, prefix, run
from worker import WorkerServer

TOKEN = "test-token-" + "a" * 32


class SessionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.servers = []
        self.urls = []
        self.addCleanup(self.cleanup)
        self.start("beast", self.temp.name)
        self.start("legion", self.temp.name)

    def start(self, name, path):
        server = WorkerServer(("127.0.0.1", 0), name, path, TOKEN)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.servers.append((server, thread))
        self.urls.append(f"http://127.0.0.1:{server.server_port}")

    def cleanup(self):
        for server, thread in self.servers:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
        self.temp.cleanup()

    def test_real_http_two_workers_odd_and_even_ranges(self):
        for count in (2, 101, 10000):
            result = run(self.urls, count, TOKEN)
            self.assertEqual(result["sum"], prefix(count))
            self.assertEqual({p["node_id"] for p in result["parts"]}, {"beast", "legion"})
            self.assertTrue(result["workspace_verified"])
            self.assertEqual(list(Path(self.temp.name).iterdir()), [])

    def test_rejects_unshared_workspaces(self):
        other = Path(self.temp.name) / "other"
        other.mkdir()
        self.servers[1][0].workspace = other
        with self.assertRaises(HTTPError):
            run(self.urls, 100, TOKEN)
        self.assertEqual(list(other.iterdir()), [])
        self.assertEqual(list(Path(self.temp.name).glob(".schrodinger*")), [])

    def test_authentication(self):
        with self.assertRaises(HTTPError) as caught:
            run(self.urls, 10, "x" * 32)
        self.assertEqual(caught.exception.code, 401)

    def test_bad_partition_and_path_traversal_rejected(self):
        for body in ({"session": "../etc/passwd"},
                     {"session": "a" * 32, "start": 0, "end": 2_000_001},
                     {"session": "a" * 32, "start": False, "end": 5}):
            with self.assertRaises(HTTPError) as caught:
                call(self.urls[0], "/v1/reduce", body, TOKEN, 1)
            self.assertEqual(caught.exception.code, 400)

    def test_symlink_probe_rejected(self):
        session = "a" * 32
        target = Path(self.temp.name) / "outside"
        target.write_text("private")
        probe = Path(self.temp.name) / (".schrodinger-probe-" + session)
        probe.symlink_to(target)
        with self.assertRaises(HTTPError):
            call(self.urls[0], "/v1/probe/read", {"session": session}, TOKEN, 1)
        self.assertEqual(target.read_text(), "private")

    def test_busy_worker_rejects_parallel_submission(self):
        server = self.servers[0][0]
        server.busy.acquire()
        try:
            with self.assertRaises(HTTPError) as caught:
                run(self.urls, 100, TOKEN)
            self.assertEqual(caught.exception.code, 409)
        finally:
            server.busy.release()

    def test_rejects_duplicate_node_identity(self):
        self.servers[1][0].node_id = "beast"
        with self.assertRaisesRegex(ValueError, "different nonempty"):
            run(self.urls, 100, TOKEN)

    def test_failure_and_corrupt_result_never_produce_success(self):
        for mode in ("failure", "corrupt", "identity"):
            def rpc(url, operation, body, token, timeout):
                if operation == "/v1/reduce" and url == self.urls[1]:
                    if mode == "failure":
                        raise TimeoutError("simulated lost worker")
                    result = call(url, operation, body, token, timeout)
                    if mode == "corrupt":
                        result["sum"] += 1
                    else:
                        result["node_id"] = "impostor"
                    return result
                return call(url, operation, body, token, timeout)
            with self.assertRaises((ValueError, TimeoutError)):
                run(self.urls, 100, TOKEN, rpc=rpc)
            self.assertEqual(list(Path(self.temp.name).iterdir()), [])

    def test_requests_are_dispatched_concurrently(self):
        barrier = threading.Barrier(2, timeout=2)
        def rpc(url, operation, body, token, timeout):
            if operation == "/v1/reduce":
                barrier.wait()
            return call(url, operation, body, token, timeout)
        self.assertTrue(run(self.urls, 100, TOKEN, rpc=rpc)["verified"])

    def test_network_read_timeout(self):
        original = self.servers[1][0].RequestHandlerClass
        class SlowHandler(original):
            def do_POST(self):
                if self.path == "/v1/reduce":
                    time.sleep(0.2)
                super().do_POST()
        self.servers[1][0].RequestHandlerClass = SlowHandler
        with self.assertRaises(TimeoutError):
            run(self.urls, 100, TOKEN, timeout=0.05)

    def test_input_validation(self):
        for count in (1, 2_000_001, True):
            with self.assertRaises(ValueError):
                run(self.urls, count, TOKEN)
        with self.assertRaises(ValueError):
            run([self.urls[0]] * 2, 100, TOKEN)
        with self.assertRaises(ValueError):
            call("http://example.com/unexpected", "/v1/status", {}, TOKEN, 1)


if __name__ == "__main__":
    unittest.main()
