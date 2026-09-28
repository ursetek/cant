//! Chunk projection throughput.

#![allow(missing_docs)]

mod common;

use std::hint::black_box;

use core::ops::ControlFlow;

use criterion::{
    BenchmarkId, Criterion, Throughput, criterion_group, criterion_main,
};

use cant::{ChunkSink, QueryMask, RawChunk};
use common::{Position, world_with_motion};

/// Sink that only counts rows; isolates projection overhead from any
/// consumer-side work.
struct Counting;

impl ChunkSink for Counting {
    fn consume(&mut self, chunk: RawChunk<'_>) -> ControlFlow<()> {
        black_box(chunk.len());
        ControlFlow::Continue(())
    }
}

fn bench_project_visit(c: &mut Criterion) {
    let mut group = c.benchmark_group("project_visit");
    for &n in &[1_000u32, 10_000, 100_000] {
        let world = world_with_motion(n);
        let pos = world.component_id::<Position>().unwrap();
        let mut mask = QueryMask::new();
        mask.read(pos);
        let query = world.prepare(mask).unwrap();

        group.throughput(Throughput::Elements(u64::from(n)));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let mut sink = Counting;
                let _ = query.project(&world, &mut sink);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_project_visit);
criterion_main!(benches);
