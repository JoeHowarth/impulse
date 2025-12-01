# Tactical Layer Physics Requirements

## Context

The tactical layer is a Newtonian combat simulation that runs when player and enemy fleets meet at a body. It operates at a different scale and timescale than the strategic (orbital mechanics) layer.

## Arena

- **Size**: 400,000 km × 400,000 km
- **Layout**: Body visible on right edge (outer edge 50,000 km into right side of screen)
- **Initial positions**: Opposing fleets spawn in center, 100,000 km apart
- **Boundary behavior**: Ship reaching edge = retreated from battle

## Timescale

- **Strategic layer**: ~864,000x realtime (10 days per real second base rate)
- **Tactical layer**: ~60x realtime
- **Transition**: Time slows dramatically when entering tactical mode

## Motion Model

### Ships
- Newtonian: position, velocity, acceleration
- Thrust-based movement (finite acceleration, not instant velocity changes)
- No gravity during tactical combat (simplification for now)
- Integration: `vel += accel * dt`, `pos += vel * dt`

### Projectiles
- **Missiles**: Self-propelled, can maneuver, track targets
- **Coilgun slugs**: Ballistic (no thrust after launch), very high speed
- **PD rounds**: Short range defensive, possibly hitscan

## Speed Regime

This is NOT a typical game physics scenario. Objects are:
- Very small (ships ~100m, missiles smaller)
- Very fast (km/s range)
- In a large arena (400,000 km)

Example: A 10 km/s projectile at 60x time, 60fps:
- Moves 10 km/frame in sim-space
- That's 100x a ship's length per frame

## Collision Detection Challenges

Traditional game engine collision detection assumes:
- Objects are large relative to their per-frame movement
- Overlap tests at discrete frames catch collisions

Our scenario breaks this:
- Objects move many body-lengths per frame
- "Tunneling" - objects pass through each other between frames

### Potential Approaches

1. **Swept/Continuous Collision Detection (CCD)**
   - Treat fast objects as line segments (frame start → frame end)
   - Test segment-to-segment or segment-to-circle intersections
   - More expensive but catches tunneling

2. **Raycast per frame**
   - For projectiles: cast ray along velocity vector
   - Simpler than full CCD, good for point-like projectiles

3. **Physics substeps**
   - Run collision checks at higher rate than rendering
   - E.g., 10 substeps per frame
   - Increases CPU cost linearly

4. **Hybrid approach**
   - Ships (slower): standard collision detection
   - Projectiles (fast): raycast or swept

## Spatial Partitioning

With many objects, O(n²) collision checks become expensive.

Options:
- **Grid**: Divide arena into cells, only check objects in same/adjacent cells
- **Quadtree**: Adaptive subdivision based on object density
- **Brute force**: Fine for small object counts (<100), revisit if needed

For initial implementation, brute force is likely fine. Add spatial partitioning when/if performance requires it.

## Targeting & Aiming

At these speeds, hitting a moving target requires leading:
- Predict target position at time of impact
- Account for projectile travel time
- Missiles can course-correct; slugs cannot

## Recommended Approach

### Physics Engine: Avian 0.4

Use `avian2d` with Bevy 0.17:

```toml
[dependencies]
avian2d = { version = "0.4", features = ["f64"] }
```

**Why Avian:**
- Native Bevy ECS design (not a wrapper around another engine)
- Built-in Swept CCD for high-speed collision detection
- Per-entity CCD configuration
- Bevy 0.17 compatible
- 2D-specific crate available

### Configuration

**Precision**: f64
- Provides ~15-16 significant digits
- Eliminates precision concerns at our scale
- Worth the minor performance cost for correctness

**Scale**: 1 unit = 1 meter
- Arena: 4×10⁸ units (f64 handles this easily)
- Ships: ~100 units (reasonable collision shape size)
- Projectiles: not tiny fractions
- Most intuitive to reason about

**CCD Strategy**: Linear CCD on everything initially
- Start simple, optimize later if needed
- `SweptCcd::linear()` on all physics entities
- Can selectively disable on slow-moving objects if performance requires

**Gravity**: Disabled globally
- Tactical combat ignores gravity (ship thrust dominates)

### Initial Implementation

```rust
// Example entity setup
commands.spawn((
    Ship { ... },
    RigidBody::Dynamic,
    Collider::circle(50.0),  // 50m radius
    SweptCcd::linear(),
    // ... other components
));
```

### Optimization Path

1. Start with CCD on everything, brute force collision
2. Profile to find actual bottlenecks
3. If needed: disable CCD on slow ships, add spatial partitioning
4. Only add complexity when measurements demand it

## Open Questions

- [ ] Exact ship acceleration/thrust values?
- [ ] Projectile speeds for each type?
- [ ] Missile tracking behavior (proportional navigation? pure pursuit?)
- [ ] PD range and effectiveness model?
- [ ] Do ships have rotation/facing, or instant orientation?
- [ ] Damage model details (all binary kills for now?)

