#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

cargo build --release --locked --target x86_64-pc-windows-gnu

