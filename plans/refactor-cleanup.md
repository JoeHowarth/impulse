# Refactor & Cleanup Migration Plan (Detailed Task Plan)

## Goal
Make the strategic/tactical split explicit while keeping a single shared universe, and complete the big_space + Avian precision pipeline without breaking current gameplay.

## Scope and Constraints
- One universe; tactical is a mode, not a separate scene.
- Strategic and tactical systems must be mode-scoped (no persistent branching on flags).
- Physics uses f64 Position; rendering uses big_space (CellCoord + Transform -> GlobalTransform).
- Existing gameplay (Phase 1/2 checkpoints) must remain playable after each phase.

---

## Phase 0 — Safety + Prep
**Intent:** Reduce migration risk and add visibility.

**Concrete Tasks**
- Inventory and categorize systems into Strategic / Tactical / Shared.
  - Produce a simple mapping list in this doc (or inline comments near system registration).
- Add temporary mode flags/resources to keep behavior stable during transition.
- Add a minimal smoke path (build + quick startup) and note it in the plan.
- Add debug asserts or logs where ordering is fragile (e.g., transfer execution before expiration).

**Implementation Notes**
- Avoid changing behavior; this is scaffolding only.

**Validation**
- Build succeeds.
- Current behavior unchanged.

---

## Phase 1 — Architecture Split (Plugins + States + SystemSets)
**Intent:** Make mode boundaries explicit and shrink coupling.

**Concrete Tasks**
- Introduce `AppState::Strategic | Tactical`.
- Define SystemSets for consistent ordering:
  - `InputSet`, `SimulationSet`, `RenderSet`, `UiSet`.
- Create feature plugins:
  - `StrategicPlugin`: strategic-only systems
  - `TacticalPlugin`: tactical-only systems
  - `TransferPlugin`: transfer LUT + transfer logic
  - `UiPlugin`: shared UI + state-scoped panels
  - `CameraPlugin`: camera scale + camera transitions
  - `PhysicsSyncPlugin`: big_space/Avian sync systems
- Move systems from `main.rs` into plugins and into SystemSets.
- Replace `if combat.active` gating with `in_state(AppState::Tactical)` for tactical systems.
- Keep shared systems registered globally (or as `OnEnter` where appropriate):
  - Transfer LUT init, camera scale updates, labels if used in both modes.

**Concrete System Moves (approx)**
- Strategic Input: body click selection, transfer popup handling, fleet number keys.
- Strategic Simulation: time controls, body updates, fleet transfers, objectives.
- Strategic Render: body shapes, fleet shapes, plan markers/arcs, labels.
- Tactical Input: box selection, tactical click, move orders.
- Tactical Simulation: ship movement, combat detection, tactical arena sync.
- Tactical Render: selection rings, move markers, tactical visuals.

**Validation**
- Strategic mode runs end-to-end.
- Tactical entry/exit still works.
- Inputs do not bleed across modes.

---

## Phase 2 — Precision Unification (big_space + Avian Sync)
**Intent:** Remove precision hacks and unify position handling.

**Concrete Tasks**
- Introduce `ComputedPosition` (or rename `ComputedBody`) for f64 heliocentric position + cell/local cache.
- Replace remaining f32 position caches with `ComputedPosition` or GlobalTransform.
- Replace any direct f32 world position reads in UI or selection with GlobalTransform.
- Install custom Avian sync systems:
  - CellCoord + Transform -> Position (before physics)
  - Position -> CellCoord + Transform (after physics)
- Remove tactical 100,000x scaling; restore real values for ship size, spacing, speed, accel, arrival thresholds.

**Concrete Touchpoints**
- `src/physics.rs`: replace sync systems and ensure scheduling in Avian sets.
- `src/tactical.rs`: revert scaled constants to real values.
- `src/ship.rs`: fleet positions derived from f64 heliocentric positions + grid conversion.
- `src/ui.rs` and `src/main.rs`: ensure projections use GlobalTransform.

**Validation**
- Tactical ships move correctly at Mercury/Neptune distances with real values.
- No jitter at extreme zoom.
- Strategic visuals unchanged.

---

## Phase 3 — Tactical Runtime Boundaries + Tactical Event Pipeline
**Intent:** Formalize tactical runtime as explicit mode, and decouple tactical input from gameplay.

