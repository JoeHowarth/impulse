//! Ship entity and flight planning.
//!
//! Data model:
//! - `ShipLocation`: Where the ship is (AtBody or InTransit)
//! - `FlightPlan`: Ordered list of planned legs with committed_count boundary
//! - `PlannedLeg`: Minimal data (target + timing), source derived from position
//!
//! Key invariants:
//! - `committed_count` legs are locked and will execute
//! - Uncommitted legs expire when departure_day passes
//! - Solutions looked up from cache, not stored in legs

use std::collections::VecDeque;

use bevy::gizmos::GizmoAsset;
use bevy::prelude::*;
use bevy_vector_shapes::prelude::*;

use crate::ComputedBody;
use crate::orbital_data::{Body, MU_SUN};
use crate::simulation::SimulationTime;
use crate::transfer::{TransferSolution, propagate_kepler_full};
use crate::transfer_cache::TransferCache;
use crate::{phys_to_visual, transfer_vis};

// ============================================================================
// Components
// ============================================================================

/// Marker for the player-controlled ship.
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

/// Ship's current location - either at a body or in transit.
#[derive(Component)]
pub enum ShipLocation {
    /// Ship is at a celestial body
    AtBody(Entity),
    /// Ship is in transit between bodies
    InTransit {
        target: Entity,
        solution: TransferSolution,
        departure_time: f64,
    },
}

impl ShipLocation {
    /// Where the ship is or will be (current body or transit destination)
    pub fn effective_body(&self) -> Entity {
        match self {
            ShipLocation::AtBody(e) => *e,
            ShipLocation::InTransit { target, .. } => *target,
        }
    }

    /// When ship arrives at effective_body (current time if AtBody)
    pub fn arrival_time(&self, current_time: f64) -> f64 {
        match self {
            ShipLocation::AtBody(_) => current_time,
            ShipLocation::InTransit {
                departure_time,
                solution,
                ..
            } => departure_time + solution.time_of_flight,
        }
    }
}

/// A planned transfer leg - minimal data, source derived from position.
#[derive(Clone)]
pub struct PlannedLeg {
    /// Target body for this leg
    pub target: Entity,
    /// Departure day (user's chosen window)
    pub departure_day: i32,
    /// Time of flight in days (from solution at planning time)
    pub tof_days: i32,
}

/// Flight plan for multi-hop journeys.
/// First `committed_count` legs are locked; rest are tentative.
#[derive(Component, Default)]
pub struct FlightPlan {
    /// Ordered list of planned legs
    pub legs: VecDeque<PlannedLeg>,
    /// Number of committed (locked) legs from front
    pub committed_count: usize,
}

// ============================================================================
// Derived Data Helpers
// ============================================================================

/// Returns the source body for a leg at given index.
/// Leg 0's source is ship's effective body; others chain from previous target.
pub fn leg_source(location: &ShipLocation, plan: &FlightPlan, leg_index: usize) -> Entity {
    if leg_index == 0 {
        location.effective_body()
    } else {
        plan.legs[leg_index - 1].target
    }
}

/// Returns the base day (arrival at source) for a leg at given index.
/// Leg 0 uses ship's arrival time; others chain from previous leg's arrival.
pub fn leg_base_day(
    location: &ShipLocation,
    plan: &FlightPlan,
    leg_index: usize,
    current_day: i32,
) -> i32 {
    if leg_index == 0 {
        match location {
            ShipLocation::AtBody(_) => current_day,
            ShipLocation::InTransit {
                departure_time,
                solution,
                ..
            } => ((departure_time + solution.time_of_flight) / 86400.0).floor() as i32,
        }
    } else {
        let prev = &plan.legs[leg_index - 1];
        prev.departure_day + prev.tof_days
    }
}

/// Looks up the exact solution for a leg from the cache.
/// Returns None if not cached (bug if leg is committed).
pub fn leg_solution<'a>(
    cache: &'a TransferCache,
    source: Entity,
    leg: &PlannedLeg,
) -> Option<&'a TransferSolution> {
    cache
        .solutions
        .get(&(source, leg.target, leg.departure_day, leg.tof_days))
}

// ============================================================================
// Systems
// ============================================================================

