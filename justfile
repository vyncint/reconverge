# Development workflows. Run `just setup` once per clone; keep `just ci`
# green before every push — never push red.

# materialize the pinned toolchain and wire the repo-local git hooks
setup:
    rustup toolchain install
    rustup show
    git config core.hooksPath .githooks

fmt:
    cargo fmt --all

test:
    cargo test --workspace

# wire the repo-local git hooks (commit-msg soft guard)
hooks:
    git config core.hooksPath .githooks

# everything CI gates on, locally
ci:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    # conformance/extractor is its own workspace, so the lines above stop at
    # its door; CI gates it separately and so does this.
    cargo fmt --manifest-path conformance/extractor/Cargo.toml --all --check
    cargo clippy --manifest-path conformance/extractor/Cargo.toml --locked --all-targets -- -D warnings
    cargo test --workspace
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
    cargo deny check
    ./scripts/check-isolation.sh
    ./scripts/check-plurals.sh
