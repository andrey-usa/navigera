#!/usr/bin/env python3
"""Two-host pages for tests/allow_domains.rs: one server on 127.0.0.1,
reachable as `localhost` too (a different hostname, same port).

Prints `LISTENING <port>` once up. Serves a plain page at `/` and the
text `DATA-OK` at `/data`, plus a redirect at `/redir` to the `localhost`
form of `/other`. Requests for `/sink…` are counted per Host header and
`/hits` answers how many reached the `localhost` name: a refused request
must never arrive.
"""

import functools
import http.server
import sys
import threading

from local_server import Server

SINK_HITS = {"localhost": 0}
LOCK = threading.Lock()


class H(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/sink"):
            with LOCK:
                host = (self.headers.get("Host") or "").split(":")[0]
                SINK_HITS[host] = SINK_HITS.get(host, 0) + 1
        if self.path == "/hits":
            with LOCK:
                body = str(SINK_HITS.get("localhost", 0)).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path == "/redir":
            self.send_response(302)
            self.send_header("Location", f"http://localhost:{self.server.server_port}/other")
            self.end_headers()
            return
        if self.path == "/data":
            body = b"DATA-OK"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            # The docs-only fetch is cross-origin (127.0.0.1 -> localhost):
            # without this the browser rejects it for CORS, not policy.
            self.send_header("Access-Control-Allow-Origin", "*")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path == "/other":
            body = b"<!doctype html><title>Other host</title><h1>other host page</h1>"
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        body = b"<!doctype html><title>Allowlist probe</title><h1>probe page</h1>"
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


srv = Server(("127.0.0.1", 0), H)
print(f"LISTENING {srv.server_port}", flush=True)
srv.serve_forever()
