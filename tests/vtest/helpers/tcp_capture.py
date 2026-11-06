#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 vigil-rs contributors
"""Minimal TCP sink for vtests: appends everything clients send to a file.

Usage: tcp_capture.py [--port N] <port-file> <output-file>

Binds 127.0.0.1 and writes the port to <port-file> once it listens, so the
test can wait for that file. Without --port the kernel picks a free port (no
race between picking a port and binding it). --port N binds a given port,
e.g. to restart a sink on the port a client is already configured for.

Accepts any number of connections, one after another or at the same time.
"""

import argparse
import os
import socketserver
import threading


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("port_file")
    parser.add_argument("out_file")
    args = parser.parse_args()

    lock = threading.Lock()

    class Handler(socketserver.BaseRequestHandler):
        def handle(self) -> None:
            while True:
                data = self.request.recv(65536)
                if not data:
                    return
                with lock, open(args.out_file, "ab") as f:
                    f.write(data)

    class Server(socketserver.ThreadingTCPServer):
        allow_reuse_address = True
        daemon_threads = True

    with Server(("127.0.0.1", args.port), Handler) as server:
        # Write atomically: the test polls for a non-empty file.
        tmp = args.port_file + ".tmp"
        with open(tmp, "w") as f:
            f.write(f"{server.server_address[1]}\n")
        os.replace(tmp, args.port_file)
        server.serve_forever()


if __name__ == "__main__":
    main()
