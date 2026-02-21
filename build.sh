#!/bin/bash
cd "$(dirname "$0")/crates/murmur-app"
cargo tauri build
echo ""
echo "Build complete. Output:"
echo "  Installer: ../../target/release/bundle/nsis/"
echo "  Standalone: ../../target/release/murmur-app.exe"
