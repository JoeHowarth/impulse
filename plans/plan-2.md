# Impulse Development Plan

## Current State (Phase 1 Complete)
- Multiple fleets with ship counts and delta-v budgets
- Lambert transfers via precomputed LUT (~1.4M solutions)
- Multi-leg flight plans with commit/cancel
- Fleet split (S) and merge (M) operations
- Objectives system (required ships at bodies)
- Victory detection when all objectives met
- Number keys (1-9) to select fleets, double-tap to pan camera
- Transfer popups showing source → destination
- Time controls, polished visuals

---

## Phase 1: Fleet Management

### 1.1 Multi-leg / Cross-system Transfers
Extend current transfer system to support:
- Sequential transfers (A → B → C)
- Transfers across different parent bodies (e.g., Titan → Ganymede)
- Heliocentric transfers that escape one sphere of influence and enter another

### 1.2 Fleets as Basic Unit
- Fleet is the selectable/commandable unit (1-100 ships)
- Click to select fleet
- Plan transfers for selected fleet
- Spawn with multiple fleets at game start

### 1.3 Merge / Split Operations
- Select fleet → split into two fleets (specify ship count)
- Select two fleets at same location → merge into one
- UI for these operations (context menu? buttons?)

### 1.4 Objective System
- Certain bodies require X ships to arrive
- Different requirements per body
- Requires planning: can't just send one fleet to each
- Forces use of multi-leg transfers and merge/split

**Phase 1 Checkpoint**: Playable fleet logistics puzzle. No combat, but interesting planning and execution.

---

## Phase 2: Tactical Foundation

Bare-bones tactical combat layer. When player and enemy fleets meet at a body, enter a Newtonian combat arena.

### 2.1 Ship Entities
- Ships exist as individual entities (children of Fleet)
- Fleet's `ship_count` becomes actual Ship children
- Ships not rendered at strategic layer (fleet marker represents them)

### 2.2 Factions
- Faction component: `PlayerControlled` vs `EnemyControlled`
- Determines hostility and targeting

### 2.3 Enemy Garrisons
- Spawn enemy fleets at certain bodies (static, no movement)
- Enemy fleets have ships like player fleets

### 2.4 Combat Trigger
- Detect when player fleet arrives at body with enemy fleet
- Triggers transition to tactical mode

### 2.5 Tactical Mode Entry
- Time slows to ~60x realtime (from strategic ~864,000x)
- Camera zooms to 400,000 km × 400,000 km arena
- Body visible on right (outer edge 50,000 km into screen)
- Fleets spawn in center, 100,000 km apart, zero relative velocity
- `TacticalMode` resource tracks active combat state

### 2.6 Ship Rendering
- Draw individual ships in tactical view
- Different visual for player vs enemy

### 2.7 Ship Selection
- Click to select one ship
- Shift+click to add to selection
- Box select (click-drag rectangle)

### 2.8 Movement Orders + Physics
- Right-click to set destination
- Ships thrust toward destination (Newtonian motion)
- Avian 2D physics with f64 precision, 1 unit = 1 meter
- Linear CCD on all entities (handle high-speed collisions)
- See `plans/tactical-physics.md` for details

### 2.9 Basic Missiles
- Press 1 to fire missile at selected target
- Missiles fire if target within 50,000 km range
- Missiles track target, collision = kill (binary damage)
- No ammunition limits yet

### 2.10 Retreat + Win/Lose
- Ship reaching arena edge = retreated from battle
- If all ships for a side retreat → battle over
- Retreating ships must immediately pick new destination or be destroyed
- Destroy all enemies = tactical victory

**Phase 2 Checkpoint**: Playable tactical combat. Fly to enemy body, enter arena, select ships, maneuver, fire missiles, win by destroying enemies.

---

## Phase 3: Combat Depth

Expand tactical combat with more weapons, systems, and strategic integration.

### 3.1 Additional Weapons
- Coilgun slugs: ballistic (no tracking), very high speed
- PD rounds: short range defensive
- Interceptor missiles: defensive, target incoming missiles

### 3.2 Automated PD Defense
- PD fires automatically at incoming threats
- Based on stats/ranges, no player micromanagement

### 3.3 Sensors & Detection
- Probabilistic detection ranges (not guaranteed to see enemies)
- Trajectory prediction lines for projectiles

### 3.4 Ammunition & Resupply
- Finite missiles/slugs per ship
- Auto-resupply at friendly bodies
- Forces conservation and logistics thinking

### 3.5 Intercept Moving Fleets
- Fleets in transit can be targeted
- Game calculates intercept point
- Enables ambushes and defensive positioning

### 3.6 Capture Mechanics
- Bodies are capturable locations
- Fleet at body with no enemies → body flips after brief timer
- Timer prevents instant ping-ponging

### 3.7 Faction Asymmetry
**Outer System (Player):**
- Faster ships (higher delta-v and/or acceleration)
- Faster missiles
- Fewer ships total
- Starts with a few bodies

**Inner System (AI):**
- More missiles per ship
- More ships total
- Ships spread across multiple fleets
- Static garrisons at bodies

### 3.8 Campaign Win/Lose
- **Win**: Capture all bodies
- **Lose**: Lose all ships
- Fixed starting ships, no production
- Pure operational game: fighting and delta-v management

**Phase 3 Checkpoint**: Full tactical depth. Asymmetric factions, multiple weapon types, territory control, campaign victory conditions.

---

## Future Phases (Not Scoped Yet)

- Focus/bullet-time system tied to morale
- Heat as tactical resource
- Ordnance design and variety
- Ship customization
- Doctrine system for PD/engagement rules
- Electronic warfare and sophisticated sensors
- Lagrange points and manifolds as strategic terrain
- Inner system AI that patrols and responds
- Economic layer, logistics, trade
- Win condition arc toward kill-shot on Earth
