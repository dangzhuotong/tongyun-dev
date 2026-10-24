"""TCP listener for readiness tests.

CPython's ``http.server`` calls ``socket.getfqdn()`` after ``bind()`` and
before ``listen()``. On macOS 15 and later that reverse lookup can block for
tens of seconds, so the process stays alive while ``connect()`` is still
refused. This script only listens on 127.0.0.1.
"""

import argparse
import socket
import sys
import time

parser = argparse.ArgumentParser(description="TCP listener for readiness tests")
parser.add_argument("port", type=int, help="Port to listen on")
parser.add_argument(
    "--delay-secs",
    type=float,
    default=0.0,
    help="Seconds to delay before binding and listening",
)
args = parser.parse_args()

if args.delay_secs > 0:
    time.sleep(args.delay_secs)

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(("127.0.0.1", args.port))
sock.listen(8)
while True:
    conn, _addr = sock.accept()
    conn.close()
