profile := "debug"

cargo-profile := if profile == "release" { "--release" } else { "" }

# Build the wordmark component for wasm32-wasip2.
# Use `just build profile=release` for a release build.
build:
    cargo build -p wordmark --target wasm32-wasip2 {{cargo-profile}}
