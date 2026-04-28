profile := "debug"

cargo-profile := if profile == "release" { "--release" } else { "" }

# Build all components for wasm32-wasip2.
# Use `just build profile=release` for a release build.
build:
    cargo build -p wordmark --target wasm32-wasip2 {{cargo-profile}}
    cargo build -p tablemark --target wasm32-wasip2 {{cargo-profile}}

# Trigger the `Publish all Components` workflow on CI for the given version,
# then watch the resulting run until it completes.
# Example: `just publish 1.2.0`
publish version:
    gh workflow run publish.yml --field version={{version}}
    @echo "Waiting for run to start..."
    @sleep 3
    gh run watch --exit-status $(gh run list --workflow=publish.yml --limit 1 --json databaseId --jq '.[0].databaseId')
