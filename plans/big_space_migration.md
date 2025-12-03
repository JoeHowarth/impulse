# Big Space Integration Plan for Impulse

## Overview

Integrate big_space 0.11 to solve f32 precision limitations when rendering at solar system scale while zooming to tactical scale (100m ships, 0.1m projectiles).

**Core insight**: The precision problem affects both rendering AND physics sync. While Avian3D uses f64 Position internally, its sync systems read/write through f32 GlobalTransform/Transform, losing precision. We need:
1. big_space to compute camera-relative GlobalTransforms for rendering
2. Custom Avian sync systems that preserve f64 precision via CellCoord

## Architecture

```
f64 orbital mechanics ──► Grid::translation_to_grid() ──► (CellCoord, Transform)
                                                                  │
                                                                  ▼
                                                          GlobalTransform (f32, camera-relative for rendering)

Avian3D f64 Position ◄──► (CellCoord, Transform)  [custom sync, NOT through GlobalTransform]
```

## Avian3D Integration (Critical)

### The Problem

Avian's built-in sync systems are broken for big_space:

1. **`transform_to_position`** (runs before physics):
   - Reads from `GlobalTransform` (f32, camera-relative in big_space!)
   - Writes to `Position` (f64)
   - **Problem**: GlobalTransform is relative to FloatingOrigin, not world space

2. **`position_to_transform`** (runs after physics):
   - Reads from `Position` (f64)
   - Writes to `Transform.translation` via `.f32()` (lossy!)
   - **Problem**: Precision loss at planetary distances

### The Solution

Disable both Avian sync systems and replace with big_space-aware versions:

```rust
// In plugin setup:
app.insert_resource(PhysicsTransformConfig {
    transform_to_position: false,  // We'll handle this
    position_to_transform: false,  // We'll handle this
    ..default()
});
```

**Replacement system 1: `cell_transform_to_position`** (before physics)
```rust
/// Syncs CellCoord + Transform → Avian Position (preserving f64 precision)
fn cell_transform_to_position(
    grids: Query<&Grid>,
    mut bodies: Query<(&ChildOf, &CellCoord, &Transform, &mut Position), Changed<Transform>>,
) {
    for (parent, cell, transform, mut position) in &mut bodies {
        let Ok(grid) = grids.get(parent.parent()) else { continue };
        // Compute world-space f64 position from cell + local transform
        let world_pos: DVec3 = grid.grid_position_double(cell, transform);
        position.0 = world_pos;
    }
}
```

**Replacement system 2: `position_to_cell_transform`** (after physics)
```rust
/// Syncs Avian Position → CellCoord + Transform (preserving f64 precision)
fn position_to_cell_transform(
    grids: Query<&Grid>,
    mut bodies: Query<(&ChildOf, &mut CellCoord, &mut Transform, &Position), Changed<Position>>,
) {
    for (parent, mut cell, mut transform, position) in &mut bodies {
        let Ok(grid) = grids.get(parent.parent()) else { continue };
        // Convert world-space f64 position to cell + small local transform
        let (new_cell, local) = grid.translation_to_grid(position.0);
        *cell = new_cell;
        transform.translation = local;
    }
}
```

### Why World-Space Position (Not Arena-Local)

We considered keeping Position in arena-local coordinates, but:
- Avian expects Position in world space for collision detection, spatial queries
- Multiple simultaneous battles would have ships at overlapping local coordinates
- World-space Position with f64 naturally separates battles by millions of km

With world-space Position:
- Each ship's Position is heliocentric (huge values, but f64 handles it)
- Avian physics works correctly (all ships in same coordinate space)
- CellCoord + Transform gives precise rendering via big_space

### Grid Structure (Simplified)

With world-space Position, ships don't need a subgrid - they can live in the main heliocentric grid:

```
BigSpace (root)
└── Grid (heliocentric, cell_edge_length ~10,000m for tactical precision)
    ├── Camera + FloatingOrigin (CellCoord + Transform)
    ├── Sun (CellCoord + Transform)
    ├── Earth (CellCoord + Transform)
    ├── Mars (CellCoord + Transform)
    ├── Fleet entities (CellCoord + Transform)
    ├── VisualShip (CellCoord + Transform + Position) ← physics entity
    ├── VisualShip (CellCoord + Transform + Position)
    └── Projectile (CellCoord + Transform + Position)
```

