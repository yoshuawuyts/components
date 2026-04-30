profile := "debug"

cargo-profile := if profile == "release" { "--release" } else { "" }

# Build all components for wasm32-wasip2.
# Use `just build profile=release` for a release build.
build:
    cargo build -p wordmark --target wasm32-wasip2 {{cargo-profile}}
    cargo build -p tablemark --target wasm32-wasip2 {{cargo-profile}}

# Build all interface-type WIT packages into .wasm files under target/wit/.
# Output: target/wit/<name>.wasm
build-wit:
    mkdir -p target/wit
    wkg wit build -d interface-types/docs -o target/wit/docs.wasm
    wkg wit build -d interface-types/acp -o target/wit/acp.wasm

# Trigger the `Publish Component` workflow on CI for a single target at the
# given version, then watch the resulting run until it completes.
# `target` must be one of: wordmark, tablemark, docs, acp.
# Example: `just publish wordmark 1.2.0`
publish target version:
    gh workflow run publish.yml --field target={{target}} --field version={{version}}
    @echo "Waiting for run to start..."
    @sleep 3
    gh run watch --exit-status $(gh run list --workflow=publish.yml --limit 1 --json databaseId --jq '.[0].databaseId')
