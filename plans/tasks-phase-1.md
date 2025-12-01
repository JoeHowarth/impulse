# Phase 1: Fleet Management - Implementation Tasks

## Current Architecture (Updated)
- `Ship` component with delta-v, `PlayerControlled` marker
- `ShipLocation` enum: `AtBody(Entity)` or `InTransit { target, solution, departure_time }`
- `FlightPlan` component: `VecDeque<PlannedLeg>` + `committed_count` boundary
- `PlannedLeg`: target, departure_day, tof_days (source derived via `leg_source()` helper)
- Transfer: click body → popup → select option → append to FlightPlan (uncommitted)
- Enter commits all uncommitted legs, N cancels last leg
- **TransferLut**: Precomputed lookup table replaces dynamic cache (see below)
- `Transfer` entities synced from committed legs for visualization

### Transfer LUT System (Replaces TransferCache)

The dynamic `TransferCache` was replaced with a precomputed `TransferLut` for instant lookups:

**Key changes:**
- LUT stores full `TransferSolution` objects (positions, velocities, orbital elements) not just delta-v
- Keyed by true anomaly buckets: `(source_idx, target_idx, ν_src_bucket, ν_tgt_bucket, tof_idx)`
- 72 anomaly buckets (5° resolution) × ~10-15 TOF candidates per body pair
- ~1.4M entries total, ~235MB on disk

**Startup flow:**
1. Check if `assets/transfer_lut.bin` exists
2. Validate version, body list, bucket count
3. If invalid/missing: regenerate using rayon parallelization (~1-2s on 8 cores)
4. Save to disk for next run
5. Build entity mappings for runtime queries

**Benefits:**
- Instant transfer lookups (no async compute, no "Loading..." popups)
- Exact solutions: ship departs at bucket-center anomaly, eliminating positional error
- Simpler code: ~700 lines of cache management deleted

**Files:**
- `transfer_lut.rs`: LUT struct, generation, lookup methods
- `transfer.rs`: `TransferSolution` with serde derives
- Deleted: `transfer_cache.rs`, `src/bin/gen_transfer_lut.rs`

---

## Step 1.1: Cross-system Transfers

### 1.1a: Remove same-parent restriction

**Goal**: Ship can transfer to any body in solar system (Luna → Mars, etc.)

**SOI simplification**: Use heliocentric positions for all transfers. The existing `compute_transfer()` already works in heliocentric coordinates, so no changes needed to the Lambert solver.

#### Changes Required

**1. main.rs - `handle_body_click()` (~line 466)**
```
Remove:
- Lines 493-496: `let current_parent = ...` lookup
- Lines 515-518: `if body.parent_name != current_parent { continue; }`
- Line 500 comment about "same parent"
```
Result: Any visible body (except current) is clickable.

**2. transfer_cache.rs - `init_transfer_cache()` (~line 162)**
```
Change sibling filter (line 191-194):
FROM: .filter(|(e, b)| b.parent_entity == source_body.parent_entity && *e != current_entity)
TO:   .filter(|(e, _)| *e != current_entity)
```
Result: Cache computed for ALL bodies, not just siblings.

**3. transfer_cache.rs - `update_transfer_cache()` (~line 233)**
```
Same change to sibling filter (line 279-283)
```

**4. transfer_cache.rs - `spawn_cache_compute_task()` (~line 381)**
```
Same change to sibling filter (lines 316-320, 406-410)
```

**5. Update doc comments**
- Remove "siblings" / "same parent" language
- Update module doc to reflect "all bodies"

#### Performance Consideration
Currently caches ~6500 solutions per sibling. With 22 bodies total, caching all would be ~130k solutions. This might be slow on init. Options:
- Accept slower startup (simplest)
- Lazy compute on first click to non-cached body
- Background async init

**Proposal**: Accept slower startup for now. Profile later.

#### Verification
- [x] Build succeeds
- [x] Can click bodies with different parents (e.g., Earth → Jupiter)
- [x] Transfer popup shows valid options
- [x] Transfer executes correctly
- [x] No regression on same-parent transfers

**Status**: COMPLETE - Implemented as planned.

---

### 1.1b: Transfer queue

**Goal**: Ship can have multiple transfers queued (A → B → C)

#### Data Model

**Option A**: Add queue to `ShipState`
```rust
pub enum ShipState {
    Orbiting { body: Entity },
    Transferring {
        solution: TransferSolution,
        departure_time: f64,
        arrival_time: f64,
        target: Entity,
    },
}
// + separate component for queue
```

**Option B**: New component alongside Ship
```rust
#[derive(Component, Default)]
pub struct TransferQueue {
    pub queued: VecDeque<QueuedTransfer>,
}

pub struct QueuedTransfer {
    pub target: Entity,
    pub departure_day: i32,  // relative to arrival at previous destination
    pub tof_days: i32,
}
```

**Proposal**: Option B - cleaner separation, queue persists across state changes.

#### Changes Required

**1. ship.rs - Add TransferQueue component**
- New `TransferQueue` component with `VecDeque<QueuedTransfer>`
- Add to ship entity on spawn

**2. ship.rs - `check_ship_arrival()`**
- After transitioning to `Orbiting`, check if queue has entries
- If yes: compute transfer for next queued destination, schedule it
- Pop from queue, transition back to `Transferring`

**3. ui.rs - Queue UI**
- Shift-click on transfer option → add to queue instead of immediate schedule
- Visual indicator when queue is non-empty (queue icon? list?)
- Way to view/clear queue