**Key insight**: big_space computes GlobalTransform relative to FloatingOrigin's cell. If camera is near the tactical arena, ships near the camera automatically have precise GlobalTransforms. No subgrid needed.

**TacticalArena** becomes a simple marker component (not a Grid):
- Used for organizational grouping (spawn/cleanup)
- Tracks which body the battle is at
- Camera follows arena position by updating its own CellCoord

**Cell edge length**: ~10,000m gives sub-meter precision within each cell, sufficient for 100m ships and 0.1m projectiles.

- **Future**: When multi-SOI support is added, bodies could become subgrids

### High-Precision Caching

GlobalTransform is computed by big_space's propagation system, but we don't control when it runs within a frame. To ensure consistent high-precision positions across all our systems within a frame:

```rust
/// Cached high-precision position computed at start of frame.
/// Bridge solution - may be cleaned up later once system ordering is solid.
#[derive(Component)]
pub struct ComputedPosition {
    pub helio_pos: DVec3,      // f64 heliocentric (or arena-local for tactical)
    pub cell: GridCell<i64>,   // cached cell
    pub local: Vec3,           // cached local offset
}
```

This replaces `ComputedBody.position: Vec3` with f64-precision data that other systems can read before GlobalTransform sync.

---

## Implementation Phases

### Phase 1: Plugin Setup & Camera

**Files**: `src/main.rs`, `src/camera.rs`, `src/physics.rs`

1. Disable Bevy's TransformPlugin, add BigSpaceDefaultPlugins:
   ```rust
   DefaultPlugins.build().disable::<TransformPlugin>(),
   BigSpaceDefaultPlugins,
   ```

2. Disable Avian's built-in transform sync (we'll replace it later):
   ```rust
   app.insert_resource(PhysicsTransformConfig {
       transform_to_position: false,
       position_to_transform: false,
       ..default()
   });
   ```

3. Configure Grid with appropriate cell size for tactical precision:
   ```rust
   let grid = Grid::new(
       10_000.0,  // cell_edge_length: 10km cells
       100.0,     // switching_threshold: recenter when 100m past cell edge
   );
   commands.spawn_big_space(grid, |root| {
       root.spawn_spatial((
           Camera3d::default(),
           FloatingOrigin,
           Projection::from(OrthographicProjection { ... }),
           PanCam { ... },
           CameraTarget::default(),
       ));
   });
   ```

4. Update `animate_camera` to work with CellCoord-aware transforms (camera moves between cells as it pans across solar system)

**Validation**: Camera should render at origin, can pan around (even if nothing else works yet)

---

### Phase 2: Body Positions

**Files**: `src/main.rs`, `src/orbital_data.rs`

1. Replace `ComputedBody` with `ComputedPosition`:
   ```rust
   #[derive(Component, Default)]
   pub struct ComputedPosition {
       pub helio_pos: DVec3,           // f64 absolute position
       pub cell: GridCell<i64>,        // grid cell
       pub local: Vec3,                // local offset within cell
       // Keep visibility/display_size here or separate component
       pub visibility: f32,
       pub display_size: f32,
   }
   ```

2. Add GridCell component to bodies at spawn time (initial cell can be origin)

3. Rewrite `update_body_positions` to compute GridCell + Transform + cache:
   ```rust
   fn update_body_positions(
       mut bodies: Query<(&Body, &mut GridCell<i64>, &mut Transform, &mut ComputedPosition)>,
       grid: Query<&Grid<i64>, With<BigSpace>>,
       sim_time: Res<SimulationTime>,
   ) {
       let grid = grid.single();
       for (body, mut cell, mut transform, mut computed) in &mut bodies {
           let helio_pos: DVec3 = compute_heliocentric_position(body, sim_time);
           let (new_cell, local) = grid.translation_to_grid(helio_pos);

           // Update GridCell + Transform (for big_space propagation)
           *cell = new_cell;
           transform.translation = local;

           // Cache high-precision data for other systems this frame
           computed.helio_pos = helio_pos;
           computed.cell = new_cell;
           computed.local = local;
       }
   }
   ```

4. Update `calculate_visibility` and `compute_display_size` to use ComputedPosition.helio_pos (f64) for distance calculations

**Validation**: Bodies should appear at correct positions, visibility LOD should work

---

### Phase 3: Rendering Systems

**Files**: `src/main.rs`, `src/ship.rs`, `src/transfer_vis.rs`

1. Update `render_system` (body circles):
   - Read GlobalTransform instead of ComputedBody.position
   - painter.set_translation uses global_transform.translation()