**Concrete Tasks**
- Move tactical setup into `OnEnter(AppState::Tactical)`:
  - Spawn TacticalArena, VisualShips, tactical UI.
  - Start camera transition and set tactical time scale.
- Move tactical teardown into `OnExit(AppState::Tactical)`:
  - Despawn tactical visuals/entities, clear selection, restore camera/time.
- Define tactical command events:
  - `OrderMove`, `OrderFire`, `OrderTarget` (if needed).
- Tactical input systems emit events; tactical simulation consumes them.

**Decision Gate**
- Decide strategic sim handling during tactical:
  - Pause and freeze strategic UI, or
  - Slow tick with coarse updates.

**Validation**
- Tactical entry/exit is seamless (camera + UI transition).
- Tactical inputs are event-driven, no direct simulation mutation.

---

## Phase 4 — Strategic Event Pipeline
**Intent:** Decouple strategic input and planning from simulation logic.

**Concrete Tasks**
- Define strategic command events:
  - `PlanTransfer`, `CommitPlan`, `CancelLeg`, `SplitFleet`, `MergeFleet`.
- Strategic input/UI emit events; strategic simulation consumes them.
- Make UI read-only; no direct mutations from UI systems.

**Validation**
- Strategic input behaves the same as before.
- Input remapping can be done without modifying simulation logic.

---

## Phase 5 — Handoff Integrity + Cleanup
**Intent:** Ensure persistence and correctness across mode transitions.

**Concrete Tasks**
- Define explicit handoff flow in one place (state transition system):
  - Strategic arrival -> Tactical spawn
  - Tactical resolution -> Strategic updates (destroy/retreat outcomes)
- Ensure logical entities persist; tactical visuals are cleaned up on exit.
- Add a cleanup validator system (debug build only) to assert no VisualShip/TacticalArena remains outside tactical mode.

**Validation**
- Repeated battles do not leak entities/resources.
- Fleet/ship counts remain consistent after battles.

---

## Decision Points (Lock In Before Phase 3)
- Strategic sim behavior during tactical (pause vs slow tick).
- Tactical battles in transit (yes/no; affects arena anchor and movement).
- Time focus system (tactical-only vs global resource).

---

## Risks and Mitigations (Summary)
- **System ordering regressions:** use SystemSets; add debug asserts where order matters.
- **Avian sync precision issues:** keep all physics on f64 Position and use CellCoord + Transform for render only.
- **Visual artifacts at tactical zoom:** disable or LOD strategic orbits/arcs during tactical.
- **Event latency:** consume events in the same stage as input when possible.

---

## Recommended Migration Order
1. Phase 0 (inventory + safety)
2. Phase 1 (state/plugin split + SystemSets)
3. Phase 2 (precision unification + remove scaling hack)
4. Phase 3 (tactical runtime boundaries + tactical event pipeline)
5. Phase 4 (strategic event pipeline)
6. Phase 5 (handoff integrity + cleanup)

---

## Progress Update (2026-01-22)

### Completed
- Phase 1 (core): `AppState` + `AppSet` introduced; systems moved into plugins with ordered sets.
- Tactical/strategic input split implemented; combat now triggers state transition.
- Strategic markers (fleet triangles + enemy rings) are hidden in tactical and re-spawn on return.
- Transfer UI is hidden during tactical; transfer graphics remain visible if zooming out.
- Camera zoom lerp restored; camera now stays pinned to the moving arena frame (using big_space CellCoord).

### Challenges Encountered
- Tactical state gating initially froze body sizing/labels/markers; resolved by keeping body rendering + labels in shared systems while gating strategic-only UI/markers.
- Transfer popup updates were running during tactical and caused stale state; moved to strategic-only UI update set.

### Divergences From Initial Plan
- Strategic simulation continues during tactical (time, body motion, transfers), rather than pausing. This keeps the global clock consistent and aligns with the “single universe” model.
- UI hide/show logic for transfer panel was added earlier than Phase 3 to reduce tactical clutter and avoid input conflicts.

### Not Yet Done
- Phase 0 system mapping list and smoke path documentation.
- Phase 2 precision unification (ComputedPosition + Avian sync), removal of 100,000x tactical scaling.
- Phase 3 explicit tactical enter/exit teardown flow and event pipeline.
