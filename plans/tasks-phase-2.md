# Phase 2: Tactical Foundation - Implementation Tasks

## Architecture

### Entity Hierarchy

```
Fleet (persistent)
├── LogicalShip (persistent, stats/identity)
├── LogicalShip
└── ...

TacticalArena (exists during combat only)
├── Transform (heliocentric position, updated if moving)
├── VisualShip (arena-local Transform, physics body)
│   └── references LogicalShip
├── VisualShip
├── Missile
└── ...
```

**LogicalShip**: Persistent entity, child of Fleet. Tracks existence, identity, stats (ammo, damage in future). Survives across battles. No physics or rendering components.

**VisualShip**: Tactical-only entity, child of TacticalArena. Has Transform (arena-local), RigidBody, Collider, visual components. Spawned when entering tactical, despawns with arena. References its LogicalShip.

**TacticalArena**: Reference frame for combat. 400,000 km × 400,000 km arena (4×10^8 m × 4×10^8 m). Position/velocity in heliocentric coordinates. For body battles: near-stationary. For transit battles: follows orbital trajectory (ships maneuver relative to moving frame). Ships crossing the boundary (200,000 km from center) have retreated.

### Coordinate Systems

- **Strategic layer**: Heliocentric meters (body positions ~10^11 m from Sun)
- **Tactical layer**: Arena-local meters (ship positions ~10^8 m max from arena center)
- Arena entity bridges the two (its Transform is heliocentric)
- Bevy computes global transforms automatically via parent-child hierarchy
- f64 precision handles the scale (15-16 significant digits)

### Physics

See `plans/tactical-physics.md` for full details.

- Avian 3D with f64 precision
- 1 unit = 1 meter
- Linear CCD on all entities (handles high-speed collisions)
- Gravity disabled (ship thrust dominates in tactical)

---

## Step 2.1: LogicalShip Entities

### Goal
Ships exist as individual entities (children of Fleet), replacing the simple `ship_count` field.

### Changes Required

**1. ship.rs - New LogicalShip component**
```rust
#[derive(Component)]
pub struct LogicalShip {
    // Future: stats, ammo, damage state
}
```

**2. ship.rs or main.rs - Fleet spawning**
- When spawning a Fleet, also spawn N LogicalShip children
- Remove or deprecate `ship_count` field (derive from children count)

**3. Helper function**
```rust
fn ship_count(fleet: Entity, children: &Query<&Children>, ships: &Query<&LogicalShip>) -> u32 {
    // Count LogicalShip children of fleet
}
```

**4. Update existing code**
- Anywhere using `fleet.ship_count` → use helper or query children

### Verification
- [x] LogicalShip component exists
- [x] Fleets spawn with correct number of LogicalShip children
- [x] `ship_count` derived from children works
- [x] Existing fleet operations (split, merge) work with new model
- [x] Objectives system counts ships correctly

**Completed.** Also removed `ship_count` field entirely - now always derived via `ship_count()` helper. Added `ComputedFleetPosition` component for cleaner rendering (mirrors `ComputedBody` pattern).

---

## Step 2.2: Factions

### Goal
Distinguish player-controlled vs enemy-controlled fleets.

### Changes Required

**1. ship.rs - Faction component**
```rust
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum Faction {
    Player,
    Enemy,
}
```

**2. Fleet spawning**
- Player fleets get `Faction::Player`
- Enemy fleets get `Faction::Enemy`

**3. Consider removing `PlayerControlled` marker**
- Or keep both if useful for queries

### Verification
- [x] Faction component exists
- [x] Player fleets have Faction::Player
- [x] Can query fleets by faction

**Completed.** Removed `PlayerControlled` marker entirely - `Faction` is now the single source of truth. Also renamed `ShipLocation` → `FleetLocation` and `render_ship` → `render_fleets` for consistency.

---

## Step 2.3: Enemy Garrisons

### Goal
Spawn static enemy fleets at certain bodies.

### Changes Required

**1. main.rs or new module - Enemy fleet spawning**
- Pick 2-3 bodies for enemy garrisons (e.g., Venus, Ceres, Titan)
- Spawn enemy Fleet with Faction::Enemy
- Spawn LogicalShip children
- Set ShipLocation::AtBody

**2. Visual distinction (optional for now)**
- Different color for enemy fleet markers
- Or defer to Step 2.6

### Verification
- [x] Enemy fleets spawn at designated bodies
- [x] Enemy fleets have correct faction
- [x] Enemy fleets have LogicalShip children
- [x] Enemy fleets visible on map (even if same color for now)

**Completed.** Faction-based colors: green for player, imperial red for enemy. Victory condition changed to "destroy all enemy fleets" with red rings around enemy-occupied bodies.

