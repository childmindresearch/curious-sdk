# Published OpenAPI spec for the full Prism mock.
openapi_url := "https://api-v2.mindlogger.org/openapi.json"

# List available commands.
help:
    @just --list

# Build the generated client.
build-api:
    openapi-to-rust generate --config "{{justfile_directory()}}/openapi-to-rust.toml"
    git -C "{{justfile_directory()}}" apply patches/openapi-to-rust-strict-utf8.patch

# Start the Prism mock with deterministic SDK response examples.
mock-sdk:
    mkdir -p "{{justfile_directory()}}/target"
    python3 "{{justfile_directory()}}/tests/prism_spec.py" "{{openapi_url}}" > "{{justfile_directory()}}/target/prism-sdk.json"
    existing="$(docker ps -aq --filter 'name=^/curious-sdk-prism$')" && if [ -n "$existing" ]; then docker rm -f "$existing"; fi
    docker run --rm --name curious-sdk-prism -p 127.0.0.1:4010:4010 \
        -v "{{justfile_directory()}}/target/prism-sdk.json:/prism-sdk.json:ro" node:22-slim \
        npx --yes @stoplight/prism-cli mock -h 0.0.0.0 /prism-sdk.json

# Run the Rust SDK integration tests against the running Prism mock.
test-mock:
    cargo test --test mock_api -- --ignored