/// Expires uncommitted legs whose departure_day has passed.
/// Committed legs are never expired (execute_departure handles them).
pub fn expire_stale_legs(mut ships: Query<&mut FlightPlan>, sim_time: Res<SimulationTime>) {
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    for mut plan in &mut ships {
        // Assert: committed legs should never be stale (execute_departure runs first)
        debug_assert!(
            plan.legs
                .iter()
                .take(plan.committed_count)
                .all(|leg| leg.departure_day >= current_day),
            "Committed leg has departure_day in the past - system ordering bug?"
        );

        // Only expire uncommitted legs (index >= committed_count)
        let committed = plan.committed_count;
        let before_len = plan.legs.len();

        plan.legs = plan
            .legs
            .iter()
            .enumerate()
            .filter(|(i, leg)| *i < committed || leg.departure_day >= current_day)
            .map(|(_, leg)| leg.clone())
            .collect();

        let expired = before_len - plan.legs.len();
        if expired > 0 {
            info!("Expired {} uncommitted leg(s)", expired);
        }
    }
}

/// Executes departure when a committed leg's departure day arrives.
/// Looks up solution from cache, deducts delta-v, transitions to InTransit.
pub fn execute_departure(
    mut ships: Query<(&mut Ship, &mut ShipLocation, &mut FlightPlan)>,
    cache: Res<TransferCache>,
    bodies: Query<&Body>,
    sim_time: Res<SimulationTime>,
) {
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    for (mut ship, mut location, mut plan) in &mut ships {
        // Only if at body and have committed leg
        let ShipLocation::AtBody(current_body) = *location else {
            continue;
        };
        if plan.committed_count == 0 || plan.legs.is_empty() {
            continue;
        }

        let leg = &plan.legs[0];
        if current_day < leg.departure_day {
            continue;
        }

        // Look up solution from cache
        let Some(solution) = leg_solution(&cache, current_body, leg) else {
            let target_name = bodies
                .get(leg.target)
                .map(|b| b.name.as_str())
                .unwrap_or("?");
            warn!(
                "No cached solution for committed leg to {} - cannot depart!",
                target_name
            );
            continue;
        };

        // Deduct departure delta-v
        let departure_dv = solution.departure_dv.norm();
        ship.delta_v_remaining -= departure_dv;

        let target_name = bodies
            .get(leg.target)
            .map(|b| b.name.as_str())
            .unwrap_or("?");
        info!(
            "Ship '{}' departing to {}! dv spent: {:.0} m/s, remaining: {:.0} m/s",
            ship.name, target_name, departure_dv, ship.delta_v_remaining
        );

        // Transition to InTransit
        *location = ShipLocation::InTransit {
            target: leg.target,
            solution: solution.clone(),
            departure_time: leg.departure_day as f64 * 86400.0,
        };

        // Remove leg from plan
        plan.legs.pop_front();
        plan.committed_count -= 1;
    }
}

/// Checks if ship has arrived at destination.
/// Deducts arrival delta-v, transitions to AtBody.
pub fn check_arrival(
    mut ships: Query<(&mut Ship, &mut ShipLocation)>,
    bodies: Query<&Body>,
    sim_time: Res<SimulationTime>,
) {
    for (mut ship, mut location) in &mut ships {
        let ShipLocation::InTransit {
            target,
            solution,
            departure_time,
        } = &*location
        else {
            continue;
        };

        let arrival_time = departure_time + solution.time_of_flight;
        if sim_time.sim_time < arrival_time {
            continue;
        }

        // Deduct arrival delta-v
        let arrival_dv = solution.arrival_dv.norm();
        ship.delta_v_remaining -= arrival_dv;

        let target_name = bodies
            .get(*target)
            .map(|b| b.name.as_str())
            .unwrap_or("?");
        info!(
            "Ship '{}' arrived at {}! dv spent: {:.0} m/s, remaining: {:.0} m/s",
            ship.name, target_name, arrival_dv, ship.delta_v_remaining
        );

        // Transition to AtBody
        *location = ShipLocation::AtBody(*target);
    }
}