*Note: For f32 precision testing, spawn locations temporarily changed to Venus (player, 10 ships) and Mercury (enemy, 10 ships). Original plan was Saturn/Ceres/Mars/Earth.*

---

## Step 2.4: Combat Trigger

### Goal
Detect when player fleet arrives at body with enemy fleet, trigger tactical mode.

### Changes Required

**1. New system - `detect_combat`**
```rust
fn detect_combat(
    fleets: Query<(Entity, &ShipLocation, &Faction)>,
    // ...
) {
    // For each player fleet that just arrived at a body:
    //   Check if any enemy fleet at same body
    //   If yes: trigger tactical mode
}
```

**2. Combat state resource**
```rust
#[derive(Resource, Default)]
pub struct CombatState {
    pub active: bool,
    pub arena: Option<Entity>,
    pub body: Option<Entity>,
    pub player_fleets: Vec<Entity>,
    pub enemy_fleets: Vec<Entity>,
}
```

**3. Hook into arrival**
- After `check_arrival` transitions fleet to AtBody
- Run combat detection

### Edge Cases
- Multiple player fleets arrive at same enemy body?
  - For now: trigger combat with all present
- Player fleet already at body when enemy arrives?
  - Enemies are static garrisons, so N/A for Phase 2

### Verification
- [x] Combat detected when player fleet arrives at enemy body
- [x] CombatState populated correctly
- [x] No false triggers (arriving at friendly body)

**Completed.** CombatState resource tracks combat. detect_combat system runs after check_arrival and triggers when player/enemy fleets share a body.

---

## Step 2.5: Tactical Mode Entry

### Goal
Transition into tactical combat: spawn arena, spawn VisualShips, setup camera and time.

### Changes Required

**1. New module - `tactical.rs`**

**2. TacticalArena component**
```rust
#[derive(Component)]
pub struct TacticalArena {
    pub body: Entity,  // The body this battle is at
    // Future: velocity for moving battles
}
```

**3. VisualShip component**
```rust
#[derive(Component)]
pub struct VisualShip {
    pub logical: Entity,  // Reference to LogicalShip
    pub faction: Faction,
}
```

**4. System - `enter_tactical_mode`**
- Spawn TacticalArena entity at body's heliocentric position
- For each LogicalShip in involved fleets:
  - Spawn VisualShip as child of arena
  - Position: player ships on left, enemy on right, 100,000 km apart
  - Initial velocity: zero (relative to arena)
  - Add physics components (RigidBody, Collider)
  - Link to LogicalShip

**5. Camera setup**
- Zoom to 400,000 km × 400,000 km view
- Position: body on right edge (outer edge 50,000 km into screen)
- Center on arena

**6. Time adjustment**
- Set SimulationTime to tactical speed (~60x realtime)
- Store previous time_scale for restoration

### Arena Layout
```
+------------------+--------+
|                  |        |
|   [P] 100km [E]  |  BODY  |
|                  |        |
+------------------+--------+
     center          right
```

### Verification
- [x] TacticalArena entity spawns at correct position
- [x] VisualShips spawn for all involved LogicalShips
- [x] VisualShips are children of arena
- [x] Initial positions correct (player bottom, enemy top, 100km apart)
- [x] Camera zooms and positions correctly
- [x] Time slows to tactical speed (60x realtime = 1 min/s)

**Completed.** tactical.rs module with TacticalArena and VisualShip components. Arena spawns offset from body so body appears on right side. Camera animates to tactical view and tracks arena movement (compensates for orbital motion). Time scale refactored to direct seconds (removed SIM_BASE_RATE multiplication). Added CameraScale resource to centralize camera scale queries for all rendering systems.

---

## Step 2.6: VisualShip Rendering

### Goal
Draw individual ships in tactical view.

### Changes Required

**1. Rendering for VisualShip**
- Add mesh/sprite to VisualShip on spawn
- Or use gizmo-based rendering like strategic layer

**2. Visual distinction**
- Player ships: one color (blue?)
- Enemy ships: another color (red?)

**3. Scale**
- Ships are ~100m, arena is 400,000 km
- Need appropriate visual size (not to scale - would be invisible)
- Maybe 1-2 km visual radius for visibility

### Verification
- [x] VisualShips render in tactical view
- [x] Player vs enemy visually distinct
- [x] Ships visible at tactical zoom level

**Completed.** Ships render as triangles using ShapePainter with LOD system. Green for player, red for enemy. White ring indicator on selected ships.

---

## Step 2.7: Ship Selection

### Goal
Select individual VisualShips for giving orders.

### Changes Required

**1. Selection component**
```rust
#[derive(Component)]
pub struct Selected;  // Reuse existing? Or new TacticalSelected?
```

**2. Click selection**
- Click on VisualShip → select it (deselect others)
- Shift+click → add to selection

