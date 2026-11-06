#!/usr/bin/env python3
"""Trap pages for chained work after an action's response.

Each trap page has one button (#go) and an output line (#out). The click
handler fetches /first (150 ms delay), and on its response schedules a
second fetch (/second, 150 ms) or other work per the trap. The report the
driver reads after the click either shows the final state ("final: <trap>")
or an in-between one.

Traps:
  sync         response -> DOM write -> second fetch started in the handler
  microtask    response -> DOM write -> second fetch in queueMicrotask
  timer0       ... in setTimeout 0
  timer30      ... in setTimeout 30 ms
  timer120     ... in setTimeout 120 ms
  raf          ... in requestAnimationFrame
  msgchan      ... in a post-paint MessageChannel (rAF, then port.postMessage)
  placeholder  response -> "Loading..." placeholder -> second fetch at once
  analytics    response -> analytics POST, no DOM change, no second fetch
  longpoll     click starts a fetch the server holds open 30 s
  gap150       response -> DOM write -> second fetch in setTimeout 150 ms
"""
import json
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DELAY_MS = 150

PAGE = """<!doctype html><title>trap {name}</title>
<button id="go">go</button><div id="out">idle</div>
<script>
const out = (t) => {{ document.getElementById('out').textContent = t; }};
const second = () => fetch('/second').then(r => r.text()).then(t => out('final: {name} ' + t));
document.getElementById('go').addEventListener('click', () => {{
  {body}
}});
</script>
"""

BODIES = {
    "sync": "fetch('/first').then(r => r.text()).then(() => { out('phase one'); second(); });",
    "microtask": "fetch('/first').then(r => r.text()).then(() => { out('phase one'); queueMicrotask(second); });",
    "timer0": "fetch('/first').then(r => r.text()).then(() => { out('phase one'); setTimeout(second, 0); });",
    "timer30": "fetch('/first').then(r => r.text()).then(() => { out('phase one'); setTimeout(second, 30); });",
    "timer120": "fetch('/first').then(r => r.text()).then(() => { out('phase one'); setTimeout(second, 120); });",
    "raf": "fetch('/first').then(r => r.text()).then(() => { out('phase one'); requestAnimationFrame(() => second()); });",
    "msgchan": """fetch('/first').then(r => r.text()).then(() => { out('phase one');
      const c = new MessageChannel();
      c.port1.onmessage = () => second();
      requestAnimationFrame(() => c.port2.postMessage(1)); });""",
    "placeholder": "fetch('/first').then(r => r.text()).then(() => { out('Loading...'); second(); });",
    "analytics": "out('working'); fetch('/first').then(() => { fetch('/beacon', {method: 'POST', body: 'x'}); });",
    "longpoll": "out('listening'); fetch('/slow').then(() => out('polled'));",
    "gap150": "fetch('/first').then(r => r.text()).then(() => { out('phase one'); setTimeout(second, 150); });",
}

TRAPS = list(BODIES)


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _text(self, s, delay=0):
        if delay:
            time.sleep(delay / 1000)
        b = s.encode()
        self.send_response(200)
        self.send_header("content-type", "text/plain")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        if self.path.startswith("/trap/"):
            name = self.path.split("/")[2]
            if name not in BODIES:
                self._text("no such trap", 0)
                return
            b = PAGE.format(name=name, body=BODIES[name]).encode()
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.send_header("content-length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)
        elif self.path == "/first":
            self._text("one", DELAY_MS)
        elif self.path == "/second":
            self._text("two", DELAY_MS)
        elif self.path == "/slow":
            self._text("late", 30000)
        else:
            self._text("ok")

    def do_POST(self):
        if self.path == "/beacon":
            self._text("seen")
        else:
            self._text("ok")


if __name__ == "__main__":
    import sys
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8973
    print(json.dumps({"port": port, "traps": TRAPS}))
    ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
