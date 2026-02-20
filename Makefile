.PHONY: run run-full download-model install check fmt

# Launch the Tauri app (no STT — model banner will show if model missing)
run:
	RUST_LOG=voitex_app_lib=info,voitex_core=debug,warn cargo run -p voitex-app

# Launch the Tauri app with full STT/VAD support
run-full:
	unset CMAKE && cargo run -p voitex-app --features full

# Download the default small.en model (~488 MB)
download-model:
	unset CMAKE && cargo run -p voitex-cli --features full -- models --download small.en

# Install the voitex CLI globally so `voitex` works in any terminal
install:
	unset CMAKE && cargo install --path crates/voitex-cli --features full

# Quick compile check
check:
	cargo check --workspace

# Format + lint
fmt:
	cargo fmt --all && cargo clippy --workspace
