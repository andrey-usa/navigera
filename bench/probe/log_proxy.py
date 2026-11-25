"""Logging CDP proxy: log_proxy.py <listen_port> <chrome_port> <log>. stdlib only.
Logs each client->browser command (t, id, session, method, params head) and
each browser->client answer/event (t, id or method)."""
import json, os, socket, struct, sys, threading, time

LISTEN, UP, LOG = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
assert len(str(LISTEN)) == len(str(UP))
lock = threading.Lock()
T0 = time.monotonic()


def log(rec):
    rec['t'] = round((time.monotonic() - T0) * 1000, 3)
    with lock, open(LOG, 'a') as f:
        f.write(json.dumps(rec) + '\n')


def recv_exact(s, n):
    buf = b''
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            raise EOFError
        buf += chunk
    return buf


def read_frame(s):
    b0, b1 = recv_exact(s, 2)
    n = b1 & 0x7F
    ext = b''
    if n == 126:
        ext = recv_exact(s, 2); n = struct.unpack('>H', ext)[0]
    elif n == 127:
        ext = recv_exact(s, 8); n = struct.unpack('>Q', ext)[0]
    key = recv_exact(s, 4) if b1 & 0x80 else None
    data = recv_exact(s, n)
    raw = bytes([b0, b1]) + ext + (key or b'') + data
    if key:
        data = bytes(c ^ key[i % 4] for i, c in enumerate(data))
    return b0, raw, data


def note(direction, b0, data):
    if b0 & 0x0F != 1:
        return
    try:
        m = json.loads(data)
    except Exception:
        return
    if direction == 'c':
        p = m.get('params', {})
        head = p.get('expression') or p.get('functionDeclaration') or p.get('type') or ''
        log({'d': 'send', 'id': m.get('id'), 's': (m.get('sessionId') or '')[:6], 'm': m.get('method'), 'p': str(head)[:90]})
    elif 'id' in m:
        log({'d': 'ans', 'id': m['id'], 's': (m.get('sessionId') or '')[:6], 'err': 'error' in m})
    else:
        log({'d': 'evt', 's': (m.get('sessionId') or '')[:6], 'm': m.get('method')})


def pump(src, dst, direction):
    try:
        while True:
            b0, raw, data = read_frame(src)
            note(direction, b0, data)
            dst.sendall(raw)
    except (EOFError, OSError):
        pass
    finally:
        for s in (src, dst):
            try: s.shutdown(socket.SHUT_RDWR)
            except OSError: pass


def serve(client):
    up = socket.create_connection(('127.0.0.1', UP))
    try:
        head = b''
        while b'\r\n\r\n' not in head:
            chunk = client.recv(65536)
            if not chunk:
                return
            head += chunk
        up.sendall(head.replace(b'127.0.0.1:%d' % LISTEN, b'127.0.0.1:%d' % UP).replace(b'localhost:%d' % LISTEN, b'localhost:%d' % UP))
        if b'upgrade: websocket' in head.lower():
            resp = b''
            while b'\r\n\r\n' not in resp:
                resp += up.recv(1)
            client.sendall(resp)
            threading.Thread(target=pump, args=(up, client, 'b'), daemon=True).start()
            pump(client, up, 'c')
        else:
            def down():
                try:
                    while True:
                        d = up.recv(65536)
                        if not d: break
                        client.sendall(d.replace(b'127.0.0.1:%d' % UP, b'127.0.0.1:%d' % LISTEN).replace(b'localhost:%d' % UP, b'localhost:%d' % LISTEN))
                except OSError: pass
            threading.Thread(target=down, daemon=True).start()
            while True:
                chunk = client.recv(65536)
                if not chunk: break
                up.sendall(chunk)
    except (EOFError, OSError):
        pass


srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(('127.0.0.1', LISTEN)); srv.listen(16)
while True:
    c, _ = srv.accept()
    threading.Thread(target=serve, args=(c,), daemon=True).start()
