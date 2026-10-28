#!/usr/bin/env python3
"""Pages for the ax scaling test: /list?n=<count> renders a <ul> of n
items, each a button, a link and a span, so the accessibility tree is
large while the printed `ax` output is capped at the output limit.

Prints "LISTENING http://127.0.0.1:<port>" and serves until killed.
"""
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        u = urlparse(self.path)
        if u.path != "/list":
            self.send_response(404)
            self.end_headers()
            return
        n = int(parse_qs(u.query).get("n", ["0"])[0])
        items = "".join(
            f'<li><button>Item {i}</button> <a href="/item/{i}">Details {i}</a> '
            f"<span>Description for item {i}</span></li>"
            for i in range(n)
        )
        body = (
            "<!doctype html><html><head><title>List bench</title></head><body>"
            f"<h1>List of {n} items</h1><ul>{items}</ul>"
            "</body></html>"
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(f"LISTENING http://127.0.0.1:{srv.server_address[1]}", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    sys.exit(0)
