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

# Run the Wasm component test suites. Each suite is itself a Wasm component
# (under `testing/`) that is composed with the real component-under-test and a
# generic runner via `wac plug`, then executed with `wasmtime`.
# Requires `wac` and `wasmtime` on PATH. See `testing/README.md`.
test:
    cargo build -p textsearch --target wasm32-wasip2 {{cargo-profile}}
    cd testing && cargo build --target wasm32-wasip2 {{cargo-profile}}
    mkdir -p target/test
    just _run-suite textsearch textsearch_tests

# (internal) Compose `<component>` with its `<suite>` and the generic runner,
# then execute the resulting test component under `wasmtime`.
_run-suite component suite:
    wac plug \
        --plug target/wasm32-wasip2/{{profile}}/{{component}}.wasm \
        testing/target/wasm32-wasip2/{{profile}}/{{suite}}.wasm \
        -o target/test/{{component}}-suite.wasm
    wac plug \
        --plug target/test/{{component}}-suite.wasm \
        testing/target/wasm32-wasip2/{{profile}}/test-runner.wasm \
        -o target/test/{{component}}-test-runner.wasm
    wasmtime run target/test/{{component}}-test-runner.wasm

# Trigger the `Publish Component` workflow on CI for a single target at the
# given version, then watch the resulting run until it completes.
# `target` must be one of: wordmark, tablemark, textsearch, docs, acp.
# Example: `just publish wordmark 1.2.0`
publish target version:
    gh workflow run publish.yml --field target={{target}} --field version={{version}}
    @echo "Waiting for run to start..."
    @sleep 3
    gh run watch --exit-status $(gh run list --workflow=publish.yml --limit 1 --json databaseId --jq '.[0].databaseId')

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
