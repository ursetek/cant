//! Bundle registration and chunk-level projection through the public API.

use core::ops::ControlFlow;

use cant::{ChunkSink, ChunkSource, QueryMask, RawChunk, RawChunkMut, World};

#[derive(Debug, PartialEq, Clone, Copy)]
struct Position(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Velocity(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Health(u32);

/// Two tuple orderings share an archetype but get distinct bundle ids.
#[test]
fn bundle_orderings_share_archetype() {
    let mut world = World::new();
    let _p = world.register_component::<Position>("Position");
    let _v = world.register_component::<Velocity>("Velocity");

    let ab = world.register_bundle::<(Position, Velocity)>();
    let ba = world.register_bundle::<(Velocity, Position)>();
    assert_ne!(ab, ba);
    assert_eq!(world.bundle_archetype(ab), world.bundle_archetype(ba));

    let x = world.spawn_bundle_id(ab, (Position(1.0, 2.0), Velocity(3.0, 4.0)));
    let y = world.spawn_bundle_id(ba, (Velocity(5.0, 6.0), Position(7.0, 8.0)));
    assert_eq!(
        world.location(x).unwrap().archetype,
        world.location(y).unwrap().archetype,
    );
}

/// A bundle with a single component spawns into the matching archetype.
#[test]
fn single_component_bundle() {
    let mut world = World::new();
    let pos = world.register_component::<Position>("Position");
    let bid = world.register_bundle::<(Position,)>();

    let e = world.spawn_bundle_id(bid, (Position(1.0, 2.0),));
    let loc = world.location(e).unwrap();
    assert!(world.archetype(loc.archetype).has_component(pos));
    assert_eq!(loc.archetype, world.bundle_archetype(bid));
}

/// A sink that collects every column's bytes into a flat buffer and records
/// the chunk count.
struct Collector {
    chunks: u32,
    entities: u64,
    bytes: Vec<u8>,
}

impl Collector {
    fn new() -> Self {
        Self {
            chunks: 0,
            entities: 0,
            bytes: Vec::new(),
        }
    }
}

impl ChunkSink for Collector {
    fn consume(&mut self, chunk: RawChunk<'_>) -> ControlFlow<()> {
        self.chunks += 1;
        self.entities += u64::from(chunk.len());
        for col in chunk.columns() {
            self.bytes.extend_from_slice(col.bytes());
        }
        ControlFlow::Continue(())
    }
}

/// Projection visits matching chunks and yields raw bytes.
#[test]
fn project_collects_bytes() {
    let mut world = World::new();
    let pos = world.register_component::<Position>("Position");

    for i in 0..4 {
        let e = world.spawn();
        #[allow(clippy::cast_precision_loss)]
        world.add_component(e, Position(i as f32, 0.0));
    }

    let mut mask = QueryMask::new();
    mask.read(pos);
    let query = world.prepare(mask).unwrap();

    let mut sink = Collector::new();
    let result = query.project(&world, &mut sink);
    assert_eq!(result, ControlFlow::Continue(()));
    assert_eq!(sink.chunks, 1);
    assert_eq!(sink.entities, 4);
    assert_eq!(sink.bytes.len(), 4 * core::mem::size_of::<Position>());
}

/// A source that emits `remaining` rows filled with a fixed byte pattern.
struct Filler {
    remaining: u32,
    fill: u8,
}

impl ChunkSource for Filler {
    fn next_chunk_len(&mut self, max_rows: u32) -> u32 {
        let n = self.remaining.min(max_rows);
        self.remaining -= n;
        n
    }

    unsafe fn produce(&mut self, mut chunk: RawChunkMut<'_>) {
        for col in chunk.columns_mut() {
            for b in col.bytes_mut() {
                *b = self.fill;
            }
        }
    }
}

/// `World::ingest` creates entities in the requested archetype.
#[test]
fn ingest_populates_world() {
    let mut world = World::new();
    let _p = world.register_component::<Position>("Position");
    let _v = world.register_component::<Velocity>("Velocity");
    let bid = world.register_bundle::<(Position, Velocity)>();
    let archetype = world.bundle_archetype(bid);

    let mut src = Filler {
        remaining: 10,
        fill: 0,
    };
    let created = world.ingest(archetype, &mut src);
    assert_eq!(created, 10);
    assert_eq!(world.live_count(), 10);
    assert_eq!(world.archetype(archetype).len(), 10);
}

/// Round-trip: collect bytes from one world, ingest them into another.
#[test]
fn roundtrip_through_projection() {
    /// Source that replays a pre-collected byte buffer.
    struct Replay {
        bytes: Vec<u8>,
        offset: usize,
        remaining: u32,
    }

    impl ChunkSource for Replay {
        fn next_chunk_len(&mut self, max_rows: u32) -> u32 {
            let n = self.remaining.min(max_rows);
            self.remaining -= n;
            n
        }

        unsafe fn produce(&mut self, mut chunk: RawChunkMut<'_>) {
            for col in chunk.columns_mut() {
                let take = col.bytes_mut().len();
                col.bytes_mut().copy_from_slice(
                    &self.bytes[self.offset..self.offset + take],
                );
                self.offset += take;
            }
        }
    }

    let mut world = World::new();
    let pos = world.register_component::<Position>("Position");

    for i in 0..3 {
        let e = world.spawn();
        #[allow(clippy::cast_precision_loss)]
        world.add_component(e, Position(i as f32, 0.0));
    }

    let mut mask = QueryMask::new();
    mask.read(pos);
    let query = world.prepare(mask).unwrap();

    let mut sink = Collector::new();
    let _ = query.project(&world, &mut sink);
    assert_eq!(sink.entities, 3);

    let mut world2 = World::new();
    let pos2 = world2.register_component::<Position>("Position");
    let bid2 = world2.register_bundle::<(Position,)>();
    let arch2 = world2.bundle_archetype(bid2);

    let mut src = Replay {
        bytes: sink.bytes,
        offset: 0,
        remaining: 3,
    };
    assert_eq!(world2.ingest(arch2, &mut src), 3);

    let mut read_mask = QueryMask::new();
    read_mask.read(pos2);
    let mut read_query = world2.prepare(read_mask).unwrap();
    let mut values = Vec::new();
    read_query.for_each(&world2, |view| {
        for p in view.column::<Position>(0).unwrap() {
            values.push(p.0);
        }
    });
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(values, vec![0.0, 1.0, 2.0]);
}

/// Zero-sized component bundles round-trip through projection without
/// touching any bytes.
#[test]
fn zst_component_projection() {
    #[derive(Debug, Clone, Copy)]
    struct Marker;

    let mut world = World::new();
    let marker = world.register_component::<Marker>("Marker");
    let bid = world.register_bundle::<(Marker,)>();
    let e = world.spawn_bundle_id(bid, (Marker,));

    let mut mask = QueryMask::new();
    mask.read(marker);
    let query = world.prepare(mask).unwrap();

    let mut sink = Collector::new();
    let _ = query.project(&world, &mut sink);
    assert_eq!(sink.entities, 1);
    assert_eq!(sink.bytes.len(), 0, "ZST column contributes no bytes");

    let _ = e;
}

/// `Health` is exercised indirectly to keep the file's other types focused.
#[test]
fn extra_component_type_is_registrable() {
    let mut world = World::new();
    let h = world.register_component::<Health>("Health");
    let e = world.spawn();
    assert!(world.add_component(e, Health(50)));
    assert!(
        world
            .archetype(world.location(e).unwrap().archetype)
            .has_component(h)
    );
}
