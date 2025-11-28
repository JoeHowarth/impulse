//! Ship entity and transfer scheduling.
//!
//! Tracks player ships that can travel between celestial bodies,
//! managing delta-v budgets and scheduled transfers.

use bevy::prelude::*;
use bevy_vector_shapes::prelude::*;

use crate::transfer::{TransferSolution, propagate_kepler_full};
use crate::orbital_data::{Body, MU_SUN};
use crate::phys_to_visual;
use crate::ComputedBody;

// ============================================================================
// Components
// ============================================================================

/// Marker for the player-controlled ship.
/// Systems use this to find the ship whose state determines valid click targets, etc.
#[derive(Component)]
pub struct PlayerControlled;

/// A ship that can travel between celestial bodies.
#[derive(Component)]
pub struct Ship {
    /// Remaining delta-v budget in m/s
    pub delta_v_remaining: f64,
    /// Ship name (for future multi-ship support)
    pub name: String,
}

/// Ship's current state - orbiting a body or in transit.
#[derive(Component)]
pub enum ShipState {
    /// Ship is at a celestial body
    Orbiting { body: Entity },
    /// Ship is in transit between bodies
    Transferring {
        solution: TransferSolution,
        departure_time: f64,
        arrival_time: f64,
        target: Entity,
    },
}

// ============================================================================
// Resources
// ============================================================================


// ============================================================================
// Systems
// ============================================================================

/// Executes scheduled transfers when their departure time arrives.
/// Queries Transfer entities and starts any whose departure_time has passed
/// (if the ship is still Orbiting).
pub fn execute_scheduled_transfers(
    transfers: Query<&crate::transfer_vis::Transfer>,
    mut ships: Query<(&mut Ship, &mut ShipState)>,
    sim_time: Res<crate::simulation::SimulationTime>,
) {
    for transfer in &transfers {
        // Only execute if departure time has arrived
        if sim_time.sim_time < transfer.departure_time {
            continue;
        }

        // Only execute if ship is still orbiting (not already transferring)
        let Ok((mut ship, mut state)) = ships.get_mut(transfer.ship) else {
            continue;
        };

        if !matches!(*state, ShipState::Orbiting { .. }) {
            continue;
        }

        // Deduct delta-v (departure burn)
        let departure_dv = transfer.solution.departure_dv.norm();
        ship.delta_v_remaining -= departure_dv;

        info!(
            "Ship '{}' departing! dv spent: {:.0} m/s, remaining: {:.0} m/s",
            ship.name, departure_dv, ship.delta_v_remaining
        );

        // Transition to transferring
        let arrival_time = transfer.departure_time + transfer.solution.time_of_flight;
        *state = ShipState::Transferring {
            solution: transfer.solution.clone(),
            departure_time: transfer.departure_time,
            arrival_time,
            target: transfer.target,
        };
    }
}

/// Checks if transferring ships have arrived at their destination.
/// Deducts arrival delta-v and transitions ship to Orbiting state.
pub fn check_ship_arrival(
    mut ships: Query<(&mut Ship, &mut ShipState)>,
    bodies: Query<&Body>,
    sim_time: Res<crate::simulation::SimulationTime>,
) {
    for (mut ship, mut state) in &mut ships {
        // Clone state data to avoid borrow issues
        let transfer_info = if let ShipState::Transferring { solution, arrival_time, target, .. } = &*state {
            Some((solution.clone(), *arrival_time, *target))
        } else {
            None
        };

        if let Some((solution, arrival_time, target)) = transfer_info {
            if sim_time.sim_time >= arrival_time {
                // Deduct arrival burn delta-v
                let arrival_dv = solution.arrival_dv.norm();
                ship.delta_v_remaining -= arrival_dv;

                // Get target body name for logging
                let target_name = bodies
                    .get(target)
                    .map(|b| b.name.clone())
                    .unwrap_or_else(|_| "Unknown".to_string());

                info!(
                    "Ship '{}' arrived at {}! dv spent: {:.0} m/s, remaining: {:.0} m/s",
                    ship.name, target_name, arrival_dv, ship.delta_v_remaining
                );

                // Transition to orbiting
                *state = ShipState::Orbiting { body: target };
            }
        }
    }
}

/// Handles time reversal: if sim time goes backward past a transfer's departure,
/// cancel the transfer and refund the delta-v.
pub fn handle_time_reversal(
    mut ships: Query<(&mut Ship, &mut ShipState)>,
    sim_time: Res<crate::simulation::SimulationTime>,
) {
    for (mut ship, mut state) in &mut ships {
        // Check if we're transferring and time went backward past departure
        let revert_info = if let ShipState::Transferring { solution, departure_time, .. } = &*state {
            if sim_time.sim_time < *departure_time {
                Some(solution.departure_dv.norm())
            } else {
                None
            }
        } else {
            None
        };

        if let Some(departure_dv) = revert_info {
            // Refund departure delta-v
            ship.delta_v_remaining += departure_dv;

            warn!(
                "Time reversal detected! Canceling transfer for '{}', refunded {:.0} m/s",
                ship.name, departure_dv
            );

            // TODO: This is a limitation - we don't know the source body.
            // Will be fixed when Transfer component stores source entity.
            *state = ShipState::Orbiting {
                body: Entity::PLACEHOLDER,
            };
        }
    }
}

