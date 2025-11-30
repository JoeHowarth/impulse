# Phase 1: Fleet Management - Implementation Tasks

## Current Architecture (Updated)
- `Ship` component with delta-v, `PlayerControlled` marker
- `ShipLocation` enum: `AtBody(Entity)` or `InTransit { target, solution, departure_time }`
- `FlightPlan` component: `VecDeque<PlannedLeg>` + `committed_count` boundary
- `PlannedLeg`: target, departure_day, tof_days (source derived via `leg_source()` helper)
- Transfer: click body → popup → select option → append to FlightPlan (uncommitted)
- Enter commits all uncommitted legs, N cancels last leg
- Cache keyed by `(source_entity, target_entity, departure_day, tof_days)`
- `Transfer` entities synced from committed legs for visualization

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

---

## Phase 1 Complete
Playable fleet logistics game:
- Multiple fleets with ship counts
- Transfers anywhere in solar system
- Queue multi-hop journeys
- Split and merge fleets
- Reach objectives to win
