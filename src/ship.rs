//! Ship entity and transfer scheduling.
//!
//! Tracks player ships that can travel between celestial bodies,
//! managing delta-v budgets and scheduled transfers.

use std::collections::VecDeque;

use bevy::gizmos::GizmoAsset;
use bevy::prelude::*;
use bevy_vector_shapes::prelude::*;

use crate::ComputedBody;
use crate::orbital_data::{Body, MU_SUN};
use crate::simulation::SimulationTime;
use crate::transfer::{TransferSolution, propagate_kepler_full};
use crate::{phys_to_visual, transfer_vis};

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
/// Transfer data lives in TransferQueue.current when Transferring.
#[derive(Component)]
pub enum ShipState {
    /// Ship is at a celestial body
    Orbiting { body: Entity },
    /// Ship is in transit - transfer data in TransferQueue.current
    Transferring,
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
    /// Whether this transfer is committed (will execute) or just planned
    pub committed: bool,
}

/// Queue of pending transfer legs for a ship.
/// Allows planning multi-hop journeys (A → B → C).
#[derive(Component, Default)]
pub struct TransferQueue {
    /// Currently executing transfer (if ship is Transferring)
    pub current: Option<QueuedTransfer>,
    /// Queued transfers, executed in order
    pub queued: VecDeque<QueuedTransfer>,
}

// ============================================================================
// Systems
// ============================================================================

/// Executes scheduled transfers when their departure time arrives.
/// Moves the front of the queue to `current` and transitions to Transferring state.
pub fn execute_scheduled_transfers(
    mut ships: Query<(&mut Ship, &mut ShipState, &mut TransferQueue)>,
    sim_time: Res<SimulationTime>,
) {
    for (mut ship, mut state, mut queue) in &mut ships {
        // Only execute if ship is orbiting (not already transferring)
        if !matches!(*state, ShipState::Orbiting { .. }) {
            continue;
        }

        // Check if front of queue is committed and ready to depart
        let should_depart = queue.queued.front().map_or(false, |front| {
            front.committed && sim_time.sim_time >= front.departure_time
        });

        if !should_depart {
            continue;
        }

        // Move from queue to current
        let transfer = queue.queued.pop_front().unwrap();

        // Deduct delta-v (departure burn)
        let departure_dv = transfer.solution.departure_dv.norm();
        ship.delta_v_remaining -= departure_dv;

        info!(
            "Ship '{}' departing! dv spent: {:.0} m/s, remaining: {:.0} m/s",
            ship.name, departure_dv, ship.delta_v_remaining
        );

        // Set current and transition to Transferring
        queue.current = Some(transfer);
        *state = ShipState::Transferring;
    }
}

/// Checks if transferring ships have arrived at their destination.
/// Deducts arrival delta-v, clears `current`, and transitions to Orbiting state.
pub fn check_ship_arrival(
    mut ships: Query<(&mut Ship, &mut ShipState, &mut TransferQueue)>,
    bodies: Query<&Body>,
    sim_time: Res<SimulationTime>,
) {
    for (mut ship, mut state, mut queue) in &mut ships {
        // Only check if transferring
        if !matches!(*state, ShipState::Transferring) {
            continue;
        }

        // Get current transfer data
        let Some(ref current) = queue.current else {
            // Transferring but no current - shouldn't happen, recover by orbiting
            warn!("Ship in Transferring state but no current transfer!");
            *state = ShipState::Orbiting {
                body: Entity::PLACEHOLDER,
            };
            continue;
        };

        // Check if arrived
        let arrival_time = current.departure_time + current.solution.time_of_flight;
        if sim_time.sim_time < arrival_time {
            continue;
        }

        // Deduct arrival burn delta-v
        let arrival_dv = current.solution.arrival_dv.norm();
        ship.delta_v_remaining -= arrival_dv;

        // Get target for logging and state transition
        let target = current.target;
        let target_name = bodies
            .get(target)
            .map(|b| b.name.clone())
            .unwrap_or_else(|_| "Unknown".to_string());

        info!(
            "Ship '{}' arrived at {}! dv spent: {:.0} m/s, remaining: {:.0} m/s",
            ship.name, target_name, arrival_dv, ship.delta_v_remaining
        );

        // Clear current and transition to orbiting
        queue.current = None;
        *state = ShipState::Orbiting { body: target };
    }
}

