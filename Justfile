default:
    @just --list

fmt:
    @cargo fmt --all

fmt-check:
    @cargo fmt --all -- --check

lint:
    @cargo clippy -- -D warnings
    @cargo clippy --profile=test -- -D warnings

test:
    @cargo test -- --no-capture

deny:
    @cargo deny check

check: deny fmt-check lint test

clean:
    @cargo clean

commit: check
    @git add -A
    @git commit

bench:
    @cargo bench

pub:
    @cargo publish -p cant-macros
    @cargo publish -p cant
