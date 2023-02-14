#!/usr/bin/env bash

set -euo pipefail

mkdir -p storage
dd if=/dev/urandom of=storage/5M.data bs=1M count=5
dd if=/dev/urandom of=storage/10M.data bs=1M count=10
dd if=/dev/urandom of=storage/50M.data bs=1M count=50
