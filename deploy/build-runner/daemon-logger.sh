#!/bin/sh
# Keep a bounded private prefix while continuing to drain the daemon's pipe.
set -eu
head -c 24000
exec cat >/dev/null
