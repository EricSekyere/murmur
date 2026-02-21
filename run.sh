#!/bin/bash
cd "$(dirname "$0")"

# Kill any running instance so the build can replace the binary
taskkill //F //IM murmur-app.exe 2>/dev/null
sleep 1

RUST_LOG=murmur_app_lib=debug,murmur_core=debug,warn cargo run -p murmur-app
