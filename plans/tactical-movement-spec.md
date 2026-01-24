# Tactical Movement Specification

## Motivation

Space combat positioning isn't about absolute locations - there's no terrain to capture. What matters is *geometry*: distances, angles, overlapping fields of fire, sensor coverage, defensive screens. Position only has meaning relative to other things.

Traditional RTS "move to waypoint" commands don't capture this. "Go to point X" is meaningless when everything is moving and the only thing that matters is your relationship to friendlies and enemies.

We need primitives that express *relational positioning* directly.

## Core Concept: The Flagship Reference Frame

Every fleet has a **designated flagship**, chosen automatically by the game (e.g., largest ship, or first capital ship). This is a property of the fleet, not a player choice (for now).

All positioning is relative to a reference frame defined by:

1. **Anchor** (Anchor 1): Your flagship. The center of your formation.
2. **Axis** (toward Anchor 2): The line from your flagship toward the enemy flagship. This defines "forward" - the threat axis. The enemy flagship is also game-designated (same logic - their fleet's flagship).

Ships specify their position as:
- **Angle**: Degrees from the threat axis (0° = directly between flagships, +90° = perpendicular)
- **Radius**: Distance from your flagship

When the enemy moves, the axis rotates. Your ships automatically adjust to maintain their angular position relative to the threat. When your flagship moves, the whole formation moves with it.

This means: **moving your flagship moves your entire formation**. The flagship is the formation's anchor, and small flagship maneuvers can efficiently reposition your whole fleet rather than having every escort burn to new positions.

## Flagship Movement

The flagship itself uses a different control scheme - direct acceleration:

- Select flagship
- Right-click-drag to set acceleration vector
- Drag direction = thrust direction
- Drag length = thrust magnitude

The flagship commits to this burn. Escorts automatically adjust to maintain their assigned positions in the (now moving) reference frame.

## Escort Positioning Controls

### Basic Assignment

1. Select one or more escort ships
2. Right-click-drag to a position
3. Game infers the relationship:
   - Anchor 1: Your flagship (automatic)
   - Anchor 2: Nearest/primary enemy flagship (automatic)
   - Angle: Computed from where you dragged relative to the axis
   - Radius: Distance from flagship to drag point

### Visual Feedback While Dragging

- Axis line drawn from your flagship toward enemy flagship
- Arc showing the angle being set
- Circle at the drag radius
- Readout: "45° / 15km from Flagship"

### Urgency Modifier

Hold modifier key while dragging to set urgency:
- No modifier: Normal urgency
- Shift: Aggressive (faster repositioning, more fuel)
- Ctrl: Lazy (slow repositioning, conserve fuel)

Or: Tap 1/2/3 after positioning to adjust urgency.

### Multi-Ship Assignment

**Future work.** For now, position ships individually.

## Visual Feedback During Play

- **Axis line**: Faint line from flagship toward enemy flagship
- **Assigned position ghost**: Where the ship should be (when out of tolerance)
- **Tolerance zone**: Faint circle/arc showing acceptable position range
- **Status indicator**: Different color/icon for "in position" vs "repositioning"

## Urgency Levels

| Level | Behavior | Use Case |
|-------|----------|----------|
| Lazy | Gentle burns, wide tolerance, slow correction | Fuel conservation, low threat |
| Normal | Moderate burns, reasonable responsiveness | Default |
| Aggressive | Hard burns, tight tolerance, fast correction | Active combat, critical positioning |
| Emergency | Maximum thrust, immediate repositioning | Imminent threat, damn the fuel |

## Tolerance and Station-Keeping

Ships don't maintain perfect positions - that would require constant thrust. Instead:

- Each urgency level has a tolerance band (angle ± X°, radius ± Y km)
- When within tolerance: coast (no thrust)
- When outside tolerance: thrust to correct
- Higher urgency = tighter tolerance = more fuel burn

## Examples

### Example 1: Defensive PD Screen

**Situation**: Your flagship faces an enemy flagship 200km away. You want PD escorts screening against incoming missiles.

**Commands**:
1. Select PD ship 1, drag to 0° / 10km (directly on axis, close to flagship)
2. Select PD ship 2, drag to +30° / 10km
3. Select PD ship 3, drag to -30° / 10km

**Result**: PD ships form a screen facing the enemy, 10km ahead of flagship. As the enemy maneuvers, the screen rotates to stay between you and them.

### Example 2: Flanking Missile Ships

**Situation**: You want missile ships on the flanks to get firing angles around the enemy's frontal PD.

**Commands**:
1. Select missile ship 1, drag to +60° / 20km
2. Select missile ship 2, drag to -60° / 20km

**Result**: Missile ships hold flanking positions. When they fire, missiles approach the enemy from angles their forward-facing PD can't cover.

### Example 3: Sensor Picket

**Situation**: You want a ship far forward for early detection.

**Commands**:
1. Select sensor ship
2. Drag to 0° (directly on axis), 50km from flagship

**Result**: Sensor ship holds position between fleets, providing early warning.

### Example 4: Flagship Maneuver to Reposition Formation

**Situation**: Enemy is flanking left. Rather than have all escorts burn to new positions, you swing the flagship.

**Commands**:
1. Select flagship
2. Right-click-drag a short acceleration vector to the right

**Result**: Flagship begins moving right. The threat axis rotates. All escorts automatically adjust, "swinging" to face the new threat angle. Small flagship burn, minimal escort fuel expenditure.

### Example 5: Closing to Engagement Range

**Situation**: Fleets are 200km apart. You want to close to 50km for weapon range.

**Commands**:
1. Select flagship
2. Right-click-drag acceleration toward enemy (significant magnitude)
3. Later: drag opposite direction to decelerate

**Result**: Flagship burns toward enemy, entire formation advances. Escorts maintain relative positions throughout.

## Control System (Implementation Direction)

Ships compute desired position in a rotating reference frame:
- Origin: Flagship position
- +X axis: Toward enemy flagship
- Desired position: `(radius * cos(angle), radius * sin(angle))`
- Desired velocity: Match flagship velocity (relative V = 0)

PD controller computes thrust:
```
pos_error = desired_pos - actual_pos  (in rotating frame)
vel_error = desired_vel - actual_vel

thrust = Kp * pos_error + Kd * vel_error
```

Urgency scales Kp/Kd. Tolerance defines dead zone where ship coasts.

## Future Extensions (Not In Initial Scope)

- Multi-ship selection with automatic angle distribution
- Player-created anchor points (rally points, zones)
- Derived anchors (midpoint, center of mass)
- More anchor types beyond flagship-to-flagship
- Formation templates (wedge, screen, echelon)
- Waypoint sequences for flagship
- Automatic fuel management / warnings
- Flagship deceleration helpers ("kill relative velocity")
