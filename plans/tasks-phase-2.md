# Phase 2: Tactical Foundation - Implementation Tasks

## Architecture

### Entity Hierarchy

```
Fleet (persistent)
├── LogicalShip (persistent, stats/identity)
└── ...

TacticalArena (exists during combat only, child of Body via GridNode)
├── VisualShip (arena-local Transform, physics body)
│   └── references LogicalShip
├── VisualShip
├── Missile
└── ...
```

**LogicalShip**: Persistent entity, child of Fleet. Tracks existence, identity, stats. Survives across battles.

**VisualShip**: Tactical-only entity, child of TacticalArena. Has physics (RigidBody, Collider, LinearVelocity), arena-local position. Despawns with arena.

**TacticalArena**: Reference frame for combat. 400km × 400km arena. Spawned as GridNode child of Body, inherits orbital motion automatically.

### Coordinate Systems

Uses big_space for f64 precision at all scales:
- **Strategic layer**: Bodies are GridNodes with CellCoord positioning
- **Tactical layer**: Arena is GridNode child of Body, ships are GridLeaf children of arena
- Ships use DVec3 arena-local coordinates (meters)
- Precision preserved via nested grid hierarchy

### Physics

- Avian 3D with f64 precision
- Custom big_space sync in `physics.rs` (bypasses default Transform sync)
- 1 unit = 1 meter
- Gravity disabled

---

## Completed Steps

### 2.1: LogicalShip Entities ✅
Ships exist as individual entities (children of Fleet). `ship_count` derived via helper function.

### 2.2: Factions ✅
`Faction::Player` and `Faction::Enemy`. Single source of truth (removed `PlayerControlled` marker).

### 2.3: Enemy Garrisons ✅
Static enemy fleets spawn at designated bodies. Faction-based colors: green (player), red (enemy).

### 2.4: Combat Trigger ✅
`detect_combat` system triggers when player/enemy fleets share a body. Populates `CombatState` resource.

### 2.5: Tactical Mode Entry ✅
- TacticalArena spawns as GridNode child of Body
- VisualShips spawn as GridLeaf children with physics components
- Camera animates to tactical view, tracks arena movement
- Time scale set to 60x (TACTICAL_TIME_SCALE)

### 2.6: VisualShip Rendering ✅
Ships render as triangles with LOD system. Green for player, red for enemy.

### 2.7: Ship Selection ✅
- Click selects single ship
- Shift+click adds to selection
- Box select (drag rectangle)
- White ring indicator on selected ships

### 2.8a: Avian Physics Setup ✅
Avian3D integrated with f64 precision. Ships have RigidBody, Collider, LinearVelocity, SweptCcd.

### 2.8b: Movement Orders ✅
Right-click sets destination. X markers rendered at destinations.

### 2.8c: Thrust/Acceleration Model ✅
Newtonian movement with stopping distance formula. Ships accelerate/decelerate realistically.

### 2.8d: Ship Rendering Improvements ✅
LOD system keeps ships visible at all zoom levels. Zoom scale indicator in UI.

### 2.8e: F32 Precision ✅
**Solved via big_space integration.** Custom sync in `physics.rs` preserves f64 precision:
- `big_space_transform_to_position`: CellCoord+Transform → Position (FixedPreUpdate)
- `position_to_big_space_transform`: Position → CellCoord+Transform (FixedPostUpdate)
- Test verifies precision at Neptune scale (~4.5e12 m)

### 2.10: Retreat + Win/Lose + Tactical Exit ✅
- **Bounds checking**: Ships crossing 200km boundary despawn
- **Victory**: All enemy VisualShips destroyed → combat ends
- **Defeat**: All player VisualShips destroyed → combat ends
- **Exit dialog**: ESC shows retreat confirmation
- **Cleanup**: `teardown_tactical_arena` despawns arena (cascades to children), `cleanup_empty_fleets` removes depleted fleets
- **Restoration**: Camera position/scale and time scale restored on exit

---

## Step 2.9: Basic Missiles (NOT STARTED)

### Goal
Fire missiles at enemies, missiles track and kill on impact.

### Targeting System
Need a way to designate attack target. Options:
- Click enemy while ships selected → set as target
- Right-click enemy = attack (vs right-click empty = move)
- Attack-move command (A + click)

**Decision needed**: Which UX pattern?

### Missile Component
```rust
#[derive(Component)]
pub struct Missile {
    pub target: Entity,  // VisualShip being targeted
    pub owner: Faction,
}
```

### Fire Command
- Key binding (1? or automatic on right-click enemy?)
- Check range (50km? configurable?)
- Spawn Missile as GridLeaf child of arena
- Initial position: at firing ship
- Physics: RigidBody, Collider, SweptCcd

### Missile Guidance
- Simple pursuit: accelerate toward current target position
- (Future: proportional navigation for smarter intercepts)

### Collision = Kill
- Avian collision event between Missile and VisualShip
- Despawn missile
- Despawn target VisualShip
- Despawn target's LogicalShip (permanent death)

### Verification
- [ ] Can designate target (targeting UX implemented)
- [ ] Fire command spawns missile
- [ ] Missile tracks toward target
- [ ] Collision destroys both missile and target
- [ ] LogicalShip despawned on ship death

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
- ✅ 2.8a-e: Physics, movement, rendering, precision
- ✅ 2.10: Retreat + win/lose + tactical exit

### Remaining
- ⬜ 2.9: Basic missiles (targeting + fire + guidance + collision)

---

## Phase 2 Complete (Target)

Playable tactical combat:
- ✅ Fly to enemy body, trigger combat
- ✅ Tactical arena with individual ships
- ✅ Select ships, give movement orders
- ⬜ Fire missiles to destroy enemies
- ✅ Win by destroying all enemies