/// Commits all uncommitted legs when Enter is pressed.
pub fn commit_plan(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut ships: Query<(&Ship, &ShipLocation, &mut FlightPlan), With<PlayerControlled>>,
    cache: Res<TransferCache>,
    bodies: Query<&Body>,
) {
    if !keyboard.just_pressed(KeyCode::Enter) {
        return;
    }

    for (ship, location, mut plan) in &mut ships {
        // Check if there are uncommitted legs
        if plan.committed_count >= plan.legs.len() {
            continue;
        }

        // Sum delta-v for uncommitted legs
        let uncommitted_dv: f64 = plan
            .legs
            .iter()
            .enumerate()
            .skip(plan.committed_count)
            .filter_map(|(i, leg)| {
                let source = leg_source(location, &plan, i);
                leg_solution(&cache, source, leg).map(|s| s.total_dv)
            })
            .sum();

        if ship.delta_v_remaining < uncommitted_dv {
            warn!(
                "Insufficient delta-v to commit plan: need {:.0} m/s, have {:.0} m/s",
                uncommitted_dv, ship.delta_v_remaining
            );
            continue;
        }

        // Log what we're committing
        for i in plan.committed_count..plan.legs.len() {
            let leg = &plan.legs[i];
            let source = leg_source(location, &plan, i);
            let source_name = bodies.get(source).map(|b| b.name.as_str()).unwrap_or("?");
            let target_name = bodies
                .get(leg.target)
                .map(|b| b.name.as_str())
                .unwrap_or("?");

            if let Some(solution) = leg_solution(&cache, source, leg) {
                info!(
                    "Committing leg {} -> {} (day {}, {:.0} m/s)",
                    source_name, target_name, leg.departure_day, solution.total_dv
                );
            }
        }

        // Commit all
        plan.committed_count = plan.legs.len();
    }
}

/// Cancels the last leg when N is pressed.
pub fn cancel_last_leg(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut ships: Query<&mut FlightPlan, With<PlayerControlled>>,
    bodies: Query<&Body>,
) {
    if !keyboard.just_pressed(KeyCode::KeyN) {
        return;
    }

    for mut plan in &mut ships {
        if let Some(cancelled) = plan.legs.pop_back() {
            // Adjust committed_count if we removed a committed leg
            if plan.committed_count > plan.legs.len() {
                plan.committed_count = plan.legs.len();
            }

            let target_name = bodies
                .get(cancelled.target)
                .map(|b| b.name.as_str())
                .unwrap_or("?");
            let was_committed = plan.legs.len() < plan.committed_count;

            info!(
                "Cancelled {} leg to {} ({} remaining)",
                if was_committed { "committed" } else { "planned" },
                target_name,
                plan.legs.len()
            );
        } else {
            info!("No legs to cancel");
        }
    }
}

