default:
    @just --list

fmt:
    @cargo fmt --all

fmt-check:
    @cargo fmt --all -- --check

lint:
    @cargo clippy --all-targets --all-features -- -D warnings

test:
    @cargo test --all-features --all-targets -- --no-capture

deny:
    @cargo deny check

check: deny fmt-check lint test

clean:
    @cargo clean

commit: check
    @git add -A
    @git commit
