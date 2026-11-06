#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 vigil-rs contributors
"""Write a request body for POST /v1/identities with one TLS identity.

Usage: tls_identity.py <name> <access> <ca.crt> <output.json>

Every client certificate signed by the CA in <ca.crt> authenticates as
<name> with <access> (read, write or admin). The CA is created by the test
at runtime, so this body cannot be a static fixture.

Example output:
  {"identities": {"ops": {"access": "read", "tls": {"ca-cert": "-----BEGIN..."}}}}
"""

import json
import sys


def main() -> None:
    name, access, ca_file, out_file = sys.argv[1:5]
    with open(ca_file) as f:
        ca = f.read()
    body = {"identities": {name: {"access": access, "tls": {"ca-cert": ca}}}}
    with open(out_file, "w") as f:
        json.dump(body, f)


if __name__ == "__main__":
    main()