// ============================================================================
// Rendering
// ============================================================================

/// Ship triangle color (cyan)
const SHIP_COLOR: Color = Color::srgb(0.3, 0.9, 0.9);

/// Ship size in visual units
const SHIP_SIZE: f32 = 3.0;

/// Departure marker color (distinct from departure burn arrow)
const DEPARTURE_MARKER_COLOR: Color = Color::srgb(0.9, 0.9, 0.3); // Yellow

/// Renders ships as triangles at their current positions.
/// - When orbiting: positioned at body location
/// - When transferring: positioned along transfer arc
pub fn render_ship(
    ships: Query<(&Ship, &ShipState)>,
    bodies: Query<&ComputedBody>,
    sim_time: Res<crate::simulation::SimulationTime>,
    mut painter: ShapePainter,
) {
    for (_ship, state) in &ships {
        let (position, velocity_dir) = match state {
            ShipState::Orbiting { body } => {
                // Position at body's current location
                let body_pos = bodies.get(*body).map(|c| c.position).unwrap_or(Vec3::ZERO);
                // Offset slightly so ship is visible next to body
                let offset_pos = body_pos + Vec3::new(SHIP_SIZE * 1.5, 0.0, 0.0);
                // Velocity direction pointing "forward" in orbit (tangent)
                (offset_pos, Vec3::new(0.0, 1.0, 0.0))
            }
            ShipState::Transferring { solution, departure_time, .. } => {
                // Propagate position along transfer arc
                let elapsed = sim_time.sim_time - departure_time;
                if elapsed < 0.0 {
                    // Before departure - shouldn't happen but handle gracefully
                    continue;
                }

                // Get current position and velocity on transfer orbit
                if let Some((r_vec, v_vec)) = propagate_kepler_full(
                    solution.departure_pos,
                    solution.departure_vel,
                    MU_SUN,
                    elapsed,
                ) {
                    let pos = phys_to_visual(r_vec);

                    // Use actual velocity direction (project to 2D XY plane)
                    let vel_dir = Vec3::new(v_vec.x as f32, v_vec.y as f32, 0.0).normalize_or_zero();

                    (pos, vel_dir)
                } else {
                    continue;
                }
            }
        };

        // Draw triangle pointing in velocity direction
        painter.set_translation(position);

        // Rotate to point in velocity direction (triangle points up by default)
        let rotation = if velocity_dir.length_squared() > 0.001 {
            Quat::from_rotation_arc(Vec3::Y, velocity_dir)
        } else {
            Quat::IDENTITY
        };
        painter.set_rotation(rotation);

        painter.set_color(SHIP_COLOR);

        // Draw an isoceles triangle pointing up
        let half_base = SHIP_SIZE * 0.5;
        let height = SHIP_SIZE;
        painter.thickness = 0.5;
        painter.line(Vec3::new(0.0, height * 0.5, 0.0), Vec3::new(-half_base, -height * 0.5, 0.0));
        painter.line(Vec3::new(-half_base, -height * 0.5, 0.0), Vec3::new(half_base, -height * 0.5, 0.0));
        painter.line(Vec3::new(half_base, -height * 0.5, 0.0), Vec3::new(0.0, height * 0.5, 0.0));
    }
}

/// Renders X markers at departure points for pending transfers.
pub fn render_departure_markers(
    transfers: Query<&crate::transfer_vis::Transfer>,
    ships: Query<&ShipState>,
    sim_time: Res<crate::simulation::SimulationTime>,
    mut painter: ShapePainter,
) {
    for transfer in &transfers {
        // Only show marker if transfer hasn't started yet
        if transfer.departure_time <= sim_time.sim_time {
            continue;
        }

        // Only show if ship is still orbiting (not already transferring)
        if let Ok(state) = ships.get(transfer.ship) {
            if !matches!(state, ShipState::Orbiting { .. }) {
                continue;
            }
        }

        // Draw an X at the departure position
        let departure_pos = phys_to_visual(transfer.solution.departure_pos);

        painter.set_translation(departure_pos);
        painter.set_rotation(Quat::IDENTITY);
        painter.set_color(DEPARTURE_MARKER_COLOR);
        painter.thickness = 0.8;

        // Draw X
        let size = 2.5;
        painter.line(
            Vec3::new(-size, -size, 0.0),
            Vec3::new(size, size, 0.0),
        );
        painter.line(
            Vec3::new(-size, size, 0.0),
            Vec3::new(size, -size, 0.0),
        );
    }
}