/// Syncs Transfer visualization entities to match ShipLocation + committed legs.
/// - InTransit -> one Transfer for active flight
/// - Committed legs -> one Transfer each for future arcs
pub fn sync_transfer_entities(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    ships: Query<(Entity, &ShipLocation, &FlightPlan)>,
    cache: Res<TransferCache>,
    transfers: Query<(Entity, &transfer_vis::Transfer)>,
    bodies: Query<&Body>,
) {
    for (ship_entity, location, plan) in &ships {
        // Build list of (source, target, solution, departure_time) for active visualizations
        let mut active: Vec<(Entity, Entity, &TransferSolution, f64)> = Vec::new();

        // Add active transfer if InTransit
        if let ShipLocation::InTransit {
            target,
            solution,
            departure_time,
        } = location
        {
            // Source for active transfer is where we departed from
            // We don't store it, but we can get the body the ship was at
            // For now, use PLACEHOLDER - the visualization doesn't need source entity
            active.push((Entity::PLACEHOLDER, *target, solution, *departure_time));
        }

        // Add committed future legs
        for i in 0..plan.committed_count.min(plan.legs.len()) {
            let leg = &plan.legs[i];
            let source = leg_source(location, plan, i);
            if let Some(solution) = leg_solution(&cache, source, leg) {
                let departure_time = leg.departure_day as f64 * 86400.0;
                active.push((source, leg.target, solution, departure_time));
            }
        }

        // Find existing Transfer entities for this ship
        let ship_transfers: Vec<_> = transfers
            .iter()
            .filter(|(_, t)| t.ship == ship_entity)
            .collect();

        // Despawn entities that don't match any active transfer
        for (transfer_entity, transfer) in &ship_transfers {
            let has_match = active.iter().any(|(_, target, _, dep_time)| {
                *target == transfer.target && (*dep_time - transfer.departure_time).abs() < 1.0
            });
            if !has_match {
                commands.entity(*transfer_entity).despawn();
            }
        }

        // Spawn entities for active transfers that don't have one
        for (source, target, solution, departure_time) in &active {
            let has_entity = ship_transfers.iter().any(|(_, t)| {
                t.target == *target && (t.departure_time - *departure_time).abs() < 1.0
            });
            if !has_entity {
                transfer_vis::spawn_transfer_visualization(
                    &mut commands,
                    &mut gizmo_assets,
                    ship_entity,
                    *source,
                    *target,
                    solution,
                    *departure_time,
                );
            }
        }

        // Debug logging
        if !active.is_empty() {
            let names: Vec<String> = active
                .iter()
                .map(|(src, tgt, _, _)| {
                    let src_name = bodies.get(*src).map(|b| b.name.as_str()).unwrap_or("?");
                    let tgt_name = bodies.get(*tgt).map(|b| b.name.as_str()).unwrap_or("?");
                    format!("{}->{}", src_name, tgt_name)
                })
                .collect();
            debug!("sync_transfer_entities: [{}]", names.join(", "));
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
/// - AtBody: positioned at body location
/// - InTransit: positioned along transfer arc
pub fn render_ship(
    ships: Query<(&Ship, &ShipLocation)>,
    bodies: Query<&ComputedBody>,
    sim_time: Res<SimulationTime>,
    mut painter: ShapePainter,
) {
    for (_ship, location) in &ships {
        let (position, velocity_dir) = match location {
            ShipLocation::AtBody(body) => {
                // Position at body's current location
                let body_pos = bodies.get(*body).map(|c| c.position).unwrap_or(Vec3::ZERO);
                // Offset slightly so ship is visible next to body
                let offset_pos = body_pos + Vec3::new(SHIP_SIZE * 1.5, 0.0, 0.0);
                // Velocity direction pointing "forward" in orbit (tangent)
                (offset_pos, Vec3::new(0.0, 1.0, 0.0))
            }
            ShipLocation::InTransit {
                solution,
                departure_time,
                ..
            } => {
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
    ships: Query<&ShipLocation>,
    sim_time: Res<SimulationTime>,
    mut painter: ShapePainter,
) {
    for transfer in &transfers {
        // Only show marker if transfer hasn't started yet
        if transfer.departure_time <= sim_time.sim_time {
            continue;
        }

        // Only show if ship is still at body (not already transferring)
        if let Ok(location) = ships.get(transfer.ship) {
            if !matches!(location, ShipLocation::AtBody(_)) {
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
pub fn render_plan_markers(
    ships: Query<&FlightPlan>,
    bodies: Query<&ComputedBody>,
    mut painter: ShapePainter,
) {
    for plan in &ships {
        for (index, leg) in plan.legs.iter().enumerate() {
            // Get target body position
            let Ok(computed) = bodies.get(leg.target) else {
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
            let num = index + 1;
            let offset = Vec3::new(0.0, -2.0, 0.0);
            draw_number(&mut painter, num, pos + offset, 3.0);
        }
    }
}

/// Renders dimmed preview arcs for uncommitted legs (not yet locked in).
pub fn render_plan_arcs(
    ships: Query<(&ShipLocation, &FlightPlan)>,
    cache: Res<TransferCache>,
    mut painter: ShapePainter,
) {
    for (location, plan) in &ships {
        // Only render uncommitted legs (index >= committed_count)
        for i in plan.committed_count..plan.legs.len() {
            let leg = &plan.legs[i];
            let source = leg_source(location, plan, i);

            // Look up solution from cache
            let Some(solution) = leg_solution(&cache, source, leg) else {
                continue;
            };

            // Draw a simplified arc
            painter.set_color(QUEUE_ARC_COLOR);
            painter.thickness = 1.0;

            let num_segments = 100;
            let tof = solution.time_of_flight;

            for j in 0..num_segments {
                let t0 = (j as f64 / num_segments as f64) * tof;
                let t1 = ((j + 1) as f64 / num_segments as f64) * tof;

                if let (Some(pos0), Some(pos1)) = (
                    crate::transfer::propagate_kepler(
                        solution.departure_pos,
                        solution.departure_vel,
                        MU_SUN,
                        t0,
                    ),
                    crate::transfer::propagate_kepler(
                        solution.departure_pos,
                        solution.departure_vel,
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
