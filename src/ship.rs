//! Ship entity and transfer scheduling.
//!
//! Tracks player ships that can travel between celestial bodies,
//! managing delta-v budgets and scheduled transfers.

use std::collections::VecDeque;

use bevy::gizmos::GizmoAsset;
use bevy::prelude::*;
use bevy_vector_shapes::prelude::*;

use crate::simulation::SimulationTime;
use crate::transfer::{TransferSolution, propagate_kepler_full};
use crate::orbital_data::{Body, MU_SUN};
use crate::{phys_to_visual, transfer_vis};
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

/// A queued transfer leg for multi-hop journeys.
#[derive(Clone)]
pub struct QueuedTransfer {
    /// Target body for this leg
    pub target: Entity,
    /// Source body (where this leg departs from)
    pub source: Entity,
    /// Pre-computed transfer solution
    pub solution: TransferSolution,
    /// Absolute departure time (seconds since epoch)
    pub departure_time: f64,
}

/// Queue of pending transfer legs for a ship.
/// Allows planning multi-hop journeys (A → B → C).
#[derive(Component, Default)]
pub struct TransferQueue {
    /// Queued transfers, executed in order
    pub queued: VecDeque<QueuedTransfer>,
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
    transfers: Query<&transfer_vis::Transfer>,
    mut ships: Query<(&mut Ship, &mut ShipState)>,
    sim_time: Res<SimulationTime>,
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
/// Deducts arrival delta-v, transitions ship to Orbiting state,
/// and executes the next queued transfer if any.
pub fn check_ship_arrival(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    mut ships: Query<(Entity, &mut Ship, &mut ShipState, &mut TransferQueue)>,
    bodies: Query<&Body>,
    sim_time: Res<SimulationTime>,
) {
    for (ship_entity, mut ship, mut state, mut queue) in &mut ships {
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

                // Execute next queued transfer if any
                if let Some(next_transfer) = queue.queued.pop_front() {
                    let next_target_name = bodies
                        .get(next_transfer.target)
                        .map(|b| b.name.clone())
                        .unwrap_or_else(|_| "Unknown".to_string());

                    // Check if we have enough delta-v
                    let required_dv = next_transfer.solution.total_dv;
                    if ship.delta_v_remaining < required_dv {
                        warn!(
                            "Ship '{}' cannot execute queued transfer to {}: need {:.0} m/s, have {:.0} m/s. Clearing queue.",
                            ship.name, next_target_name, required_dv, ship.delta_v_remaining
                        );
                        queue.queued.clear();
                    } else {
                        info!(
                            "Ship '{}' executing queued transfer to {} (departure in {:.1} days)",
                            ship.name, next_target_name,
                            (next_transfer.departure_time - sim_time.sim_time) / 86400.0
                        );

                        // Spawn transfer visualization
                        transfer_vis::spawn_transfer_visualization(
                            &mut commands,
                            &mut gizmo_assets,
                            ship_entity,
                            next_transfer.source,
                            next_transfer.target,
                            &next_transfer.solution,
                            next_transfer.departure_time,
                        );
                    }
                }
            }
        }
    }
}

/// System to handle Enter key to execute the first queued transfer.
/// When the ship is orbiting and has queued transfers, pressing Enter schedules the first one.
/// The actual departure (state transition, delta-v deduction) happens in `execute_scheduled_transfers`.
pub fn execute_queue_on_enter(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut ships: Query<(Entity, &Ship, &ShipState, &mut TransferQueue), With<PlayerControlled>>,
    bodies: Query<&Body>,
) {
    if !keyboard.just_pressed(KeyCode::Enter) {
        return;
    }

    for (ship_entity, ship, state, mut queue) in &mut ships {
        // Only execute if ship is orbiting (not already transferring)
        let ShipState::Orbiting { .. } = state else {
            continue;
        };

        // Check if there's a queued transfer
        let Some(next_transfer) = queue.queued.pop_front() else {
            continue;
        };

        let target_name = bodies
            .get(next_transfer.target)
            .map(|b| b.name.clone())
            .unwrap_or_else(|_| "Unknown".to_string());

        let source_name = bodies
            .get(next_transfer.source)
            .map(|b| b.name.clone())
            .unwrap_or_else(|_| "Unknown".to_string());

        // Check if we have enough delta-v
        let required_dv = next_transfer.solution.total_dv;
        if ship.delta_v_remaining < required_dv {
            warn!(
                "Ship cannot execute queued transfer {} -> {}: need {:.0} m/s, have {:.0} m/s. Clearing queue.",
                source_name, target_name, required_dv, ship.delta_v_remaining
            );
            queue.queued.clear();
            continue;
        }

        info!(
            "Scheduling queued transfer {} -> {} (departure day {}, {} m/s)",
            source_name, target_name,
            (next_transfer.departure_time / 86400.0) as i32,
            next_transfer.solution.total_dv as i32
        );

        // Spawn transfer visualization - this creates the Transfer entity
        // which will be executed when departure time arrives (by execute_scheduled_transfers)
        transfer_vis::spawn_transfer_visualization(
            &mut commands,
            &mut gizmo_assets,
            ship_entity,
            next_transfer.source,
            next_transfer.target,
            &next_transfer.solution,
            next_transfer.departure_time,
        );
    }
}

