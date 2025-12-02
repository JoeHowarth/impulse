# Big Space Integration Plan for Impulse

## Overview

Integrate big_space 0.11 to solve f32 precision limitations when rendering at solar system scale while zooming to tactical scale (100m ships, 0.1m projectiles).

**Core insight**: The precision problem is *only* in the rendering pipeline. Orbital mechanics (f64) and Avian3D physics (f64) are fine. We need big_space to compute camera-relative GlobalTransforms so nearby objects have precise f32 coordinates regardless of their absolute heliocentric position.

## Architecture

```
f64 orbital mechanics ──┐
                        ├──► Grid::translation_to_grid() ──► (GridCell, Transform) ──► GlobalTransform (camera-relative)
Avian3D f64 physics ────┘
```

### Grid Structure

```
BigSpace (root)
└── Grid<i64> (heliocentric, cell_size ~1e9m)
    ├── Sun (GridCell + Transform)
    ├── Earth (GridCell + Transform)
    ├── Mars (GridCell + Transform)
    ├── Fleet entities (GridCell + Transform)
    ├── ... all bodies flat
    │
    └── TacticalArena (GridCell + Transform + Grid<i64>)  ← subgrid
        ├── VisualShip (GridCell + Transform)  ← NOT just local Transform
        ├── VisualShip (GridCell + Transform)
        └── Projectile (GridCell + Transform)
```

- **Flat grid for bodies**: All celestial bodies in one heliocentric grid (no nesting)
- **Subgrid for TacticalArena**: Ships/projectiles get GridCell + Transform (not just local Transform) because GlobalTransform propagation loses precision NOW - we need GridCell precision for the subgrid too
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

**Files**: `src/main.rs`, `src/camera.rs`

1. Disable Bevy's TransformPlugin, add BigSpaceDefaultPlugins:
   ```rust
   DefaultPlugins.build().disable::<TransformPlugin>(),
   BigSpaceDefaultPlugins,
   ```

2. Create BigSpace root in setup, spawn camera with FloatingOrigin:
   ```rust
   commands.spawn_big_space_default(|root| {
       root.spawn_spatial((
           Camera3d::default(),
           FloatingOrigin,
           Projection::from(OrthographicProjection { ... }),
           PanCam { ... },
           CameraTarget::default(),
       ));
   });
   ```

3. Update `animate_camera` to work with GridCell-aware transforms (camera moves between cells as it pans across solar system)

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

### Phase 5: Tactical Mode with Subgrid

**Files**: `src/tactical.rs`, `src/picking.rs`

1. TacticalArena becomes a Grid:
   ```rust
   let (arena_cell, arena_local) = root_grid.translation_to_grid(body_helio_pos + offset);
   commands.spawn((
       TacticalArena { ... },
       Grid::<i64>::default(),  // subgrid for tactical entities
       arena_cell,
       Transform::from_translation(arena_local),
   ));
   ```

2. VisualShips spawn with GridCell + Transform in arena's grid:
   ```rust
   // Ships need GridCell because GlobalTransform propagation loses precision
   let ship_pos_arena_local = DVec3::new(x_offset, y_offset, 0.0);
   let (ship_cell, ship_local) = arena_grid.translation_to_grid(ship_pos_arena_local);

   commands.entity(arena).with_children(|builder| {
       builder.spawn((
           VisualShip { ... },
           ship_cell,
           Transform::from_translation(ship_local),
       ));
   });
   ```

3. `update_arena_position`:
   - Recompute arena's GridCell + Transform from body position each frame
   - Camera tracking: update camera's GridCell to match arena's cell
   - No delta-based translation - set absolute position from orbital mechanics

4. Ship movement (when Avian physics moves ships):
   - Read Position from Avian (DVec3)
   - Convert to GridCell + Transform via arena_grid.translation_to_grid()
   - GlobalTransform will be precise because it's relative to FloatingOrigin

5. Tactical picking:
   - `screen_to_arena_local` converts screen ray to arena-local DVec3
   - Compare against ship positions using high-precision ComputedPosition

6. Projectiles (future):
   - Spawn with GridCell + Transform in arena grid
   - 0.1m precision works because local Transform values are small

**Validation**: Can enter tactical, ships render at correct positions, can select ships, picking works, ships can move without jitter

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
| `src/camera.rs` | FloatingOrigin, animate_camera with GridCell awareness |
| `src/ship.rs` | Fleet GridCell, position computation, all render functions |
| `src/tactical.rs` | Arena as subgrid, VisualShip local transforms, arena tracking |
| `src/picking.rs` | Likely minimal changes (GlobalTransform still works) |
| `src/ui.rs` | Likely minimal changes (world_to_viewport still works) |
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

---

## Implementation Approach

**Phase by phase**: Each phase results in a runnable game. This makes debugging easier and allows stopping partway if needed.

- Phase 1 (Plugin + Camera): Game runs, camera works, but nothing renders correctly yet
- Phase 2 (Bodies): Bodies render at correct positions
- Phase 3 (Rendering): All visual systems work
- Phase 4 (Click/UI): Interaction works
- Phase 5 (Tactical): Tactical mode works with subgrid
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
