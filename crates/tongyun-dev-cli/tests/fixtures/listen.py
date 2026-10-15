"""TCP listener for readiness tests.

CPython's ``http.server`` calls ``socket.getfqdn()`` after ``bind()`` and
before ``listen()``. On macOS 15 and later that reverse lookup can block for
tens of seconds, so the process stays alive while ``connect()`` is still
refused. This script only listens on 127.0.0.1.
"""

import socket
import sys

port = int(sys.argv[1])
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(("127.0.0.1", port))
sock.listen(8)
while True:
    conn, _addr = sock.accept()
    conn.close()
