"""End-to-end test of the fixed templates server.py and main.py.

Run with:  python3 -m unittest discover -s crates/laoban-m0/python-tests

Starts main.py with a sample handler and server.py with sample static
files on ephemeral loopback ports, then checks static serving and the
api/ proxy. No network beyond loopback.
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest
import urllib.error
import urllib.request

TEMPLATES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "templates")

SAMPLE_HANDLER = '''
import json

ORDERS = []


def handle(method, path, body):
    if path == "/orders" and method == "GET":
        return 200, {"orders": ORDERS}
    if path == "/orders" and method == "POST":
        order = json.loads(body.decode("utf-8"))
        ORDERS.append(order)
        return 201, {"ok": True, "count": len(ORDERS)}
    if path == "/boom":
        raise RuntimeError("secret detail that must not leak")
    if path == "/echo":
        return 200, {"method": method, "path": path, "body": body.decode("utf-8")}
    return 404, {"error": "no such endpoint"}
'''

INDEX = '<!doctype html><html><head><link rel="stylesheet" href="style.css"></head>' \
        '<body><script src="app.js"></script></body></html>'
APP_JS = 'fetch("api/orders").then(r => r.json());'
STYLE = 'body { color: #333; }'

NO_PROXY = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def start(script, env):
    full_env = dict(os.environ)
    full_env.update(env)
    full_env["PORT"] = "0"
    full_env["PYTHONDONTWRITEBYTECODE"] = "1"
    proc = subprocess.Popen(
        [sys.executable, os.path.join(TEMPLATES, script)],
        env=full_env,
        stderr=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        text=True,
    )
    line = proc.stderr.readline()
    if "listening on port" not in line:
        proc.kill()
        raise RuntimeError("unexpected startup line: %r" % line)
    return proc, int(line.rsplit(" ", 1)[1])


def fetch(port, path, method="GET", body=None, headers=None):
    req = urllib.request.Request(
        "http://127.0.0.1:%d%s" % (port, path), data=body, method=method, headers=headers or {}
    )
    try:
        with NO_PROXY.open(req, timeout=10) as resp:
            return resp.status, resp.headers.get("Content-Type", ""), resp.read()
    except urllib.error.HTTPError as err:
        return err.code, err.headers.get("Content-Type", ""), err.read()


class RuntimeTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        api_dir = os.path.join(cls.tmp.name, "api")
        web_dir = os.path.join(cls.tmp.name, "web")
        os.makedirs(api_dir)
        os.makedirs(web_dir)
        with open(os.path.join(api_dir, "handler.py"), "w") as f:
            f.write(SAMPLE_HANDLER)
        for name, content in (("index.html", INDEX), ("app.js", APP_JS), ("style.css", STYLE)):
            with open(os.path.join(web_dir, name), "w") as f:
                f.write(content)
        # A file next to the static ones that must never be served.
        with open(os.path.join(web_dir, "server.py"), "w") as f:
            f.write("# not public\n")

        cls.api, cls.api_port = start("main.py", {"LAOBAN_CODE_DIR": api_dir})
        cls.web, cls.web_port = start(
            "server.py",
            {
                "LAOBAN_STATIC_DIR": web_dir,
                "LAOBAN_API_URL": "http://127.0.0.1:%d/" % cls.api_port,
            },
        )
        # A web server whose API is down.
        cls.orphan, cls.orphan_port = start(
            "server.py",
            {"LAOBAN_STATIC_DIR": web_dir, "LAOBAN_API_URL": "http://127.0.0.1:9/"},
        )

    @classmethod
    def tearDownClass(cls):
        for proc in (cls.api, cls.web, cls.orphan):
            proc.kill()
            proc.wait()
            proc.stderr.close()
        cls.tmp.cleanup()

    def test_static_files_with_content_types(self):
        status, ctype, body = fetch(self.web_port, "/")
        self.assertEqual((status, body.decode()), (200, INDEX))
        self.assertTrue(ctype.startswith("text/html"))
        status, ctype, body = fetch(self.web_port, "/app.js")
        self.assertEqual((status, body.decode()), (200, APP_JS))
        self.assertTrue(ctype.startswith("text/javascript"))
        status, ctype, _ = fetch(self.web_port, "/style.css?v=1")
        self.assertEqual(status, 200)
        self.assertTrue(ctype.startswith("text/css"))
        status, _, body = fetch(self.web_port, "/index.html", method="HEAD")
        self.assertEqual((status, body), (200, b""))

    def test_only_whitelisted_files_are_served(self):
        for path in ("/server.py", "/../server.py", "/%2e%2e/etc/passwd", "/nope.html", "/web/index.html"):
            self.assertEqual(fetch(self.web_port, path)[0], 404, path)
        self.assertEqual(fetch(self.web_port, "/", method="POST", body=b"x")[0], 405)

    def test_proxy_end_to_end(self):
        order = json.dumps({"item": "蛋餅", "qty": 2}).encode("utf-8")
        status, ctype, body = fetch(
            self.web_port, "/api/orders", method="POST", body=order,
            headers={"Content-Type": "application/json"},
        )
        self.assertEqual(status, 201, body)
        self.assertTrue(ctype.startswith("application/json"))
        self.assertTrue(json.loads(body)["ok"])
        status, _, body = fetch(self.web_port, "/api/orders")
        self.assertEqual(status, 200)
        self.assertIn({"item": "蛋餅", "qty": 2}, json.loads(body)["orders"])

    def test_proxy_passes_method_body_and_strips_prefix(self):
        for method in ("PUT", "PATCH", "DELETE"):
            status, _, body = fetch(self.web_port, "/api/echo?x=1", method=method, body=b"hello")
            self.assertEqual(status, 200)
            self.assertEqual(json.loads(body), {"method": method, "path": "/echo", "body": "hello"})

    def test_handler_status_is_passed_through(self):
        status, _, body = fetch(self.web_port, "/api/missing")
        self.assertEqual(status, 404)
        self.assertEqual(json.loads(body), {"error": "no such endpoint"})

    def test_exception_is_500_without_traceback(self):
        for port, path in ((self.api_port, "/boom"), (self.web_port, "/api/boom")):
            status, _, body = fetch(port, path)
            self.assertEqual(status, 500)
            self.assertEqual(json.loads(body), {"error": "internal error"})
            self.assertNotIn(b"Traceback", body)
            self.assertNotIn(b"secret detail", body)

    def test_api_down_is_502(self):
        status, _, body = fetch(self.orphan_port, "/api/orders")
        self.assertEqual(status, 502)
        self.assertEqual(json.loads(body), {"error": "api unavailable"})
        # Static files still work.
        self.assertEqual(fetch(self.orphan_port, "/")[0], 200)


if __name__ == "__main__":
    unittest.main()
