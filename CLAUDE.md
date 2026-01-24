# Project Structure

```
src/
  model/          # Pure data types + computation (no Bevy systems)
    fleet.rs      # Fleet, LogicalShip, FleetLocation, CombatState, Faction
    orbital.rs    # Body, OrbitalElements, propagate_elliptic
    transfer.rs   # TransferSolution, Lambert solver math

  common/         # Always-on systems (run in both modes)
    simulation.rs # SimulationTime, time controls
    rendering.rs  # Body positions, shapes, orbit gizmos
    ui.rs         # HUD (date/speed/zoom), body labels, victory overlay

  strategic/      # Strategic mode - full vertical slice
    commands.rs   # StrategicCommand enum (message-based input)
    systems.rs    # Fleet movement, arrival, combat detection
    input.rs      # Fleet selection, transfer planning
    rendering.rs  # Fleet shapes, transfer arcs, objective rings
    ui.rs         # Transfer popup, fleet tabs, info panel
    transfer_*.rs # LUT generation, arc visualization

  tactical/       # Tactical mode (not yet fully split out)
    mod.rs        # Arena, VisualShip, movement, rendering, UI - all in one for now
    commands.rs   # TacticalCommand enum
    input.rs      # Ship selection, box select, move orders

  # Infrastructure (root level)
  spatial.rs      # GridNode/GridLeaf - f64 precision hierarchy (big_space)
  camera.rs       # Camera animation, scale tracking
  physics.rs      # Avian3D integration with big_space sync
  app_state.rs    # AppState::Strategic | Tactical
  app_sets.rs     # System ordering: Input → Simulation → Render → Ui
  plugins.rs      # Plugin registration and system scheduling
```

## Design Principles

**Vertical slices by mode**: Each mode (strategic/tactical) owns its full stack - input, systems, rendering, UI. You can work on tactical combat without touching strategic code.

**Model is pure**: `model/` has no Bevy systems, just types and math. Can reason about game logic independently.

**Common for shared concerns**: Body rendering, time controls, HUD run regardless of mode.

**Message-based input**: Input systems post `StrategicCommand`/`TacticalCommand` messages, separate systems consume them. Decouples input handling from state mutation.

**Two-layer ships**: `LogicalShip` (persistent, strategic) vs `VisualShip` (ephemeral, tactical). Fleets survive combat; individual ships are spawned/despawned per battle.

## Maintenance

**Keep this codemap current**: Before every commit, verify the structure above matches reality. Update if files were added/moved/removed.

## Extension Points

- **New weapon type**: Add to `tactical/mod.rs` (component + systems), `tactical/input.rs` (fire command)
- **New strategic mechanic**: Add to `strategic/systems.rs`, wire input in `strategic/input.rs`
- **New UI panel**: Mode-specific goes in `strategic/ui.rs` or `tactical/ui.rs`, shared in `common/ui.rs`
- **New ship stat**: Add to `LogicalShip` in `model/fleet.rs`, use in `tactical/mod.rs`

---

# Working Style

- Do NOT change approach from what we discussed without talking to me first
- Use relative paths - we're already in the `impulse` directory (no need for `/Users/jh/personal/impulse/...`)
- If a fix isn't working as expected, discuss options before pivoting to a different solution
- When I ask "why isn't X working", I want to understand the problem, not have you silently switch to approach Y

# Bevy Development Notes

## Understanding the Bevy API

The Bevy game engine is included as a local submodule in this project. When working with Bevy APIs or looking for usage examples, you can reference the examples in the `bevy/examples/` directory.

These examples demonstrate the current Bevy API patterns and best practices for the version we're using (0.17.3).

## Local Development

This project uses a local Bevy dependency via git submodule, allowing us to:
- Make custom modifications to Bevy if needed
- Reference example code directly
- Ensure all dependencies use the same Bevy version via `[patch.crates-io]`
- Do not use cargo run
- You can't run the game yourself - I must do that