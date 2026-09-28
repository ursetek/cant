//! Compile-fail tests for `#[system]`.
//!
//! Run with `TRYBUILD=overwrite cargo test --test system_errors` once to
//! generate the `.stderr` files, then keep them in sync on later runs.

#[test]
fn compile_failures() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
