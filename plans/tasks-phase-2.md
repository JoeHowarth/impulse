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

- Avian 2D with f64 precision
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
- [ ] Enemy fleets spawn at designated bodies
- [ ] Enemy fleets have correct faction
- [ ] Enemy fleets have LogicalShip children
- [ ] Enemy fleets visible on map (even if same color for now)

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
- [ ] Combat detected when player fleet arrives at enemy body
- [ ] CombatState populated correctly
- [ ] No false triggers (arriving at friendly body)

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
- [ ] TacticalArena entity spawns at correct position
- [ ] VisualShips spawn for all involved LogicalShips
- [ ] VisualShips are children of arena
- [ ] Initial positions correct (100km apart, center of arena)
- [ ] Camera zooms and positions correctly
- [ ] Time slows to tactical speed

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
- [ ] VisualShips render in tactical view
- [ ] Player vs enemy visually distinct
- [ ] Ships visible at tactical zoom level

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
- [ ] Click selects single ship
- [ ] Shift+click adds to selection
- [ ] Box select works
- [ ] Selected ships visually highlighted
- [ ] Selection only works in tactical mode

---

## Step 2.8a: Avian Setup

### Goal
Integrate Avian 2D physics engine.

### Changes Required

**1. Cargo.toml**
```toml
[dependencies]
avian2d = { version = "0.4", features = ["f64"] }
```

**2. main.rs - Plugin setup**
```rust
use avian2d::prelude::*;

app.add_plugins(PhysicsPlugins::default())
   .insert_resource(Gravity(Vec2::ZERO));
```

**3. Test with large coordinates**
- Spawn test bodies at ~10^11 meter positions
- Verify collisions still work
- Remove test after confirming

### Verification
- [ ] Avian compiles and runs
- [ ] Gravity disabled
- [ ] f64 precision enabled
- [ ] Large coordinate test passes

---

## Step 2.8b: Movement Orders

### Goal
Right-click to set destination, ships move toward it.

### Changes Required

**1. Destination component**
```rust
#[derive(Component)]
pub struct MoveOrder {
    pub destination: Vec2,  // Arena-local coordinates
}
```

**2. Right-click handler**
- In tactical mode: right-click sets MoveOrder on selected ships
- Convert screen position to arena-local coordinates

**3. Basic movement system**
- Ships with MoveOrder accelerate toward destination
- For now: simple "set velocity toward target" (refine in 2.8c)

### Verification
- [ ] Right-click sets destination
- [ ] Selected ships move toward destination
- [ ] Unselected ships don't move

---

## Step 2.8c: Thrust/Acceleration Model

### Goal
Realistic Newtonian movement with finite acceleration.

### Changes Required

**1. Ship stats**
```rust
#[derive(Component)]
pub struct ShipStats {
    pub max_acceleration: f64,  // m/s²
}
```

**2. Movement system (refined)**
```rust
fn ship_movement(
    mut ships: Query<(&MoveOrder, &ShipStats, &mut LinearVelocity, &Transform)>,
) {
    // Calculate desired direction
    // Apply acceleration (clamped to max)
    // Handle arrival (clear MoveOrder when close + slow)
}
```

**3. Deceleration**
- Ships need to slow down to stop at destination
- Simple approach: start braking at halfway point
- Or: always accelerate toward destination, overshoot and correct

**4. Velocity clamping (optional)**
- Max speed limit? Or let physics handle it?

### Verification
- [ ] Ships accelerate gradually (not instant velocity)
- [ ] Ships decelerate and stop at destination
- [ ] Movement feels Newtonian

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

## Phase 2 Complete

Playable tactical combat:
- Fly to enemy body, trigger combat
- Tactical arena with individual ships
- Select ships, give movement orders
- Fire missiles to destroy enemies
- Win by destroying all enemies