**3. Box selection**
- Click+drag → draw rectangle
- On release → select all VisualShips in rectangle

**4. Visual feedback**
- Selected ships have highlight/outline

**5. Only in tactical mode**
- This selection system only active when CombatState.active

### Verification
- [x] Click selects single ship
- [x] Shift+click adds to selection
- [x] Box select works
- [x] Selected ships visually highlighted
- [x] Selection only works in tactical mode

**Completed.** New `picking.rs` module with unified selection system. Reuses `Selected` component from ship.rs. Click, shift+click, and box selection all work. White ring indicator on selected ships. Also fixed camera tracking: apply arena delta to both camera position AND target during animation, snap to exact target when animation ends. Changed PanCam to right-mouse-only for panning.

---

## Step 2.8a: Avian Setup

### Goal
Integrate Avian 3D physics engine with f64 precision.

### Changes Required

**1. Cargo.toml**
```toml
[dependencies]
avian3d = { version = "0.3", features = ["f64"] }
```

**2. main.rs - Plugin setup**
```rust
use avian3d::prelude::*;

app.add_plugins(PhysicsPlugins::default())
   .insert_resource(Gravity(DVec3::ZERO));
```

**3. VisualShip physics components**
- RigidBody::Dynamic
- Collider (sphere, 50m radius)
- LinearVelocity, SweptCcd
- SleepingDisabled (ships always active)

### Verification
- [x] Avian3D compiles and runs
- [x] Gravity disabled
- [x] f64 precision enabled
- [x] VisualShips spawn with physics components

**Completed.** Avian3D integrated with f64 precision. Ships have RigidBody, Collider, LinearVelocity, SweptCcd.

---

## Step 2.8b: Movement Orders

### Goal
Right-click to set destination, ships move toward it.

### Changes Required

**1. MoveOrder component**
```rust
#[derive(Component)]
pub struct MoveOrder {
    pub destination: DVec3,  // Arena-local coordinates (f64)
}
```

**2. Right-click handler (picking.rs)**
- `handle_tactical_move_order` system
- Convert screen position to arena-local coordinates via `screen_to_arena_local`
- Insert MoveOrder on all selected ships

**3. Destination markers**
- `render_move_markers` draws X at destination for selected ships with orders

### Verification
- [x] Right-click sets destination
- [x] Selected ships get MoveOrder
- [x] X marker appears at destination
- [x] Unselected ships don't get orders

**Completed.** Right-click movement orders working. X markers rendered at destinations.

---

## Step 2.8c: Thrust/Acceleration Model

### Goal
Realistic Newtonian movement with finite acceleration.

### Changes Required

**1. ShipStats component**
```rust
#[derive(Component)]
pub struct ShipStats {
    pub max_acceleration: f64,  // 10 m/s² (1g)
    pub max_speed: f64,         // 50 km/s
}
```

**2. Movement system - `update_ship_movement`**
- Compute distance to destination
- Calculate stopping distance: `d = v²/(2a)`
- If distance > stopping_distance: accelerate toward target
- Else: decelerate (brake opposite to velocity)
- Clear MoveOrder when arrived (within 1km and <10 m/s)

**3. Velocity clamping**
- Clamp to max_speed (50 km/s)

### Verification
- [x] Ships accelerate gradually
- [x] Ships decelerate using stopping distance formula
- [x] Ships stop at destination
- [x] Movement feels Newtonian

**Completed.** Newtonian thrust model implemented. Ships accelerate/decelerate realistically using stopping distance formula.

*Note: Currently using 100,000x scaled values (see Step 2.8e workaround). Real values will be restored after big_space integration.*

---

## Step 2.8d: Ship Rendering Improvements

### Goal
Realistic ship sizing with LOD system.

### Changes Required

**1. Ship spacing and size**
- Physical ship size: 100m (realistic)
- Ship spacing: 1km between ships in formation

**2. LOD system (inspired by bodies)**
- Log-based scaling: ships stay visible at all zoom levels
- `compute_ship_display()` returns (display_size, visibility)
- Minimum size is physical size
- Fade when screen size < 2 pixels

**3. Zoom scale indicator**
- Added to time panel: "100px = X km/AU/m"
- `ZoomScaleText` component and `format_distance` helper

### Verification
- [x] Ships render with LOD scaling
- [x] Ships visible at all zoom levels (until fade threshold)
- [x] Zoom scale indicator shows in UI

**Completed.** Ship rendering improved with LOD system.

*Note: Currently using 100,000x scaled values (10,000 km ships, 100,000 km spacing) as f32 precision workaround. Real values will be restored after big_space integration.*

---

## Step 2.8e: F32 Precision Issue

### Problem Identified
Ship movement and rendering fail at planetary distances due to f32 precision limits:
- Avian3D uses f64 internally but syncs Position → Transform (f32)
- At Mercury (~50B meters): f32 can only represent changes of ~5,000m
- At Venus (~100B meters): precision drops to ~10,000m

