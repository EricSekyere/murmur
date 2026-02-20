param(
    [Parameter(Position=0)]
    [ValidateSet("run", "run-full", "download-model", "install", "check", "fmt")]
    [string]$Command = "run"
)

$env:CMAKE = $null  # prevent cmake-rs treating a directory as the executable

switch ($Command) {
    "run" {
        $env:RUST_LOG = "voitex_app_lib=info,voitex_core=debug,warn"
        cargo run -p voitex-app
    }
    "run-full" {
        cargo run -p voitex-app --features full
    }
    "download-model" {
        cargo run -p voitex-cli --features full -- models --download small.en
    }
    "install" {
        cargo install --path crates/voitex-cli --features full
    }
    "check" {
        cargo check --workspace
    }
    "fmt" {
        cargo fmt --all
        cargo clippy --workspace
    }
    default {
        Write-Host "Usage: .\dev.ps1 [run|run-full|download-model|install|check|fmt]"
    }
}