**4. transfer_cache.rs - Cache for queued destinations**
- When queuing a transfer, need cache for THAT body's outgoing transfers
- May need to trigger async cache compute for intermediate destinations

#### Edge Cases
- What if queued transfer is no longer valid when ship arrives? (body moved, dv changed)
  - **Proposal**: Recompute on arrival. If no valid transfer, notify player, stay at body.
- Queue while already transferring?
  - **Proposal**: Allow. Queue applies after current transfer completes.

#### Verification
- [x] Can queue transfer while orbiting
- [x] Queued transfer executes after current completes
- [x] Queue UI shows pending transfers (flight plan panel with rows)
- [x] Can clear queue (N key cancels last leg)
- [x] Handles invalid queued transfer gracefully

**Status**: COMPLETE

**Implementation notes (differs from original plan):**
- Used `FlightPlan` with `committed_count` boundary instead of per-leg `committed` boolean
- `ShipLocation` replaces `ShipState`, with `InTransit` embedding the solution
- Source derived via `leg_source()` helper, not stored in leg
- Solution looked up from cache via exact key, not stored in leg
- Simplified UI: click appends uncommitted, Enter commits all (no shift+click distinction)
- Flight plan panel shows rows with: destination, arrival day, dv, remaining dv
- Cache key includes source_entity; unused sources pruned when no longer in plan
- System ordering: `execute_departure` before `expire_stale_legs` (debug_assert validates)
- `find_best_transfer_in_range` returns `tof_days` for exact cache key matching

---

## Step 1.2: Fleets

### 1.2a: Ship becomes Fleet
- Add `ship_count: u32` to `Ship` component (or rename component to `Fleet`)
- Delta-v stays fleet-wide (all ships same type for now)
- Display ship count in UI panel and on fleet marker

**Checkpoint**: Same game, but "ship" is now "fleet of N ships"

### 1.2b: Multiple fleets
- Spawn 2-3 fleets at different starting locations
- Add selection state: `Selected` marker component
- Click fleet to select (deselects others)
- Only selected fleet shows transfer popup on body click
- Show selected fleet status in info panel

**Checkpoint**: Manage multiple fleets, plan transfers independently

#### Verification
- [x] Fleet component with ship_count field
- [x] 3 fleets spawned (Alpha/Earth, Bravo/Mars, Charlie/Jupiter)
- [x] Selected marker component
- [x] Click selects fleet, Shift+click opens transfer popup
- [x] Fleet tabs at bottom of screen with name, ships, delta-v
- [x] Number keys 1-9 select fleets
- [x] Visual offset for multiple fleets at same body

**Status**: COMPLETE

**Implementation notes:**
- `Fleet` component replaces `Ship` with `name`, `ship_count`, `delta_v_remaining`
- `Selected` marker component for active fleet
- `compute_fleet_positions()` helper offsets multiple fleets at same body in semicircle
- Fleet tabs UI at bottom center shows all fleets with keyboard hints
- Click detection uses offset positions for accurate selection

---

## Step 1.3: Merge/Split

### 1.3a: Split operation
- UI on selected fleet: "Split" button
- Opens dialog: specify ship count to split off
- Creates new fleet entity at same body with specified count
- Original fleet keeps remainder

**Checkpoint**: Can divide forces

### 1.3b: Merge operation
- When 2+ fleets at same body, show "Merge" option
- First pass: "merge all fleets here" button (simpler than multi-select)
- Combines ship_count, despawns merged fleet entities

**Checkpoint**: Can consolidate forces

#### Verification
- [x] S key splits selected fleet in half
- [x] M key merges all fleets at body into selected
- [x] New fleets get unique NATO phonetic names
- [x] Only works at body (not in transit)

**Status**: COMPLETE

**Implementation notes:**
- Keyboard-driven: S for split, M for merge (simpler than dialogs)
- Split divides in half (larger half stays with original)
- New fleet names from NATO phonetic alphabet (Delta, Echo, Foxtrot...)
- Merge takes highest delta-v, combines ship counts
- `FLEET_COUNTER` atomic for unique name generation

---

## Step 1.4: Objectives

### 1.4a: Objective display
- New component: `Objective { required_ships: u32 }` on certain bodies
- Render objective marker (icon + "0/5 ships" text)
- Track friendly ships present at body

**Checkpoint**: Can see goals, progress tracked

### 1.4b: Win condition
- System checks: all objectives satisfied?
- Victory screen/overlay when complete
- Setup: objectives require more ships than any single starting fleet (forces splitting/routing)

**Checkpoint**: Complete logistics puzzle with win state

#### Verification
- [x] Objective component with required_ships
- [x] Mars (8 ships) and Saturn (6 ships) objectives
- [x] Orange ring + X/Y ships counter at objective bodies
- [x] Ring turns green when objective satisfied
- [x] VictoryState resource tracks win
- [x] Victory overlay shows when all objectives complete

**Status**: COMPLETE

**Implementation notes:**
- `Objective { required_ships }` component attached to body entities
- `VictoryState` resource with `victory_achieved` flag and `victory_time`
- `count_ships_at_body()` helper counts all player ships at a body
- `check_objectives` system runs each frame, sets victory when all satisfied
- `render_objectives` draws colored ring (orange/green) and X/Y ship count
- Victory overlay covers screen with "VICTORY" and completion time

---

## Phase 1 Complete
Playable fleet logistics game:
- Multiple fleets with ship counts
- Transfers anywhere in solar system
- Queue multi-hop journeys
- Split and merge fleets
- Reach objectives to win
