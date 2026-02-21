#!/bin/bash
cd "$(dirname "$0")"
RUST_LOG=murmur_app_lib=info,murmur_core=debug,warn cargo run -p murmur-app
