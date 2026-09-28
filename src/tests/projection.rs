//! Tests for chunk projection: sink visit order, source-driven ingest.

use core::ops::ControlFlow;

use crate::projection::{ChunkSink, ChunkSource, RawChunk, RawChunkMut};
use crate::{QueryMask, World};

#[derive(Debug, PartialEq, Clone, Copy)]
struct Pos(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Vel(f32, f32);

// ---------------------------------------------------------------------------
// Sinks
// ---------------------------------------------------------------------------

/// Records chunk and entity counts, plus the first chunk's per-column byte
/// lengths.
struct CountingSink {
    chunks: u32,
    entities: u64,
    first_chunk_bytes: Vec<usize>,
}

impl CountingSink {
    fn new() -> Self {
        Self {
            chunks: 0,
            entities: 0,
            first_chunk_bytes: Vec::new(),
        }
    }
}

impl ChunkSink for CountingSink {
    fn consume(&mut self, chunk: RawChunk<'_>) -> ControlFlow<()> {
        if self.chunks == 0 {
            self.first_chunk_bytes =
                chunk.columns().iter().map(|c| c.bytes().len()).collect();
        }
        self.chunks += 1;
        self.entities += u64::from(chunk.len());
        ControlFlow::Continue(())
    }
}

/// Breaks on the very first chunk.
struct StopAfterFirst;

impl ChunkSink for StopAfterFirst {
    fn consume(&mut self, _chunk: RawChunk<'_>) -> ControlFlow<()> {
        ControlFlow::Break(())
    }
}

/// Collects all bytes from all columns into one buffer.
struct ByteCollector {
    bytes: Vec<u8>,
    count: u32,
}

impl ChunkSink for ByteCollector {
    fn consume(&mut self, chunk: RawChunk<'_>) -> ControlFlow<()> {
        self.count += chunk.len();
        for col in chunk.columns() {
            self.bytes.extend_from_slice(col.bytes());
        }
        ControlFlow::Continue(())
    }
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

/// Fills `total` rows with zero bytes for every column.
struct ZerosSource {
    remaining: u32,
}

impl ChunkSource for ZerosSource {
    fn next_chunk_len(&mut self, max_rows: u32) -> u32 {
        let n = self.remaining.min(max_rows);
        self.remaining -= n;
        n
    }

    unsafe fn produce(&mut self, mut chunk: RawChunkMut<'_>) {
        for col in chunk.columns_mut() {
            let bytes = col.bytes_mut();
            for b in bytes.iter_mut() {
                *b = 0;
            }
        }
    }
}

/// Replays previously collected bytes.
struct BytesSource {
    bytes: Vec<u8>,
    offset: usize,
    remaining: u32,
}

impl ChunkSource for BytesSource {
    fn next_chunk_len(&mut self, max_rows: u32) -> u32 {
        let n = self.remaining.min(max_rows);
        self.remaining -= n;
        n
    }

    unsafe fn produce(&mut self, mut chunk: RawChunkMut<'_>) {
        for col in chunk.columns_mut() {
            if col.layout().size() == 0 {
                continue;
            }
            let take = col.bytes_mut().len();
            col.bytes_mut()
                .copy_from_slice(&self.bytes[self.offset..self.offset + take]);
            self.offset += take;
        }
    }
}

/// Reports a length larger than the current chunk can hold.
struct GreedySource;

impl ChunkSource for GreedySource {
    fn next_chunk_len(&mut self, max_rows: u32) -> u32 {
        max_rows + 1
    }
    unsafe fn produce(&mut self, _chunk: RawChunkMut<'_>) {}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// `project` visits every matching chunk exactly once.
#[test]
fn project_visits_all_chunks() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");

    for i in 0..5 {
        let e = world.spawn();
        #[allow(clippy::cast_precision_loss)]
        world.add_component(e, Pos(i as f32, 0.0));
    }

    let mut mask = QueryMask::new();
    mask.read(pos);
    let query = world.prepare(mask).unwrap();

    let mut sink = CountingSink::new();
    let result = query.project(&world, &mut sink);
    assert_eq!(result, ControlFlow::Continue(()));
    assert_eq!(sink.entities, 5);
    assert_eq!(sink.chunks, 1, "all five rows fit one chunk");
    assert_eq!(
        sink.first_chunk_bytes,
        vec![5 * core::mem::size_of::<Pos>()],
    );
}

/// A sink returning `Break` stops projection early.
#[test]
fn project_stops_on_break() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let e = world.spawn();
    world.add_component(e, Pos(0.0, 0.0));

    let mut mask = QueryMask::new();
    mask.read(pos);
    let query = world.prepare(mask).unwrap();

    let result = query.project(&world, &mut StopAfterFirst);
    assert_eq!(result, ControlFlow::Break(()));
}

/// `ingest` creates the requested number of entities and writes component
/// bytes through the source.
#[test]
fn ingest_creates_entities() {
    let mut world = World::new();
    let _pos = world.register_component::<Pos>("Pos");
    let _vel = world.register_component::<Vel>("Vel");

    let bid = world.register_bundle::<(Pos, Vel)>();
    let archetype = world.bundle_archetype(bid);

    let mut source = ZerosSource { remaining: 7 };
    let created = world.ingest(archetype, &mut source);

    assert_eq!(created, 7);
    assert_eq!(world.live_count(), 7);
    let arch = world.archetype(archetype);
    assert_eq!(arch.len(), 7);
}

/// Ingest spanning multiple chunks works when the source splits itself.
#[test]
fn ingest_across_chunks() {
    let mut world = World::new();
    let _pos = world.register_component::<Pos>("Pos");
    let bid = world.register_bundle::<(Pos,)>();
    let archetype = world.bundle_archetype(bid);
    let cap = world.archetype(archetype).chunk_capacity();
    let total = cap + 100;

    let mut source = ZerosSource { remaining: total };
    let created = world.ingest(archetype, &mut source);

    assert_eq!(created, total);
    assert_eq!(world.archetype(archetype).len(), total);
    assert_eq!(world.archetype(archetype).chunk_count(), 2);
}

/// A source returning `0` on the first call creates no entities.
#[test]
fn ingest_empty_source() {
    let mut world = World::new();
    let _pos = world.register_component::<Pos>("Pos");
    let bid = world.register_bundle::<(Pos,)>();
    let archetype = world.bundle_archetype(bid);

    let mut source = ZerosSource { remaining: 0 };
    assert_eq!(world.ingest(archetype, &mut source), 0);
    assert_eq!(world.live_count(), 0);
}

/// A source returning more than `max_rows` panics.
#[test]
#[should_panic(expected = "only")]
fn ingest_overreaching_source_panics() {
    let mut world = World::new();
    let _pos = world.register_component::<Pos>("Pos");
    let bid = world.register_bundle::<(Pos,)>();
    let archetype = world.bundle_archetype(bid);

    world.ingest(archetype, &mut GreedySource);
}

/// Round-trip: sink the current world, feed it back through a source, get
/// the same component bytes out.
#[test]
fn project_then_ingest_roundtrip() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");

    for i in 0..4 {
        let e = world.spawn();
        #[allow(clippy::cast_precision_loss)]
        world.add_component(e, Pos(i as f32, 0.0));
    }

    let mut mask = QueryMask::new();
    mask.read(pos);
    let mut query = world.prepare(mask).unwrap();

    let mut collector = ByteCollector {
        bytes: Vec::new(),
        count: 0,
    };
    let _ = query.project(&world, &mut collector);
    assert_eq!(collector.count, 4);

    let mut world2 = World::new();
    let _pos2 = world2.register_component::<Pos>("Pos");
    let bid = world2.register_bundle::<(Pos,)>();
    let archetype = world2.bundle_archetype(bid);

    let mut src = BytesSource {
        bytes: collector.bytes,
        offset: 0,
        remaining: 4,
    };
    let created = world2.ingest(archetype, &mut src);
    assert_eq!(created, 4);

    let mut values = Vec::new();
    query.for_each(&world2, |view| {
        let ps = view.column::<Pos>(0).unwrap();
        for p in ps {
            values.push(p.0);
        }
    });
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(values, vec![0.0, 1.0, 2.0, 3.0]);
}
