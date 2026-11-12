"""A TLS front for a local DevTools port, as a browser in the cloud is
reached: `wss://` on the outside, the browser's plain WebSocket inside.

    python3 tls_relay.py <target port> <cert.pem> <key.pem>

Listens on 127.0.0.1 on a free port, prints it on one line, then relays
every connection's bytes both ways to 127.0.0.1:<target port>.
"""

import asyncio
import ssl
import sys


async def pipe(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except (ConnectionError, ssl.SSLError):
        pass
    finally:
        writer.close()


async def main():
    target = int(sys.argv[1])
    ctx = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
    ctx.load_cert_chain(sys.argv[2], sys.argv[3])

    async def relay(reader, writer):
        try:
            inner_reader, inner_writer = await asyncio.open_connection("127.0.0.1", target)
        except OSError:
            writer.close()
            return
        await asyncio.gather(pipe(reader, inner_writer), pipe(inner_reader, writer))

    server = await asyncio.start_server(relay, "127.0.0.1", 0, ssl=ctx)
    print(server.sockets[0].getsockname()[1], flush=True)
    async with server:
        await server.serve_forever()


asyncio.run(main())