/// System to handle N key to cancel/clear the transfer queue.
pub fn cancel_queue_on_n(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut ships: Query<&mut TransferQueue, With<PlayerControlled>>,
) {
    if !keyboard.just_pressed(KeyCode::KeyN) {
        return;
    }

    for mut queue in &mut ships {
        if !queue.queued.is_empty() {
            let count = queue.queued.len();
            queue.queued.clear();
            info!("Cancelled {} queued transfer(s)", count);
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
    sim_time: Res<SimulationTime>,
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
    transfers: Query<&transfer_vis::Transfer>,
    ships: Query<&ShipState>,
    sim_time: Res<SimulationTime>,
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

/// Queue waypoint marker color (cyan, dimmed)
const QUEUE_MARKER_COLOR: Color = Color::srgba(0.3, 0.8, 0.8, 0.7);

/// Queue arc color (dimmed orange)
const QUEUE_ARC_COLOR: Color = Color::srgba(1.0, 0.6, 0.2, 0.4);

/// Renders numbered waypoint markers at queued destination bodies.
pub fn render_queue_markers(
    ships: Query<&TransferQueue>,
    bodies: Query<&ComputedBody>,
    mut painter: ShapePainter,
) {
    for queue in &ships {
        for (index, queued) in queue.queued.iter().enumerate() {
            // Get target body position
            let Ok(computed) = bodies.get(queued.target) else {
                continue;
            };

            let pos = computed.position;

            // Draw a circle with number
            painter.set_translation(pos);
            painter.set_rotation(Quat::IDENTITY);
            painter.set_color(QUEUE_MARKER_COLOR);
            painter.thickness = 1.0;

            // Circle around the waypoint
            let radius = 8.0;
            painter.hollow = true;
            painter.circle(radius);

            // Draw the number (1-indexed) as simple lines
            // This is a hacky way to draw numbers, but works for 1-9
            let num = index + 1;
            let offset = Vec3::new(0.0, -2.0, 0.0);
            draw_number(&mut painter, num, pos + offset, 3.0);
        }
    }
}

/// Renders dimmed preview arcs for queued transfers.
pub fn render_queue_arcs(
    ships: Query<&TransferQueue>,
    mut painter: ShapePainter,
) {
    for queue in &ships {
        for queued in queue.queued.iter() {
            // Draw a simplified arc from source to target
            painter.set_color(QUEUE_ARC_COLOR);
            painter.thickness = 1.0;

            // Draw arc using the pre-computed solution
            let num_segments = 100;
            let tof = queued.solution.time_of_flight;

            for i in 0..num_segments {
                let t0 = (i as f64 / num_segments as f64) * tof;
                let t1 = ((i + 1) as f64 / num_segments as f64) * tof;

                if let (Some(pos0), Some(pos1)) = (
                    crate::transfer::propagate_kepler(
                        queued.solution.departure_pos,
                        queued.solution.departure_vel,
                        MU_SUN,
                        t0,
                    ),
                    crate::transfer::propagate_kepler(
                        queued.solution.departure_pos,
                        queued.solution.departure_vel,
                        MU_SUN,
                        t1,
                    ),
                ) {
                    let p0 = phys_to_visual(pos0);
                    let p1 = phys_to_visual(pos1);
                    painter.line(p0, p1);
                }
            }
        }
    }
}

/// Helper to draw a simple number using lines (1-9 only).
fn draw_number(painter: &mut ShapePainter, num: usize, center: Vec3, size: f32) {
    painter.set_translation(center);
    let h = size;
    let w = size * 0.6;

    match num {
        1 => {
            painter.line(Vec3::new(0.0, h/2.0, 0.0), Vec3::new(0.0, -h/2.0, 0.0));
        }
        2 => {
            painter.line(Vec3::new(-w/2.0, h/2.0, 0.0), Vec3::new(w/2.0, h/2.0, 0.0));
            painter.line(Vec3::new(w/2.0, h/2.0, 0.0), Vec3::new(w/2.0, 0.0, 0.0));
            painter.line(Vec3::new(w/2.0, 0.0, 0.0), Vec3::new(-w/2.0, 0.0, 0.0));
            painter.line(Vec3::new(-w/2.0, 0.0, 0.0), Vec3::new(-w/2.0, -h/2.0, 0.0));
            painter.line(Vec3::new(-w/2.0, -h/2.0, 0.0), Vec3::new(w/2.0, -h/2.0, 0.0));
        }
        3 => {
            painter.line(Vec3::new(-w/2.0, h/2.0, 0.0), Vec3::new(w/2.0, h/2.0, 0.0));
            painter.line(Vec3::new(w/2.0, h/2.0, 0.0), Vec3::new(w/2.0, -h/2.0, 0.0));
            painter.line(Vec3::new(w/2.0, -h/2.0, 0.0), Vec3::new(-w/2.0, -h/2.0, 0.0));
            painter.line(Vec3::new(-w/2.0, 0.0, 0.0), Vec3::new(w/2.0, 0.0, 0.0));
        }
        4 => {
            painter.line(Vec3::new(-w/2.0, h/2.0, 0.0), Vec3::new(-w/2.0, 0.0, 0.0));
            painter.line(Vec3::new(-w/2.0, 0.0, 0.0), Vec3::new(w/2.0, 0.0, 0.0));
            painter.line(Vec3::new(w/2.0, h/2.0, 0.0), Vec3::new(w/2.0, -h/2.0, 0.0));
        }
        5 => {
            painter.line(Vec3::new(w/2.0, h/2.0, 0.0), Vec3::new(-w/2.0, h/2.0, 0.0));
            painter.line(Vec3::new(-w/2.0, h/2.0, 0.0), Vec3::new(-w/2.0, 0.0, 0.0));
            painter.line(Vec3::new(-w/2.0, 0.0, 0.0), Vec3::new(w/2.0, 0.0, 0.0));
            painter.line(Vec3::new(w/2.0, 0.0, 0.0), Vec3::new(w/2.0, -h/2.0, 0.0));
            painter.line(Vec3::new(w/2.0, -h/2.0, 0.0), Vec3::new(-w/2.0, -h/2.0, 0.0));
        }
        6 => {
            painter.line(Vec3::new(w/2.0, h/2.0, 0.0), Vec3::new(-w/2.0, h/2.0, 0.0));
            painter.line(Vec3::new(-w/2.0, h/2.0, 0.0), Vec3::new(-w/2.0, -h/2.0, 0.0));
            painter.line(Vec3::new(-w/2.0, -h/2.0, 0.0), Vec3::new(w/2.0, -h/2.0, 0.0));
            painter.line(Vec3::new(w/2.0, -h/2.0, 0.0), Vec3::new(w/2.0, 0.0, 0.0));
            painter.line(Vec3::new(w/2.0, 0.0, 0.0), Vec3::new(-w/2.0, 0.0, 0.0));
        }
        7 => {
            painter.line(Vec3::new(-w/2.0, h/2.0, 0.0), Vec3::new(w/2.0, h/2.0, 0.0));
            painter.line(Vec3::new(w/2.0, h/2.0, 0.0), Vec3::new(w/2.0, -h/2.0, 0.0));
        }
        8 => {
            painter.line(Vec3::new(-w/2.0, h/2.0, 0.0), Vec3::new(w/2.0, h/2.0, 0.0));
            painter.line(Vec3::new(w/2.0, h/2.0, 0.0), Vec3::new(w/2.0, -h/2.0, 0.0));
            painter.line(Vec3::new(w/2.0, -h/2.0, 0.0), Vec3::new(-w/2.0, -h/2.0, 0.0));
            painter.line(Vec3::new(-w/2.0, -h/2.0, 0.0), Vec3::new(-w/2.0, h/2.0, 0.0));
            painter.line(Vec3::new(-w/2.0, 0.0, 0.0), Vec3::new(w/2.0, 0.0, 0.0));
        }
        9 => {
            painter.line(Vec3::new(-w/2.0, h/2.0, 0.0), Vec3::new(w/2.0, h/2.0, 0.0));
            painter.line(Vec3::new(w/2.0, h/2.0, 0.0), Vec3::new(w/2.0, -h/2.0, 0.0));
            painter.line(Vec3::new(-w/2.0, h/2.0, 0.0), Vec3::new(-w/2.0, 0.0, 0.0));
            painter.line(Vec3::new(-w/2.0, 0.0, 0.0), Vec3::new(w/2.0, 0.0, 0.0));
        }
        _ => {
            // For numbers > 9, just draw a dot
            painter.circle(size * 0.3);
        }
    }
}
