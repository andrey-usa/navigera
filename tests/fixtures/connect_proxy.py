"""An HTTP proxy that tunnels with CONNECT, as a corporate one does.

    python3 connect_proxy.py <log file> [<user:password>]

Listens on 127.0.0.1 on a free port and prints it on one line. With
credentials, a CONNECT without the matching `Proxy-Authorization: Basic`
gets 407. Each tunnel it opens is appended to the log as
`CONNECT <host:port>`; then bytes are relayed both ways.
"""

import asyncio
import base64
import sys


async def pipe(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except ConnectionError:
        pass
    finally:
        writer.close()


async def main():
    log = sys.argv[1]
    want = None
    if len(sys.argv) > 2:
        want = "Basic " + base64.b64encode(sys.argv[2].encode()).decode()

    async def handle(reader, writer):
        head = await reader.readuntil(b"\r\n\r\n")
        lines = head.decode("latin-1").split("\r\n")
        method, target, _ = lines[0].split(" ", 2)
        headers = {}
        for line in lines[1:]:
            if ":" in line:
                k, v = line.split(":", 1)
                headers[k.strip().lower()] = v.strip()
        if method != "CONNECT":
            writer.write(b"HTTP/1.1 405 Method Not Allowed\r\n\r\n")
            writer.close()
            return
        if want and headers.get("proxy-authorization") != want:
            writer.write(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\n\r\n")
            writer.close()
            return
        host, port = target.rsplit(":", 1)
        try:
            inner_reader, inner_writer = await asyncio.open_connection(host.strip("[]"), int(port))
        except OSError:
            writer.write(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
            writer.close()
            return
        with open(log, "a") as f:
            f.write(f"CONNECT {target}\n")
        writer.write(b"HTTP/1.1 200 Connection established\r\n\r\n")
        await writer.drain()
        await asyncio.gather(pipe(reader, inner_writer), pipe(inner_reader, writer))

    server = await asyncio.start_server(handle, "127.0.0.1", 0)
    print(server.sockets[0].getsockname()[1], flush=True)
    async with server:
        await server.serve_forever()


asyncio.run(main())