**Symptoms observed:**
- Transform.x stays constant while Avian Position.x changes correctly
- Small velocity components get "eaten" by f32 precision loss
- Ships appear to move only in the dominant direction

### Temporary Workaround (Active)
Scale all tactical values 100,000x larger so movements exceed f32 precision threshold:

| Parameter | Real Value | Test Value |
|-----------|------------|------------|
| Ship size | 100m | 10,000 km |
| Ship spacing | 1km | 100,000 km |
| Acceleration | 10 m/s² (1g) | 1,000,000 m/s² |
| Max speed | 50 km/s | 5,000,000 km/s |
| Arrival distance | 1km | 10,000 km |
| Arrival velocity | 10 m/s | 100 km/s |

This workaround is documented in `src/tactical.rs` with a comment block explaining the issue.

### Permanent Solution
Integrate big_space 0.11 for camera-relative GlobalTransforms. See `plans/big_space_migration.md` for full implementation plan.

### Verification (deferred to big_space integration)
- [ ] Ships render at all zoom levels at any distance from Sun
- [ ] No jitter at tactical zoom
- [ ] Neptune-distance battles work with 100m ships

**Status:** Working with temporary 100,000x scale workaround. Permanent fix requires big_space integration.

---

## Step 2.9: Basic Missiles

### Goal
Press 1 to fire missile at target, missiles track and kill on impact.

### Changes Required

**1. Missile component**
```rust
#[derive(Component)]
pub struct Missile {
    pub target: Entity,  // VisualShip being targeted
    pub owner: Faction,
}
```

**2. Targeting**
- Need a way to designate target (click on enemy while ships selected?)
- Store current target on selected ships or globally

**3. Fire command**
- Press 1 with ships selected and target designated
- Check range (50,000 km)
- Spawn Missile entity as child of arena
- Initial position: at firing ship
- Missile has: RigidBody, Collider, SweptCcd

**4. Missile guidance**
- System: missiles accelerate toward target
- Simple pursuit (point at target) or proportional navigation (later)

**5. Collision = kill**
- Avian collision event between Missile and VisualShip
- Despawn both missile and target VisualShip
- Despawn target's LogicalShip

### Verification
- [ ] Can designate target
- [ ] Press 1 fires missile (if in range)
- [ ] Missile tracks toward target
- [ ] Collision destroys target
- [ ] LogicalShip also despawned

---

## Step 2.10: Retreat + Win/Lose + Tactical Exit

### Goal
End conditions for tactical combat.

### Changes Required

**1. Retreat detection**
- System checks VisualShip positions against arena boundary (200,000 km from center)
- Ship crosses boundary → retreated
- Despawn VisualShip
- LogicalShip survives but needs new destination (or mark as "retreated")

**2. Win condition**
- All enemy VisualShips destroyed or retreated → player wins

**3. Lose condition**
- All player VisualShips destroyed or retreated → player loses

**4. Tactical exit - `exit_tactical_mode`**
- Despawn TacticalArena (cascades to all VisualShips, missiles)
- Reset camera to strategic view
- Restore time scale
- Clear CombatState

**5. Post-battle**
- If player won: enemy fleet destroyed (LogicalShips already gone)
- If player lost: player fleet destroyed
- Retreated ships: must immediately pick new destination or be destroyed
  - For now: simplify - retreated ships return to nearest friendly body?

### Verification
- [ ] Ships crossing boundary are removed from battle
- [ ] Victory when all enemies gone
- [ ] Defeat when all player ships gone
- [ ] Tactical mode exits cleanly
- [ ] Camera and time restored
- [ ] Surviving ships persist correctly

---

## Phase 2 Progress Summary

### Completed
- ✅ 2.1: LogicalShip entities
- ✅ 2.2: Factions
- ✅ 2.3: Enemy garrisons
- ✅ 2.4: Combat trigger
- ✅ 2.5: Tactical mode entry
- ✅ 2.6: VisualShip rendering
- ✅ 2.7: Ship selection
- ✅ 2.8a: Avian physics setup
- ✅ 2.8b: Movement orders
- ✅ 2.8c: Thrust/acceleration model
- ✅ 2.8d: Ship rendering improvements

### Working (with workaround)
- ⚠️ 2.8e: F32 precision - working via 100,000x scale, needs big_space for proper fix

### Not Started
- ⬜ 2.9: Basic missiles
- ⬜ 2.10: Retreat + win/lose + tactical exit

---

## Phase 2 Complete (Target)

Playable tactical combat:
- Fly to enemy body, trigger combat
- Tactical arena with individual ships
- Select ships, give movement orders
- Fire missiles to destroy enemies
- Win by destroying all enemies
