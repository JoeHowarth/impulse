# Missile Guidance System v2 - Design Spec

## Overview

Replace the current simple pursuit guidance with a three-phase guidance system that models realistic missile behavior: boost, coast with corrections, and terminal homing.

## Missile Flight Profile

```
LAUNCH → [BOOST] → [COAST] → [TERMINAL] → IMPACT
           1s        variable    ~200m
```

### Phase 1: Boost
- **Duration**: 1 second
- **Acceleration**: 100 m/s²
- **Delta-V gained**: 100 m/s (added to ship's velocity at launch)
- **Distance covered**: ~50m during boost
- **Guidance**: Predictive lead - heading locked at launch
- **Steering**: None (committed to initial heading)

### Phase 2: Coast
- **Duration**: Variable (until terminal distance)
- **Thrust**: None, except for single midcourse correction
- **Guidance**: Predictive lead recalculation
- **Correction budget**: 6 m/s delta-V
- **Correction trigger**: When distance crosses 50% of initial distance

### Phase 3: Terminal
- **Trigger**: Distance to target < 200m
- **Guidance**: Proportional Navigation (N=3)
- **Maneuvering budget**: 4 m/s delta-V
- **Purpose**: Handle last-second evasion, correct accumulated errors

## Delta-V Budget

| Phase | Budget | Purpose |
|-------|--------|---------|
| Boost | 100 m/s | Reach cruise velocity |
| Midcourse | 6 m/s | Single heading correction |
| Terminal | 4 m/s | PN maneuvering |
| **Total** | **110 m/s** | |

The 10 m/s correction budget (10% of boost) is split 60/40 between midcourse and terminal.

---

## Guidance Algorithms

### Boost Phase: Predictive Lead

At launch, calculate where the target will be when the missile arrives, and aim there.

**Simplified calculation** (v1 - treats boost as near-instant):
```rust
let cruise_speed = BOOST_ACCEL * BOOST_DURATION;  // 100 m/s relative
let distance = (target_pos - missile_pos).length();
let flight_time = distance / cruise_speed;
let intercept_point = target_pos + target_vel * flight_time;
let heading = (intercept_point - missile_pos).normalize();
```

**Limitations of v1**:
- Ignores 50m traveled during boost
- Ignores launch ship's velocity contribution to intercept geometry
- Assumes constant target velocity

**Future improvement**: Iterative solver that accounts for:
1. Missile position/velocity during and after boost
2. Full flight time including boost duration
3. Target acceleration estimation

### Midcourse Correction: Heading Adjustment

Single correction fired at ~50% of original distance. Purpose is to correct for:
- Initial prediction errors
- Target velocity changes since launch

```rust
// Recalculate intercept with current state
let distance = (target_pos - missile_pos).length();
let time_to_target = distance / missile_speed;
let new_intercept = target_pos + target_vel * time_to_target;

// Compute required heading change
let desired_heading = (new_intercept - missile_pos).normalize();
let current_heading = missile_vel.normalize();
let correction = desired_heading - current_heading;

// Apply correction (capped by budget)
let correction_dv = correction.length().min(midcourse_budget);
missile_vel += correction.normalize() * correction_dv;
```

This is an instantaneous velocity change (impulsive maneuver), not continuous thrust.

### Terminal Phase: Proportional Navigation

PN steers to keep the line-of-sight (LOS) angle constant, which guarantees intercept.

```rust
// Line of sight angle
let los_vec = target_pos - missile_pos;
let los_angle = los_vec.y.atan2(los_vec.x);

// LOS rate (how fast the angle is changing)
let los_rate = (los_angle - prev_los_angle) / dt;

// Closing speed (positive when approaching)
let distance = los_vec.length();
let closing_speed = (prev_distance - distance) / dt;

// PN guidance law: accelerate perpendicular to LOS
let N = 3.0;  // Navigation constant
let accel_command = N * closing_speed * los_rate;

// Apply acceleration perpendicular to velocity
let perp = DVec3::new(-missile_vel.y, missile_vel.x, 0.0).normalize();
let accel_magnitude = accel_command.abs().min(budget / dt);
missile_vel += perp * accel_magnitude.copysign(accel_command) * dt;
terminal_budget -= accel_magnitude * dt;
```

---

## Data Structures

### Missile Component (replaces current simple version)

```rust
#[derive(Component)]
pub struct Missile {
    pub target: Entity,
    pub owner_faction: Faction,
    pub phase: MissilePhase,
    pub initial_distance: f64,  // for midcourse trigger
}

#[derive(Clone)]
pub enum MissilePhase {
    Boost {
        heading: DVec3,        // locked at launch
        time_remaining: f64,   // counts down from BOOST_DURATION
    },
    Coast {
        budget: f64,           // delta-V remaining for correction
        correction_done: bool, // only one allowed
    },
    Terminal {
        budget: f64,           // delta-V remaining for PN
        prev_los: f64,         // previous LOS angle
        prev_distance: f64,    // previous distance (for closing speed)
    },
}
```

### Constants

```rust
// Boost phase
const BOOST_ACCEL: f64 = 100.0;       // m/s²
const BOOST_DURATION: f64 = 1.0;      // seconds

// Delta-V budgets
const MIDCOURSE_BUDGET: f64 = 6.0;    // m/s
const TERMINAL_BUDGET: f64 = 4.0;     // m/s

// Phase transitions
const TERMINAL_DISTANCE: f64 = 200.0; // meters

// PN constant
const PN_GAIN: f64 = 3.0;
```

---

## Phase Transitions

```
┌─────────┐
│  BOOST  │──── time_remaining <= 0 ────►┌─────────┐
└─────────┘                               │  COAST  │
                                          └────┬────┘
                                               │
                              distance < TERMINAL_DISTANCE
                                               │
                                               ▼
                                         ┌──────────┐
                                         │ TERMINAL │
                                         └──────────┘
```

---

## Engagement Characteristics

### Reference Scenario
- **Missile cruise**: 100 m/s (relative to launch)
- **Ship max accel**: 1-5 m/s²
- **Ship rotation**: Slow (exact rate TBD)

### Flight Times (approximate)
| Distance | Flight Time | Target Displacement at 3 m/s² |
|----------|-------------|-------------------------------|
| 500m | 5s | 37m |
| 1000m | 10s | 150m |
| 2000m | 20s | 600m |

Longer range = more time for evasion = lower hit probability.

### Stationary Targets
A stationary (or constant-velocity) target is a guaranteed hit at any range, assuming no obstacles. The predictive guidance will calculate correct lead.

### Evading Targets
Target acceleration during flight causes prediction errors:
- **Small accel (1-2 m/s²)**: Midcourse correction usually sufficient
- **Large accel (3-5 m/s²)**: May need terminal PN to close the gap
- **Late maneuver**: PN handles this if target waits until terminal phase

---

## Implementation Notes

### Collision Detection
At 100 m/s and 64 Hz physics, missile moves ~1.5m per frame. This is fine for the 50m radius ship colliders. SweptCcd already enabled.

### Velocity Inheritance
Missile starts with firing ship's velocity. If ship is moving at 30 m/s and fires forward, missile will be at 130 m/s after boost (30 + 100).

### Target Velocity Tracking
For v1, use instantaneous `LinearVelocity` of target. Future improvement: track velocity over several frames to estimate acceleration.

### Rotation
Assume instant rotation for v1. Missile can reorient heading instantly for corrections. Future improvement: angular velocity limits.

### Coordinate System
All calculations in arena-local coordinates (meters). Use `arena_grid.grid_position_double()` for precision.

---

## Test Scenarios

| # | Scenario | Distance | Target Motion | Success Criteria |
|---|----------|----------|---------------|------------------|
| 1 | Stationary target | 500m | None | Hit without corrections |
| 2 | Constant velocity | 500m | 20 m/s perpendicular | Hit with lead calculation |
| 3 | Constant velocity | 1000m | 30 m/s perpendicular | Hit, may need correction |
| 4 | Mild evasion | 500m | 2 m/s² perpendicular | Hit with correction |
| 5 | Hard evasion | 500m | 5 m/s² perpendicular | Hit with correction + PN |
| 6 | Late dodge | 500m | 5 m/s² in terminal phase | PN handles it |
| 7 | Long range | 2000m | 20 m/s + 2 m/s² | Tests full system |

---

## Future Improvements

1. **Iterative intercept solver**: Properly account for boost kinematics
2. **Acceleration tracking**: Observe target accel over 3-5 frames
3. **Multiple corrections**: Allow 2-3 smaller corrections instead of one
4. **Angular velocity limits**: Realistic missile rotation rates
5. **Countermeasures**: Flares, chaff, ECM affecting guidance
6. **Proximity fuze**: Detonate near target instead of requiring direct hit

---

## Migration from Current System

Current simple pursuit (`update_missile_guidance`) will be replaced by:
1. `update_missile_boost` - handles boost phase
2. `update_missile_coast` - handles coast + midcourse
3. `update_missile_terminal` - handles PN guidance

Spawning logic in `update_missile_firing` needs to:
1. Calculate initial heading using predictive lead
2. Initialize `MissilePhase::Boost` with that heading
3. Store `initial_distance` for midcourse trigger

The existing collision handling (`handle_missile_collisions`) remains unchanged.
