# Timeline Engine

Event-sourced history for the geometry kernel. Every operation is an
immutable event in a per-branch log; the model state at any point is
derived by replaying events from the branch root.

The point of this design (over a parametric feature tree) is to let
agents fork the timeline, explore variations in parallel, and either
merge a branch back or discard it without untangling parent-child
dependencies. The kernel itself is stateless from this layer's view —
it consumes operations and emits topology.

## What's here

- `Timeline` (`timeline.rs`) — owns the per-branch event log, the
  `DashMap` of branches and checkpoints, and the
  `add_operation` / `create_checkpoint` / `merge_branches` surface.
- `BranchManager` (`branch/mod.rs`) — standalone branch registry with
  `create_ai_branch` for agent exploration and scoring helpers.
  Currently lives alongside `Timeline` rather than inside it; both
  are exercised by the api-server.
- `ExecutionEngine` (`execution/mod.rs`) — replays events against a
  `BRepModel` via the kernel's `OperationRecorder` trait.
- `CacheManager` (`cache/mod.rs`) — LRU (`lru::LruCache`) over
  per-branch operation outputs and tessellation.
- `Storage` (`storage/`) — append-only event log, snapshot files,
  index.

## Status

In active use by the api-server. The delete-from-timeline path has a
known bug when an operation recorded events before failing partway
through (tracked in `MEMORY.md`); everything else is operational. The
storage layer writes locally; cross-process persistence is not yet
wired.

## Usage

Add the crate to a workspace member's `Cargo.toml`:

```toml
[dependencies]
timeline-engine = { path = "../timeline-engine" }
```

### Adding operations to a timeline

```rust
use timeline_engine::prelude::*;

let timeline = Timeline::new(TimelineConfig::default());
let main = BranchId::main();
let author = Author::System;

let sketch_id = EntityId::new();
let sketch_op = Operation::CreateSketch {
    plane: SketchPlane::XY,
    elements: vec![SketchElement::Rectangle {
        corner: [0.0, 0.0],
        width: 100.0,
        height: 50.0,
    }],
};
timeline.add_operation(sketch_op, author.clone(), main).await?;

let extrude_op = Operation::Extrude {
    sketch_id,
    distance: 25.0,
    direction: None,
};
timeline.add_operation(extrude_op, author.clone(), main).await?;

timeline
    .create_checkpoint(
        "Base part complete".to_string(),
        "Created base rectangular solid".to_string(),
        main,
        author,
        vec!["milestone".to_string()],
    )
    .await?;
```

`Timeline::new` is synchronous and returns the value directly;
`add_operation`, `create_checkpoint`, and `merge_branches` are async
and yield `TimelineResult<_>`.

### Forking a branch for agent exploration

```rust
let branches = BranchManager::new();
let exploration = branches.create_ai_branch(
    BranchId::main(),
    fork_event_index,
    "weight-optimizer".to_string(),
    "claude-opus-4-6".to_string(),
    OptimizationObjective::MinimizeWeight,
    Vec::new(),
)?;

// ... append Operation::Transform events on `exploration` via
//     timeline.add_operation(...) ...

timeline
    .merge_branches(exploration, BranchId::main(), MergeStrategy::FastForward)
    .await?;
```

`MergeStrategy` covers `FastForward`, `ThreeWay`, `Rebase`, `Squash`,
and `CherryPick`. Divergent histories require `ThreeWay` with a
`ConflictStrategy`.

## Extending

To add a new kernel operation to the timeline:

1. Add a variant to `Operation` in `types.rs`.
2. Register a handler in `operations/mod.rs` and `execution/registry.rs`.
3. Implement the replay path against `BRepModel` via
   `OperationRecorder`.
4. Add unit tests against a fresh `Timeline`.

Existing operation modules (`operations/extrude.rs`,
`operations/fillet.rs`, etc.) are the working reference.