2. Update fleet rendering:
   - `ComputedFleetPosition.position` becomes redundant if fleets have GridCell
   - Or keep it as cache of GlobalTransform for convenience
   - `render_fleets`, `render_objectives`, `render_departure_markers`, `render_plan_markers`, `render_plan_arcs` all use GlobalTransform

3. Update `phys_to_visual`:
   - Keep for orbit gizmo generation (parent-relative, stays f32)
   - Or replace with grid-aware version where needed

4. Gizmo orbit rendering:
   - Orbits are rendered relative to parent body
   - Parent body has GlobalTransform (camera-relative)
   - Orbit points are local offsets - should still work
   - May need testing at extreme zoom levels

**Validation**: Bodies render as circles, fleets render as triangles, orbits visible

---

### Phase 4: Click Detection & UI

**Files**: `src/main.rs`, `src/picking.rs`, `src/ui.rs`

1. `camera.world_to_viewport(camera_transform, position)`:
   - Still works because GlobalTransform is camera-relative
   - Position values will be small (near camera) = precise
   - Should require minimal changes

2. `camera.viewport_to_world`:
   - Returns world position relative to camera
   - Need to convert back to GridCell if placing something

3. UI label positioning:
   - Uses world_to_viewport with GlobalTransform - should work

**Validation**: Can click on bodies, UI labels appear correctly

---

### Phase 5: Tactical Mode & Avian Sync Systems

**Files**: `src/tactical.rs`, `src/physics.rs`, `src/picking.rs`

**Key change**: No subgrid needed. Ships live in the main heliocentric grid with world-space Position. Custom sync systems bridge Avian ↔ big_space.

1. **Add custom Avian sync systems** (see "Avian3D Integration" section above):
   ```rust
   // In physics.rs or tactical.rs
   app.add_systems(
       FixedPostUpdate,
       cell_transform_to_position
           .in_set(PhysicsTransformSystems::TransformToPosition),
   );
   app.add_systems(
       FixedPostUpdate,
       position_to_cell_transform
           .in_set(PhysicsTransformSystems::PositionToTransform),
   );
   ```

2. **TacticalArena is now just a marker** (not a Grid):
   ```rust
   commands.spawn((
       TacticalArena { body: body_entity, ... },
       // No Grid component - ships are in main grid
   ));
   ```

3. **VisualShips spawn in the main grid** with CellCoord + Position:
   ```rust
   let ship_helio_pos: DVec3 = body_helio_pos + DVec3::new(x_offset, y_offset, 0.0);
   let (ship_cell, ship_local) = grid.translation_to_grid(ship_helio_pos);

   // Spawn as child of BigSpace root, not arena
   root.spawn_spatial((
       VisualShip { ... },
       ship_cell,
       Transform::from_translation(ship_local),
       Position(ship_helio_pos),  // Avian physics component
       RigidBody::Dynamic,
       // ... other physics components
   ));
   ```

4. **Camera tracks arena** by updating its CellCoord:
   ```rust
   fn update_camera_for_tactical(
       arena: Query<&TacticalArena>,
       bodies: Query<&ComputedPosition, With<Body>>,
       mut camera: Query<(&mut CellCoord, &mut Transform), With<FloatingOrigin>>,
       grid: Query<&Grid, With<BigSpace>>,
   ) {
       // Move camera's CellCoord to match arena body position
       let body_pos = bodies.get(arena.body).unwrap().helio_pos;
       let (cell, local) = grid.translation_to_grid(body_pos + arena_offset);
       camera.cell = cell;
       camera.transform.translation = local;
   }
   ```

5. **Ship movement**: Avian updates Position (f64), our sync system writes CellCoord + Transform

6. **Tactical picking**: Use GlobalTransform (now precise because camera is nearby)

7. **Remove the 100,000x scaling hack** - real values work now:
   - Ship size: 100m
   - Ship spacing: 1km
   - Acceleration: 10 m/s² (1g)
   - Max speed: 50 km/s

**Validation**:
- Ships render at correct positions without jitter
- Ship movement works with real (non-scaled) values
- Can select ships, picking works
- Test at Mercury, Venus, AND Neptune distances

---

### Phase 6: Fleet Positions & Transfers

**Files**: `src/ship.rs`, `src/transfer_vis.rs`

1. Fleets get GridCell component:
   - AtBody: copy body's GridCell + offset
   - InTransit: compute from Kepler propagation

