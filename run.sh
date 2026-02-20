#!/bin/bash
cd "$(dirname "$0")"
RUST_LOG=voitex_app_lib=info,voitex_core=debug,warn cargo run -p voitex-app
