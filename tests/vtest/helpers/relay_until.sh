#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 vigil-rs contributors
#
# Run vigil-log-relay with --stdout-sink until its output has a line
# matching PATTERN, or until it exits by itself; at most 5 seconds.
#
# Usage: relay_until.sh RELAY OUT PATTERN [relay options…]
#
# stdout goes to OUT, the relay's log to OUT.log. The relay gets
# --stdout-sink and --healthcheck 127.0.0.1:0 in front of the options.
# The exit code is the relay's when it exited by itself (it was not
# killed), else 0.
#
# Example:
#   relay_until.sh target/debug/vigil-log-relay /tmp/x/a.out "ERROR last" \
#       --source-url http://127.0.0.1:8080/stream

relay=$1
out=$2
pattern=$3
shift 3

"$relay" --stdout-sink --healthcheck 127.0.0.1:0 "$@" \
	> "$out" 2> "$out.log" < /dev/null &
pid=$!
for _ in $(seq 1 50); do
	grep -q -- "$pattern" "$out" && break
	kill -0 "$pid" 2> /dev/null || break
	sleep 0.1
done
if kill "$pid" 2> /dev/null; then
	wait "$pid"
	exit 0
fi
wait "$pid"
