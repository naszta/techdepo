# List the recipes.
[private]
default:
    @just --list --unsorted

# Native-only and wasm-only members: xtask is native only; the services worker and the SPA are
# wasm32 only.
native := "--workspace --exclude techdepo-services-worker --exclude techdepo-web"
wasm := "--workspace --exclude xtask --target wasm32-unknown-unknown"

# Run all CI jobs (the heavy builds last; they never overlap).
ci: fmt check clippy test deny catalog eval build-workers build-web

# Check formatting.
fmt:
    cargo fmt --all --check

# Check everything that ships (wasm32) and everything that builds natively (services, tests, xtask).
check:
    cargo check {{ wasm }} --locked
    cargo check {{ native }} --all-targets --locked

# Run clippy for wasm32 and natively (with the tests), denying warnings.
clippy:
    cargo clippy {{ wasm }} --locked -- --deny warnings
    cargo clippy {{ native }} --all-targets --locked -- --deny warnings

# Run the tests (natively; the db, services and mail-in tests run on SQLite).
test:
    cargo test {{ native }} --locked

# Check advisories, licenses, bans and sources (deny.toml).
deny:
    cargo deny --locked check

# Validate the embedded catalog.
catalog:
    cargo run --locked -p xtask -- catalog validate

# Run the support evals on the mock models (fails below the eval files' minimums).
eval:
    cargo run --locked -p xtask -- eval classify --backend mock --out target/evals/classify-mock.md
    cargo run --locked -p xtask -- eval agents --backend mock --out target/evals/agents-mock.md

# Release builds of the three Workers (worker-build, into apps/*/build).
build-workers:
    cd apps/services && worker-build --release
    cd apps/web-worker && worker-build --release
    cd apps/mail-in && worker-build --release

# Release build of the SPA into apps/web-worker/public.
build-web:
    scripts/build-web.sh
