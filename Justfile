alias ta := test-all
alias t := test

# run all the CI integration tasks.
ci: fmt clippy test-all
    # nothing else

# run rustfmt.
fmt:
    cargo fmt --all

# run clippy as in CI.
clippy:
    cargo clippy --locked --all --all-features -- -D warnings

# run all workspace tests with `cargo-nextest` if available, and `cargo test` otherwise.
test-all:
    if command -v cargo-nextest >/dev/null 2>&1; then \
        cargo nextest run --workspace --retries 0 --locked --all-features; \
    else \
        cargo test --workspace --locked --all-features; \
    fi

# run a single test with cargo-nextest or cargo-test.
test TEST ARGS="":
    if command -v cargo-nextest >/dev/null 2>&1; then \
        cargo nextest run --workspace --retries 0 --locked -- {{TEST}} {{ARGS}}; \
    else \
        cargo test --workspace --locked -- {{TEST}} {{ARGS}}; \
    fi
