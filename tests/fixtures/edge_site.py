#!/usr/bin/env python3
"""Edge-case pages for tests/edge_cases.rs: each page reproduces one way a
browser driver goes wrong (a page that never finishes loading, a slow
popup, a styled checkbox, a cross-origin iframe, ...).

Serves two origins from one process: the main one on 127.0.0.1 and a second
port reached as `localhost` (a different origin) for cross-origin iframes.
Prints `LISTENING http://127.0.0.1:<port> <other-port>` once both are up.
"""
import http.server
import socketserver
import sys
import threading
import time
import urllib.parse

OTHER = {"port": 0}

PAGES = {
    # A visually hidden checkbox under a styled box, inside its label.
    "/checkbox": """<!doctype html><title>cb</title><style>
label{position:relative;display:inline-block;padding:4px}
input{position:absolute;left:4px;top:4px;margin:0;width:20px;height:20px}
.box{position:relative;display:inline-block;width:20px;height:20px;background:#ccc}
</style><label><input type=checkbox id=agree><span class=box></span> I agree</label>
<div id=out></div><script>agree.addEventListener('click',e=>out.textContent='trusted='+e.isTrusted+' checked='+agree.checked)</script>""",
    # The <title> repeats a link's text exactly.
    "/title": """<!doctype html><title>Products</title><nav><a href="/landed">All products</a></nav>""",
    "/landed": """<!doctype html><title>Landed</title><h1>landed</h1>""",
    "/xframe": """<!doctype html><title>outer</title><h1>outer</h1>
<iframe id=f src="http://localhost:{other}/inner" width=400 height=200 style="margin-top:300px"></iframe>""",
    "/sframe": """<!doctype html><title>outer</title><h1>outer</h1>
<iframe id=f src="/inner" width=400 height=200 style="margin-top:300px"></iframe>""",
    "/inner": """<!doctype html><title>inner</title><button id=b onclick="this.textContent='clicked trusted='+event.isTrusted">Pay now</button>""",
    "/popup": """<!doctype html><title>opener</title><a href="/slow?ms=1500" target=_blank>open slow</a>""",
    "/hanglink": """<!doctype html><title>hl</title><a href="/hang">go hang</a>""",
    "/upload": """<!doctype html><title>up</title><input type=file id=f><div id=o></div><script>f.onchange=()=>o.textContent=f.files[0].name+':'+f.files[0].size</script>""",
    # A hidden copy ahead of the visible element.
    "/twice": """<!doctype html><title>two</title><div class=t style="display:none">hidden</div><div class=t>shown</div>""",
    "/keys": """<!doctype html><title>keys</title><input id=i autofocus><p id=p>no form here</p>""",
    # A single-page app: the list arrives by fetch, then renders a beat later.
    "/spa": """<!doctype html><title>spa</title><p id=s role=status>Loading items…</p><script>
fetch('/slow?ms=300').then(r => r.text()).then(() => setTimeout(() => { s.textContent = 'Loaded 3 items' }, 200))</script>""",
    "/counter": """<!doctype html><title>counter</title><p>Page <span id=n>0</span></p><button id=next onclick="n.textContent=+n.textContent+1">Next page</button>""",
    "/prompt": """<!doctype html><title>prompt</title><button onclick="o.textContent='got:'+prompt('Name?','Ada')">ask</button><div id=o></div>""",
}


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        query = urllib.parse.parse_qs(url.query)
        if url.path == "/hang":
            # Commits, renders a heading, then never finishes the document.
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.end_headers()
            self.wfile.write(b"<!doctype html><title>hang</title><h1>partial</h1>" + b" " * 2048)
            self.wfile.flush()
            time.sleep(600)
            return
        if url.path == "/slow":
            time.sleep(int(query.get("ms", ["1000"])[0]) / 1000)
            body = b"<!doctype html><title>slow</title><h1>slow page</h1>"
        elif url.path in PAGES:
            body = PAGES[url.path].replace("{other}", str(OTHER["port"])).encode()
        else:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 0
    main_srv = Server(("127.0.0.1", port), Handler)
    other_srv = Server(("127.0.0.1", 0), Handler)
    OTHER["port"] = other_srv.server_address[1]
    threading.Thread(target=other_srv.serve_forever, daemon=True).start()
    print(f"LISTENING http://127.0.0.1:{main_srv.server_address[1]} {OTHER['port']}", flush=True)
    main_srv.serve_forever()


if __name__ == "__main__":
    main()
