# Impulse Development Plan

## Current State
- 1 ship with delta-v budget
- Lambert transfers between same-parent bodies
- Transfer cache, popups, hover previews
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

## Phase 2: Simple Combat

### 2.1 Combat Entities
All projectiles are real entities with visuals:
- Missiles (offensive, maneuverable)
- Coilgun slugs (offensive, ballistic)
- PD rounds (defensive, short range)
- Interceptor missiles (defensive)

One type of each for now. No ordnance variety yet.

### 2.2 Combat Physics
- Newtonian trajectories + gravity vector
- Skip orbital mechanics during fights (ship acceleration dominates)
- Basic trajectory prediction lines

### 2.3 Combat Systems
- **Offense**: Player chooses targets for missiles/coilguns
- **Defense**: Automated PD, no active control (fires based on stats/ranges)
- **Sensors**: Probabilistic detection ranges (not guaranteed)
- **Damage**: Binary - any impact is fatal

### 2.4 Engagement Model
- Proximity-based engagement initiation
- Fleets in transit can be intercepted (target a moving fleet, game calculates intercept point)
- Fleets as valid transfer targets

### 2.5 Ammunition & Resupply
- Finite missiles/slugs
- Auto-resupply at friendly bodies
- (Possibly strictly finite at start - no resupply)

### 2.6 Capture Mechanics
- Bodies are capturable locations
- Fleet at body with no enemies → body flips after brief timer
- Timer prevents instant ping-ponging

### 2.7 Faction Asymmetry
**Outer System (Player):**
- Faster ships (higher delta-v and/or acceleration)
- Faster missiles
- Fewer ships total
- Starts with a few bodies

**Inner System (AI):**
- More missiles per ship
- More ships total
- Ships spread across multiple fleets
- Static garrisons at bodies (like dungeons to beat)
- Iterate AI behavior later

### 2.8 Win/Lose Conditions
- **Win**: Capture all bodies
- **Lose**: Lose all ships
- Fixed starting ships, no production
- Pure operational game: fighting and delta-v management

**Phase 2 Checkpoint**: Small but playable game. Asymmetric factions, real combat with tactical depth, capture objectives.

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
