# Post-Mortem: big_space + Avian3D Transform Sync

**Date:** 2024-12-07
**Severity:** High (physics broken at planetary scales)
**Status:** Resolved

## The Problem

Avian3D's built-in transform sync systems lose f64 precision when used with big_space:

```
Avian's sync path (broken):

  GlobalTransform (f32, camera-relative)
         |
         v
  Position (f64) ──────> PRECISION LOST
         |
         v
  Physics runs at wrong coordinates
```

At Venus (~100 billion meters), f32 precision drops to ~10km. Ships would jitter or collide incorrectly.

## Architecture

### The Precision Problem

```
┌─────────────────────────────────────────────────────────────────┐
│                    COORDINATE SYSTEMS                           │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  big_space:                                                     │
│  ┌──────────────┐   ┌──────────────┐                           │
│  │ CellCoord    │ + │ Transform    │  = World Position (f64)   │
│  │ (i64 cell)   │   │ (f32 local)  │                           │
│  │ 1,000,000    │   │ 500m offset  │    10,000,000,500m        │
│  └──────────────┘   └──────────────┘                           │
│         │                  │                                    │
│         │    10km cells    │                                    │
│         ▼                  ▼                                    │
│  ┌─────────────────────────────────────┐                       │
│  │          Grid (10km cells)          │                       │
│  │  ┌─────┬─────┬─────┬─────┬─────┐   │                       │
│  │  │cell │cell │cell │cell │cell │   │                       │
│  │  │ 999 │1000 │1001 │1002 │1003 │   │                       │
│  │  └─────┴─────┴─────┴─────┴─────┘   │                       │
│  └─────────────────────────────────────┘                       │
│                                                                 │
│  Avian:                                                         │
│  ┌──────────────┐                                              │
│  │ Position     │  World-space f64                             │
│  │ (DVec3)      │  10,000,000,500m                             │
│  └──────────────┘                                              │
│                                                                 │
│  Bevy Rendering:                                                │
│  ┌──────────────┐                                              │
│  │GlobalTransform│  Camera-relative f32 (small values = good)  │
│  │ (f32)        │  500m from camera                            │
│  └──────────────┘                                              │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### The Solution: Custom Sync Systems

```
Frame Timeline:
═══════════════════════════════════════════════════════════════════

  FixedPreUpdate              FixedUpdate              FixedPostUpdate
       │                          │                          │
       ▼                          ▼                          ▼
┌──────────────────┐    ┌──────────────────┐    ┌──────────────────┐
│ FORWARD SYNC     │    │ PHYSICS          │    │ REVERSE SYNC     │
│                  │    │                  │    │                  │
│ CellCoord        │    │ Avian solver     │    │ Position         │
│     +            │───>│ updates          │───>│     +            │
│ Transform        │    │ Position         │    │ Rotation         │
│     │            │    │                  │    │     │            │
│     ▼            │    │                  │    │     ▼            │
│ Position (f64)   │    │                  │    │ CellCoord        │
│                  │    │                  │    │     +            │
│                  │    │                  │    │ Transform        │
└──────────────────┘    └──────────────────┘    └──────────────────┘

Source of truth:         Source of truth:         Source of truth:
  CellCoord+Transform      Position                 Position
```

## The Bug

Forward sync was scheduled in `FixedPostUpdate` instead of `FixedPreUpdate`:

```rust
// WRONG - runs AFTER physics
.add_systems(FixedPostUpdate, big_space_transform_to_position ...)

// CORRECT - runs BEFORE physics
.add_systems(FixedPreUpdate, big_space_transform_to_position ...)
```

This meant:
1. Physics ran with stale/zero Position values
2. Forward sync overwrote physics results
3. Reverse sync had nothing useful to write back

## Hierarchy Handling

Both sync systems walk the entity tree, maintaining a world-space cache:

```
BigSpace (root)
    │
    ├── Planet [CellCoord + Transform]
    │       │
    │       └── Arena [Transform only]
    │               │
    │               └── Ship [Transform + Position]
    │
    └── Camera [CellCoord + Transform + FloatingOrigin]


Forward Sync (tree walk):
─────────────────────────
1. Planet: world = grid.grid_position_double(cell, transform)
2. Arena:  world = parent_world + rotate(local_offset)
3. Ship:   world = parent_world + rotate(local_offset)
           Position = world  ← write to physics

Reverse Sync (tree walk):
─────────────────────────
1. Planet: has Position? use it : compute from CellCoord
           → update CellCoord + Transform from Position
2. Arena:  local = inverse(parent_world) * my_world
           → update Transform
3. Ship:   local = inverse(parent_world) * Position
           → update Transform
```

## Key Insight: No Tick-Based Change Detection Needed

Initial concern: bidirectional sync might fight itself. Avian uses tick-based detection to know which component was modified "more recently."

Why we don't need it:

```
Frame N:
  FixedPreUpdate:  CellCoord+Transform is SOURCE → writes Position
  FixedUpdate:     Physics modifies Position
  FixedPostUpdate: Position is SOURCE → writes CellCoord+Transform

No ambiguity. Schedule order defines source of truth.
```

## Test Strategy

```rust
#[test]
fn big_space_sync_with_proper_scheduling() {
    // Manually run schedules in order (no time system needed)
    app.world_mut().run_schedule(FixedPreUpdate);   // Forward sync
    app.world_mut().run_schedule(FixedUpdate);      // Fake physics
    app.world_mut().run_schedule(FixedPostUpdate);  // Reverse sync

    // Verify round-trip: cell crossing works correctly
    // Body moved 15km → crossed into next cell
    // CellCoord updated: 1,000,000 → 1,000,002
    // Local transform: 500m → -4500m
    // Reconstructed position matches physics result
}
```

## Lessons Learned

1. **Schedule placement is critical** - one wrong schedule breaks the entire sync flow
2. **Test the full round-trip** - isolated unit tests missed the schedule ordering bug
3. **Manual schedule execution** - `app.world_mut().run_schedule()` is cleaner than fighting Bevy's time system in tests
4. **big_space + physics = custom sync required** - can't use Avian's built-in sync with floating origin rendering

## Files Changed

- `src/physics.rs`: Fixed schedule, added round-trip test