2. `update_fleet_positions`:
   ```rust
   fn update_fleet_positions(
       mut fleets: Query<(&FleetLocation, &mut GridCell, &mut Transform)>,
       bodies: Query<(&GridCell, &Transform), With<Body>>,
       grid: Query<&Grid>,
       sim_time: Res<SimulationTime>,
   ) {
       // ... compute heliocentric DVec3, convert to cell+local
   }
   ```

3. Transfer arcs:
   - Currently pre-baked as GizmoAsset with f32 points
   - Points are relative to departure position
   - Should work if departure position has correct GlobalTransform

**Validation**: Fleets render at bodies, fleets move along transfer arcs

---

## Critical Files Summary

| File | Changes |
|------|---------|
| `src/main.rs` | Plugin setup, BigSpace spawn, body position system, render_system |
| `src/camera.rs` | FloatingOrigin, animate_camera with CellCoord awareness |
| `src/physics.rs` | **NEW**: Custom Avian sync systems (cell_transform_to_position, position_to_cell_transform), PhysicsTransformConfig |
| `src/ship.rs` | Fleet CellCoord, position computation, all render functions |
| `src/tactical.rs` | Simplified arena (no subgrid), spawn ships in main grid, camera tracking, **remove 100,000x scaling hack** |
| `src/picking.rs` | Minimal changes (GlobalTransform still works) |
| `src/ui.rs` | Minimal changes (world_to_viewport still works) |
| `src/transfer_vis.rs` | Arc rendering, burn arrows |

---

## Potential Issues & Mitigations

### bevy_pancam compatibility
- PanCam modifies Transform directly
- With FloatingOrigin, this should still work (camera moves in local space)
- May need testing - if broken, implement custom pan logic

### bevy_vector_shapes compatibility
- ShapePainter.set_translation takes Vec3
- GlobalTransform.translation() returns Vec3 (camera-relative)
- Should work without changes

### Gizmo orbit rendering at extreme zoom
- Orbit GizmoAssets are f32 point clouds
- At tactical zoom, if parent body is imprecise, orbits may jitter
- Mitigation: orbits are only visible at strategic zoom where precision is fine

### bevy_pancam zoom + GridCell
- Zooming doesn't change position, only scale
- Should work fine

### Avian sync system ordering
- Our custom sync systems must run in the correct Avian system sets
- `cell_transform_to_position` in `PhysicsTransformSystems::TransformToPosition`
- `position_to_cell_transform` in `PhysicsTransformSystems::PositionToTransform`
- If ordering is wrong, physics may see stale positions or transforms may lag

### Avian + big_space recentering interaction
- big_space's `recenter_large_transforms` runs in PostUpdate on Changed<Transform>
- Our `position_to_cell_transform` also writes to Transform (and CellCoord)
- These should not conflict since we set both CellCoord and Transform atomically
- But verify that we don't trigger infinite change detection loops

### Multiple simultaneous battles
- With world-space Position, battles at different planets are naturally separated
- Ships at Mercury vs Neptune are millions of km apart in Position space
- No collision layer hacks needed
- Camera FloatingOrigin should only affect rendering, not physics

---

## Implementation Approach

**Phase by phase**: Each phase results in a runnable game. This makes debugging easier and allows stopping partway if needed.

- Phase 1 (Plugin + Camera): Game runs, camera works, but nothing renders correctly yet
- Phase 2 (Bodies): Bodies render at correct positions
- Phase 3 (Rendering): All visual systems work
- Phase 4 (Click/UI): Interaction works
- Phase 5 (Tactical + Avian): Custom Avian sync, tactical mode works, **remove scaling hack**
- Phase 6 (Fleets): Full game functionality restored

## Testing Strategy

1. **Per-phase validation**: Each phase has specific validation criteria listed
2. **Visual regression**: Compare before/after screenshots at various zoom levels
3. **Precision test**: After Phase 5, zoom to tactical scale at Neptune, verify no jitter on 100m ships
4. **Physics test**: After Phase 5, verify tactical combat still works with Avian3D
5. **Edge cases**:
   - Camera at solar system edge (Neptune+)
   - Rapid zoom in/out
   - Tactical mode entry/exit at various locations

---

## Future Considerations (not in this PR)

- Multi-SOI: Bodies become subgrids when entering their SOI
- Moon orbits: Currently flat, could be hierarchical
- Multiplayer: GridCell coordinates are absolute (good for sync)
