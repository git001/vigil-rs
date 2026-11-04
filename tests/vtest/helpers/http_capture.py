#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 vigil-rs contributors
"""Minimal HTTP server for vtests: appends every POST body as one line to a
file and answers 200.

Usage: http_capture.py <port-file> <output-file>

Binds 127.0.0.1 on a free port chosen by the kernel and writes the port to
<port-file> once it listens, so the test can wait for that file (no race
between picking a port and binding it).
"""

import http.server
import os
import sys


def main() -> None:
    port_file, out_file = sys.argv[1], sys.argv[2]

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            n = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(n)
            with open(out_file, "ab") as f:
                f.write(body + b"\n")
            self.send_response(200)
            self.end_headers()

        def log_message(self, *args) -> None:
            pass

    srv = http.server.HTTPServer(("127.0.0.1", 0), Handler)
    tmp = port_file + ".tmp"
    with open(tmp, "w") as f:
        f.write(f"{srv.server_address[1]}\n")
    os.replace(tmp, port_file)  # appears complete or not at all
    srv.serve_forever()


if __name__ == "__main__":
    main()
