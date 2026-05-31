profile := "debug"

cargo-profile := if profile == "release" { "--release" } else { "" }

# Build all components for wasm32-wasip2.
# Use `just build profile=release` for a release build.
build:
    cargo build -p wordmark --target wasm32-wasip2 {{cargo-profile}}
    cargo build -p tablemark --target wasm32-wasip2 {{cargo-profile}}
    cargo build -p textsearch --target wasm32-wasip2 {{cargo-profile}}

# Build all interface-type WIT packages into .wasm files under target/wit/.
# Output: target/wit/<name>.wasm
build-wit:
    mkdir -p target/wit
    wkg wit build -d interface-types/docs -o target/wit/docs.wasm
    wkg wit build -d interface-types/acp -o target/wit/acp.wasm

# Trigger the `Publish Component` workflow on CI for a single target at the
# given version, then watch the resulting run until it completes.
# `target` must be one of: wordmark, tablemark, textsearch, docs, acp.
# Example: `just publish wordmark 1.2.0`
publish target version:
    gh workflow run publish.yml --field target={{target}} --field version={{version}}
    @echo "Waiting for run to start..."
    @sleep 3
    gh run watch --exit-status $(gh run list --workflow=publish.yml --limit 1 --json databaseId --jq '.[0].databaseId')

# --- Wasm-component test harness (see testing/README.md) -------------------
#
# Tests are themselves Wasm components that export `wasi:test/tests`. Each is
# composed with its component-under-test plus a generic runner (via WAC) into a
# single component, then executed with `wasmtime`. The harness lives in the
# separate `testing/` workspace (excluded from the root workspace).

# Build the reusable test harness (generic runner + every suite) for
# wasm32-wasip2. Built in release so the async helper crate uses `opt-level=s`.
build-test-infra:
    cd testing && cargo build --release --target wasm32-wasip2

# Build, compose, and run the test suite for a single component.
# `name` must be one of: textsearch, wordmark, tablemark.
# Always (re)builds the component-under-test and the harness first so the
# composed artifact never goes stale.
test-component name: build-test-infra
    cargo build -p {{name}} --release --target wasm32-wasip2
    mkdir -p target/test
    wac compose \
        --dep yosh:{{name}}=target/wasm32-wasip2/release/{{name}}.wasm \
        --dep yosh:{{name}}-tests=testing/target/wasm32-wasip2/release/{{name}}_tests.wasm \
        --dep yosh:test-runner=testing/target/wasm32-wasip2/release/wasi-test-runner-cli.wasm \
        testing/compositions/{{name}}.wac \
        -o target/test/{{name}}-test.wasm
    wasmtime run -Wcomponent-model-async target/test/{{name}}-test.wasm

# Build, compose, and run every component's test suite.
test-components: (test-component "textsearch") (test-component "wordmark") (test-component "tablemark")

# Show the latest semver tag published to GHCR for each package.
# Skips non-semver tags (e.g. `latest`). Prints `<package>: <version>` per line,
# or `<package>: -` if no semver tag has been published yet.
versions:
    @for pkg in wordmark tablemark textsearch docs acp; do \
        latest=$(gh api -H "Accept: application/vnd.github+json" \
            "/users/yoshuawuyts/packages/container/components%2F$pkg/versions" \
            --jq '[.[].metadata.container.tags[]? | select(test("^v?[0-9]+\\.[0-9]+\\.[0-9]+([-+].*)?$"))] | unique | .[]' 2>/dev/null \
            | sed 's/^v//' \
            | sort -V \
            | tail -n1); \
        printf '%-10s %s\n' "$pkg" "${latest:--}"; \
    done
