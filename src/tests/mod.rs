//! Unit tests for crate internals.
//!
//! These tests live in a dedicated module tree so they can access
//! `pub(crate)` items without exposing them to external users. Each test
//! documents the invariant it protects, not just the mechanics it exercises.
//!
//! Integration tests — those that only touch the public API — live under
//! `tests/` at the crate root.

#![allow(dead_code)]

mod archetype;
mod bundle;
mod commands;
mod component;
mod entity;
mod mask;
mod projection;
mod query;
mod schedule;
mod storage;
mod system;
mod world;
