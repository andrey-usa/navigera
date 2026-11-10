"""The fixture sites' HTTP server: a ThreadingHTTPServer that binds without
looking up its host's name.

HTTPServer.server_bind ends with socket.getfqdn(host), a reverse lookup
whose answer nothing here reads. On GitHub's macOS runners it takes about
35 s, paid at every start. A script in this directory imports it by name:
Python puts the script's own directory first on its path.
"""

import socketserver
from http.server import ThreadingHTTPServer


class Server(ThreadingHTTPServer):
    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]