/// Sync system: ensures Transfer entities match current + committed queue entries.
/// - Despawns Transfer entities that don't have a matching entry
/// - Spawns Transfer entities for entries that don't have one
pub fn sync_transfer_entities(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    ships: Query<(Entity, &TransferQueue)>,
    transfers: Query<(Entity, &transfer_vis::Transfer)>,
    bodies: Query<&crate::orbital_data::Body>,
) {
    for (ship_entity, queue) in &ships {
        // Collect all transfers that should have visualization:
        // current (if any) + committed queue entries
        let mut active_transfers: Vec<&QueuedTransfer> = Vec::new();
        if let Some(ref current) = queue.current {
            active_transfers.push(current);
        }
        active_transfers.extend(queue.queued.iter().filter(|t| t.committed));

        // Debug: log what we're tracking
        if !active_transfers.is_empty() {
            let names: Vec<String> = active_transfers
                .iter()
                .map(|t| {
                    let src = bodies.get(t.source).map(|b| b.name.as_str()).unwrap_or("?");
                    let tgt = bodies.get(t.target).map(|b| b.name.as_str()).unwrap_or("?");
                    format!("{}->{}", src, tgt)
                })
                .collect();
            debug!(
                "sync_transfer_entities: active transfers = [{}]",
                names.join(", ")
            );
        }

        // Find Transfer entities for this ship
        let ship_transfers: Vec<_> = transfers
            .iter()
            .filter(|(_, t)| t.ship == ship_entity)
            .collect();

        // Despawn Transfer entities that don't match any active transfer
        for (transfer_entity, transfer) in &ship_transfers {
            let has_match = active_transfers.iter().any(|q| {
                q.target == transfer.target
                    && (q.departure_time - transfer.departure_time).abs() < 1.0
            });
            if !has_match {
                commands.entity(*transfer_entity).despawn();
            }
        }

        // Spawn Transfer entities for active entries that don't have one
        for queued in &active_transfers {
            let has_entity = ship_transfers.iter().any(|(_, t)| {
                t.target == queued.target && (t.departure_time - queued.departure_time).abs() < 1.0
            });
            if !has_entity {
                transfer_vis::spawn_transfer_visualization(
                    &mut commands,
                    &mut gizmo_assets,
                    ship_entity,
                    queued.source,
                    queued.target,
                    &queued.solution,
                    queued.departure_time,
                );
            }
        }
    }
}

/// System to handle Enter key to commit the entire transfer queue.
/// Just marks all queued transfers as committed - sync_transfer_entities handles visualization.
pub fn execute_queue_on_enter(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut ships: Query<(&Ship, &mut TransferQueue), With<PlayerControlled>>,
    bodies: Query<&Body>,
) {
    if !keyboard.just_pressed(KeyCode::Enter) {
        return;
    }

    for (ship, mut queue) in &mut ships {
        // Check if there are any uncommitted transfers
        let has_uncommitted = queue.queued.iter().any(|t| !t.committed);
        if !has_uncommitted {
            continue;
        }

        // Check total delta-v for all uncommitted transfers
        let uncommitted_dv: f64 = queue
            .queued
            .iter()
            .filter(|t| !t.committed)
            .map(|t| t.solution.total_dv)
            .sum();

        if ship.delta_v_remaining < uncommitted_dv {
            warn!(
                "Insufficient delta-v to commit queue: need {:.0} m/s, have {:.0} m/s",
                uncommitted_dv, ship.delta_v_remaining
            );
            continue;
        }

        // Commit all transfers - sync_transfer_entities will spawn visualizations
        for transfer in queue.queued.iter_mut() {
            if !transfer.committed {
                let target_name = bodies
                    .get(transfer.target)
                    .map(|b| b.name.as_str())
                    .unwrap_or("?");
                let source_name = bodies
                    .get(transfer.source)
                    .map(|b| b.name.as_str())
                    .unwrap_or("?");

                info!(
                    "Committing transfer {} -> {} (departure day {}, {} m/s)",
                    source_name,
                    target_name,
                    (transfer.departure_time / 86400.0) as i32,
                    transfer.solution.total_dv as i32
                );

                transfer.committed = true;
            }
        }
    }
}

