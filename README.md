# cant

[![crates.io](https://img.shields.io/crates/v/cant.svg)](https://crates.io/crates/cant)
[![docs.rs](https://img.shields.io/docsrs/cant)](https://docs.rs/cant)
[![license](https://img.shields.io/crates/l/cant.svg)](#license)
[![MSRV](https://img.shields.io/badge/MSRV-1.98-blue.svg)](#)

A dynamic archetype ECS for Rust. Components and systems register at runtime;
hot paths still index `Vec`s.

```rust
#[derive(Component, Debug, Clone, Copy)]
struct Position { x: f32, y: f32 }

#[derive(Component, Debug, Clone, Copy)]
struct Velocity { x: f32, y: f32 }

#[system]
#[resources(dt: f32)]
fn integrate(positions: &mut [Position], velocities: &[Velocity]) {
    for (p, v) in positions.iter_mut().zip(velocities) {
        p.x += v.x * *dt;
        p.y += v.y * *dt;
    }
}
```

That's a system. The macro derives the query, the access list, and the cost hint
from the signature. Everything else is `u32` arithmetic.

## What makes it different

*   **Runtime everything.** Components, bundles, systems — registered at runtime,
    dense IDs, no compile-time registry. Mods and editors are first-class.
*   **Flat data structures.** Archetype transitions are a `Vec<u32>` lookup. No
    hash maps in hot paths.
*   **Cost-hint scheduling.** Systems declare `#[hint(8_000)]` (ns). The
    scheduler parallelizes only when the batch is worth it. A warm rayon pool,
    not fresh OS threads.
*   **Byte-level projection.** `ChunkSink`/`ChunkSource` move data in and out at
    the chunk level, with zero knowledge of the consumer.
*   **Honest unsafe.** Concentrated in storage and dispatch. Documented
    invariants, not formalities.

## Performance

10k entities, two 8-byte components, `--release`:

| | |
|---|---|
| Read query | 7 µs (1.4 Gelem/s) |
| Write query | 2.8 µs |
| Spawn bundle | 218 µs |
| Migrate component | 474 µs |
| Parallel dispatch, 8 systems | 20.9 µs |

On the same shapes, comparable to `hecs` and `legion`, 2–8× faster than
`bevy_ecs` on spawn and migration. Not the fastest for compile-time-known
workloads: we pay a few percent for runtime flexibility and buy it back on
structural changes.

## When to use it

+   Components or systems are decided at runtime.
+   You want a standalone scheduler, not a framework.
+   You need to move data out of the ECS in bulk.
+   You like macros that expand to code you can read.

## When not to use it

-   You want an engine: use [Bevy].
-   You need 1.0 stability: this is `0.x`.
-   Your workload is fully monomorphizable and you need the last 5%.
-   You want `unsafe` to be someone else's problem.

## Status

Core is written and tested. Not yet done: events, multi-query systems,
serialization helpers. API will churn.

## Why "cant"

`hecs`, `legion`, `shipyard`, `bevy` &mdash; someone had to keep the pattern.
Also because everything here is something you *can't* do in a compile-time ECS.

## Made with care

Every decision has a benchmark behind it. Dense IDs. Flat edge graph.
Version-based query refresh. Byte-arena command buffers. Cost hints. Read the
code &mdash; it explains itself.

## License

Licensed under either MIT or Apache-2.0 at your option.

---

Made with ❤️ for the Rust community.
