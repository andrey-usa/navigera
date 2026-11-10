#!/usr/bin/env python3
"""Offline bind/HTTP smoke checks: python3 bench/test_local_servers.py."""
import ast
import importlib.util
import socket
import socketserver
import sys
import threading
import unittest
import urllib.request
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = (
    'bench/webarena/harproxy.py',
    'bench/quiet-traps/server.py',
    'bench/decide/relay.py',
    'bench/decide/compare.py',
    'bench/llm/llm_bench.py',
    'bench/gate/gate.py',
    'bench/ide-eval/mock_llm.py',
    'bench/decide/mock_jev.py',
    'bench/site/server.py',
    'tests/fixtures/local_server.py',
)


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'local server ok')

    def log_message(self, *_):
        pass


class LocalServers(unittest.TestCase):
    def test_http_servers_skip_reverse_lookup(self):
        for index, script in enumerate(SCRIPTS):
            with self.subTest(script=script):
                path = ROOT / script
                spec = importlib.util.spec_from_file_location(f'local_server_{index}', path)
                module = importlib.util.module_from_spec(spec)
                sys.modules[spec.name] = module
                spec.loader.exec_module(module)
                # Guard the construction sites as well as the subclass itself.
                tree = ast.parse(path.read_text())
                self.assertFalse(any(
                    isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                    and node.func.id in ('HTTPServer', 'ThreadingHTTPServer')
                    for node in ast.walk(tree)
                ))
                server_class = getattr(module, 'Server', module.ThreadingHTTPServer)
                with patch.object(socket, 'getfqdn', side_effect=AssertionError('reverse lookup')):
                    with server_class(('127.0.0.1', 0), Handler) as server:
                        self.assertEqual(server.server_name, '127.0.0.1')
                        self.assertEqual(server.server_port, server.server_address[1])
                        self.assertGreater(server.server_port, 0)
                        thread = threading.Thread(target=server.handle_request)
                        server.timeout = 2
                        thread.start()
                        try:
                            with urllib.request.urlopen(
                                f'http://127.0.0.1:{server.server_port}/', timeout=2
                            ) as response:
                                self.assertEqual(response.read(), b'local server ok')
                        finally:
                            thread.join(timeout=3)
                        self.assertFalse(thread.is_alive())

    def test_ladder_tcp_server_needs_no_override(self):
        with patch.object(socket, 'getfqdn', side_effect=AssertionError('reverse lookup')):
            with socketserver.ThreadingTCPServer(('127.0.0.1', 0), Handler) as server:
                self.assertGreater(server.server_address[1], 0)


if __name__ == '__main__':
    unittest.main()