/// Expires uncommitted queue entries whose departure time has passed.
/// Committed transfers are handled separately (they execute on departure).
pub fn expire_uncommitted_queue(
    mut ships: Query<&mut TransferQueue>,
    bodies: Query<&Body>,
    sim_time: Res<SimulationTime>,
) {
    for mut queue in &mut ships {
        // Collect indices of expired uncommitted entries (iterate in reverse for safe removal)
        let mut expired_count = 0;
        queue.queued.retain(|transfer| {
            // Keep committed transfers (they execute normally)
            if transfer.committed {
                return true;
            }
            // Keep if departure time hasn't passed
            if sim_time.sim_time < transfer.departure_time {
                return true;
            }
            // Expired - remove it
            let target_name = bodies
                .get(transfer.target)
                .map(|b| b.name.as_str())
                .unwrap_or("?");
            info!(
                "Expired uncommitted transfer to {} (departure day {} has passed)",
                target_name,
                (transfer.departure_time / 86400.0) as i32
            );
            expired_count += 1;
            false
        });

        if expired_count > 0 {
            info!("Removed {} expired uncommitted transfer(s)", expired_count);
        }
    }
}

/// System to handle N key to cancel transfers.
/// Just pops from the back of the queue - sync_transfer_entities handles despawning.
pub fn cancel_queue_on_n(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut ships: Query<&mut TransferQueue, With<PlayerControlled>>,
    bodies: Query<&crate::orbital_data::Body>,
) {
    if !keyboard.just_pressed(KeyCode::KeyN) {
        return;
    }

    for mut queue in &mut ships {
        // Pop the last transfer from the queue
        if let Some(cancelled) = queue.queued.pop_back() {
            let target_name = bodies
                .get(cancelled.target)
                .map(|b| b.name.as_str())
                .unwrap_or("?");

            info!(
                "Cancelled {} transfer to {} ({} remaining in queue)",
                if cancelled.committed {
                    "committed"
                } else {
                    "queued"
                },
                target_name,
                queue.queued.len()
            );
        } else {
            info!("No transfers to cancel");
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
/// - When transferring: positioned along transfer arc (reads from queue.current)
pub fn render_ship(
    ships: Query<(&Ship, &ShipState, &TransferQueue)>,
    bodies: Query<&ComputedBody>,
    sim_time: Res<SimulationTime>,
    mut painter: ShapePainter,
) {
    for (_ship, state, queue) in &ships {
        let (position, velocity_dir) = match state {
            ShipState::Orbiting { body } => {
                // Position at body's current location
                let body_pos = bodies.get(*body).map(|c| c.position).unwrap_or(Vec3::ZERO);
                // Offset slightly so ship is visible next to body
                let offset_pos = body_pos + Vec3::new(SHIP_SIZE * 1.5, 0.0, 0.0);
                // Velocity direction pointing "forward" in orbit (tangent)
                (offset_pos, Vec3::new(0.0, 1.0, 0.0))
            }
            ShipState::Transferring => {
                // Get transfer data from queue.current
                let Some(ref current) = queue.current else {
                    continue;
                };

                // Propagate position along transfer arc
                let elapsed = sim_time.sim_time - current.departure_time;
                if elapsed < 0.0 {
                    // Before departure - shouldn't happen but handle gracefully
                    continue;
                }

                // Get current position and velocity on transfer orbit
                if let Some((r_vec, v_vec)) = propagate_kepler_full(
                    current.solution.departure_pos,
                    current.solution.departure_vel,
                    MU_SUN,
                    elapsed,
                ) {
                    let pos = phys_to_visual(r_vec);

                    // Use actual velocity direction (project to 2D XY plane)
                    let vel_dir =
                        Vec3::new(v_vec.x as f32, v_vec.y as f32, 0.0).normalize_or_zero();

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
        painter.line(
            Vec3::new(0.0, height * 0.5, 0.0),
            Vec3::new(-half_base, -height * 0.5, 0.0),
        );
        painter.line(
            Vec3::new(-half_base, -height * 0.5, 0.0),
            Vec3::new(half_base, -height * 0.5, 0.0),
        );
        painter.line(
            Vec3::new(half_base, -height * 0.5, 0.0),
            Vec3::new(0.0, height * 0.5, 0.0),
        );
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
        painter.line(Vec3::new(-size, -size, 0.0), Vec3::new(size, size, 0.0));
        painter.line(Vec3::new(-size, size, 0.0), Vec3::new(size, -size, 0.0));
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
pub fn render_queue_arcs(ships: Query<&TransferQueue>, mut painter: ShapePainter) {
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
            painter.line(Vec3::new(0.0, h / 2.0, 0.0), Vec3::new(0.0, -h / 2.0, 0.0));
        }
        2 => {
            painter.line(
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
                Vec3::new(w / 2.0, h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(w / 2.0, h / 2.0, 0.0),
                Vec3::new(w / 2.0, 0.0, 0.0),
            );
            painter.line(Vec3::new(w / 2.0, 0.0, 0.0), Vec3::new(-w / 2.0, 0.0, 0.0));
            painter.line(
                Vec3::new(-w / 2.0, 0.0, 0.0),
                Vec3::new(-w / 2.0, -h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(-w / 2.0, -h / 2.0, 0.0),
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
            );
        }
        3 => {
            painter.line(
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
                Vec3::new(w / 2.0, h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(w / 2.0, h / 2.0, 0.0),
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
                Vec3::new(-w / 2.0, -h / 2.0, 0.0),
            );
            painter.line(Vec3::new(-w / 2.0, 0.0, 0.0), Vec3::new(w / 2.0, 0.0, 0.0));
        }
        4 => {
            painter.line(
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
                Vec3::new(-w / 2.0, 0.0, 0.0),
            );
            painter.line(Vec3::new(-w / 2.0, 0.0, 0.0), Vec3::new(w / 2.0, 0.0, 0.0));
            painter.line(
                Vec3::new(w / 2.0, h / 2.0, 0.0),
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
            );
        }
        5 => {
            painter.line(
                Vec3::new(w / 2.0, h / 2.0, 0.0),
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
                Vec3::new(-w / 2.0, 0.0, 0.0),
            );
            painter.line(Vec3::new(-w / 2.0, 0.0, 0.0), Vec3::new(w / 2.0, 0.0, 0.0));
            painter.line(
                Vec3::new(w / 2.0, 0.0, 0.0),
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
                Vec3::new(-w / 2.0, -h / 2.0, 0.0),
            );
        }
        6 => {
            painter.line(
                Vec3::new(w / 2.0, h / 2.0, 0.0),
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
                Vec3::new(-w / 2.0, -h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(-w / 2.0, -h / 2.0, 0.0),
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
                Vec3::new(w / 2.0, 0.0, 0.0),
            );
            painter.line(Vec3::new(w / 2.0, 0.0, 0.0), Vec3::new(-w / 2.0, 0.0, 0.0));
        }
        7 => {
            painter.line(
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
                Vec3::new(w / 2.0, h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(w / 2.0, h / 2.0, 0.0),
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
            );
        }
        8 => {
            painter.line(
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
                Vec3::new(w / 2.0, h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(w / 2.0, h / 2.0, 0.0),
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
                Vec3::new(-w / 2.0, -h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(-w / 2.0, -h / 2.0, 0.0),
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
            );
            painter.line(Vec3::new(-w / 2.0, 0.0, 0.0), Vec3::new(w / 2.0, 0.0, 0.0));
        }
        9 => {
            painter.line(
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
                Vec3::new(w / 2.0, h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(w / 2.0, h / 2.0, 0.0),
                Vec3::new(w / 2.0, -h / 2.0, 0.0),
            );
            painter.line(
                Vec3::new(-w / 2.0, h / 2.0, 0.0),
                Vec3::new(-w / 2.0, 0.0, 0.0),
            );
            painter.line(Vec3::new(-w / 2.0, 0.0, 0.0), Vec3::new(w / 2.0, 0.0, 0.0));
        }
        _ => {
            // For numbers > 9, just draw a dot
            painter.circle(size * 0.3);
        }
    }
}
