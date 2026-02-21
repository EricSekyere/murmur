.PHONY: run run-full download-model install check fmt

# Launch the Tauri app (no STT — model banner will show if model missing)
run:
	RUST_LOG=murmur_app_lib=info,murmur_core=debug,warn cargo run -p murmur-app

# Launch the Tauri app with full STT/VAD support
run-full:
	unset CMAKE && cargo run -p murmur-app --features full

# Download the default small.en model (~488 MB)
download-model:
	unset CMAKE && cargo run -p murmur-cli --features full -- models --download small.en

# Install the murmur CLI globally so `murmur` works in any terminal
install:
	unset CMAKE && cargo install --path crates/murmur-cli --features full

# Quick compile check
check:
	cargo check --workspace

# Format + lint
fmt:
	cargo fmt --all && cargo clippy --workspace
