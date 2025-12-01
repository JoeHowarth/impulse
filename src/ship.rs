//! Fleet entities and flight planning.
//!
//! Data model:
//! - `FleetLocation`: Where the fleet is (AtBody or InTransit)
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
use crate::transfer_lut::TransferLut;
use crate::{phys_to_visual, transfer_vis};

// ============================================================================
// Components
// ============================================================================

/// Faction that a fleet belongs to.
#[derive(Component, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Faction {
    #[default]
    Player,
    Enemy,
}

/// An individual ship entity within a fleet.
/// Ships are spawned as children of their Fleet entity.
/// For now this is a simple marker - stats (ammo, damage) will be added later.
#[derive(Component)]
pub struct LogicalShip;

/// A fleet of ships that travel together between celestial bodies.
#[derive(Component)]
pub struct Fleet {
    /// Remaining delta-v budget in m/s (same for all ships in fleet)
    pub delta_v_remaining: f64,
    /// Fleet name for display
    pub name: String,
}

/// Counts LogicalShip children of a fleet entity.
pub fn ship_count(
    fleet_entity: Entity,
    children: &Query<&Children>,
    ships: &Query<&LogicalShip>,
) -> u32 {
    children
        .get(fleet_entity)
        .map(|c| c.iter().filter(|e| ships.contains(*e)).count() as u32)
        .unwrap_or(0)
}

/// Marker for the currently selected fleet.
#[derive(Component)]
pub struct Selected;

/// Computed visual position for a fleet.
/// Updated each frame by update_fleet_positions system.
#[derive(Component)]
pub struct ComputedFleetPosition {
    pub position: Vec3,
    pub velocity_dir: Vec3,
}

/// Resource to track victory state
#[derive(Resource, Default)]
pub struct VictoryState {
    /// Whether all objectives are satisfied
    pub victory_achieved: bool,
    /// Time when victory was achieved (for display)
    pub victory_time: Option<f64>,
}

/// Resource to track active tactical combat state
#[derive(Resource, Default)]
pub struct CombatState {
    /// Whether tactical combat is currently active
    pub active: bool,
    /// The tactical arena entity (if spawned)
    pub arena: Option<Entity>,
    /// The body where combat is occurring
    pub body: Option<Entity>,
    /// Player fleets involved in combat
    pub player_fleets: Vec<Entity>,
    /// Enemy fleets involved in combat
    pub enemy_fleets: Vec<Entity>,
}

/// Fleet's current location - either at a body or in transit.
#[derive(Component)]
pub enum FleetLocation {
    /// Fleet is at a celestial body
    AtBody(Entity),
    /// Fleet is in transit between bodies
    InTransit {
        target: Entity,
        solution: TransferSolution,
        departure_time: f64,
    },
}

impl FleetLocation {
    /// Where the fleet is or will be (current body or transit destination)
    pub fn effective_body(&self) -> Entity {
        match self {
            FleetLocation::AtBody(e) => *e,
            FleetLocation::InTransit { target, .. } => *target,
        }
    }

    /// When ship arrives at effective_body (current time if AtBody)
    #[allow(dead_code)]
    pub fn arrival_time(&self, current_time: f64) -> f64 {
        match self {
            FleetLocation::AtBody(_) => current_time,
            FleetLocation::InTransit {
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
pub fn leg_source(location: &FleetLocation, plan: &FlightPlan, leg_index: usize) -> Entity {
    if leg_index == 0 {
        location.effective_body()
    } else {
        plan.legs[leg_index - 1].target
    }
}

/// Returns the base day (arrival at source) for a leg at given index.
/// Leg 0 uses ship's arrival time; others chain from previous leg's arrival.
pub fn leg_base_day(
    location: &FleetLocation,
    plan: &FlightPlan,
    leg_index: usize,
    current_day: i32,
) -> i32 {
    if leg_index == 0 {
        match location {
            FleetLocation::AtBody(_) => current_day,
            FleetLocation::InTransit {
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
/// Looks up solution from LUT, deducts delta-v, transitions to InTransit.
pub fn execute_departure(
    mut fleets: Query<(&mut Fleet, &mut FleetLocation, &mut FlightPlan)>,
    lut: Res<TransferLut>,
    bodies: Query<&Body>,
    sim_time: Res<SimulationTime>,
) {
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    for (mut fleet, mut location, mut plan) in &mut fleets {
        // Only if at body and have committed leg
        let FleetLocation::AtBody(current_body) = *location else {
            continue;
        };
        if plan.committed_count == 0 || plan.legs.is_empty() {
            continue;
        }

        let leg = &plan.legs[0];
        if current_day < leg.departure_day {
            continue;
        }

        // Get orbital elements for lookup
        let (Ok(source_body), Ok(target_body)) = (bodies.get(current_body), bodies.get(leg.target)) else {
            warn!("Cannot get body data for departure");
            continue;
        };

        // Look up solution from LUT
        let Some(solution) = lut.get_transfer(
            current_body,
            leg.target,
            &source_body.orbital_elements,
            &target_body.orbital_elements,
            leg.departure_day,
            leg.tof_days,
        ) else {
            warn!(
                "No LUT solution for committed leg to {} - cannot depart!",
                target_body.name
            );
            continue;
        };

        // Deduct departure delta-v
        let departure_dv = solution.departure_dv.norm();
        fleet.delta_v_remaining -= departure_dv;

        info!(
            "Fleet '{}' departing to {}! dv spent: {:.0} m/s, remaining: {:.0} m/s",
            fleet.name, target_body.name, departure_dv, fleet.delta_v_remaining
        );

        // Transition to InTransit
        *location = FleetLocation::InTransit {
            target: leg.target,
            solution,
            departure_time: leg.departure_day as f64 * 86400.0,
        };

        // Remove leg from plan
        plan.legs.pop_front();
        plan.committed_count -= 1;
    }
}

/// Checks if fleet has arrived at destination.
/// Deducts arrival delta-v, transitions to AtBody.
pub fn check_arrival(
    mut fleets: Query<(&mut Fleet, &mut FleetLocation)>,
    bodies: Query<&Body>,
    sim_time: Res<SimulationTime>,
) {
    for (mut fleet, mut location) in &mut fleets {
        let FleetLocation::InTransit {
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
        fleet.delta_v_remaining -= arrival_dv;

        let target_name = bodies.get(*target).map(|b| b.name.as_str()).unwrap_or("?");
        info!(
            "Fleet '{}' arrived at {}! dv spent: {:.0} m/s, remaining: {:.0} m/s",
            fleet.name, target_name, arrival_dv, fleet.delta_v_remaining
        );

        // Transition to AtBody
        *location = FleetLocation::AtBody(*target);
    }
}

/// Detects when player fleets arrive at bodies with enemy fleets and triggers combat.
pub fn detect_combat(
    fleets: Query<(Entity, &Fleet, &FleetLocation, &Faction)>,
    bodies: Query<&Body>,
    mut combat: ResMut<CombatState>,
) {
    // Skip if combat is already active
    if combat.active {
        return;
    }

    // Group fleets by body
    use bevy::platform::collections::HashMap;
    let mut player_at_body: HashMap<Entity, Vec<Entity>> = HashMap::new();
    let mut enemy_at_body: HashMap<Entity, Vec<Entity>> = HashMap::new();

    for (fleet_entity, _, location, faction) in &fleets {
        if let FleetLocation::AtBody(body) = location {
            match faction {
                Faction::Player => player_at_body.entry(*body).or_default().push(fleet_entity),
                Faction::Enemy => enemy_at_body.entry(*body).or_default().push(fleet_entity),
            }
        }
    }

    // Find first body with both player and enemy fleets
    for (body, player_fleets) in &player_at_body {
        if let Some(enemy_fleets) = enemy_at_body.get(body) {
            let body_name = bodies.get(*body).map(|b| b.name.as_str()).unwrap_or("?");
            info!(
                "COMBAT TRIGGERED at {}! {} player fleet(s) vs {} enemy fleet(s)",
                body_name,
                player_fleets.len(),
                enemy_fleets.len()
            );

            // Populate combat state
            combat.active = true;
            combat.body = Some(*body);
            combat.player_fleets = player_fleets.clone();
            combat.enemy_fleets = enemy_fleets.clone();
            // arena will be set by tactical mode entry (Step 2.5)
            combat.arena = None;

            return; // Only one combat at a time
        }
    }
}

/// Commits all uncommitted legs when Enter is pressed.
/// Only operates on the selected fleet.
pub fn commit_plan(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut fleets: Query<(&Fleet, &FleetLocation, &mut FlightPlan), With<Selected>>,
    lut: Res<TransferLut>,
    bodies: Query<&Body>,
) {
    if !keyboard.just_pressed(KeyCode::Enter) {
        return;
    }

    for (fleet, location, mut plan) in &mut fleets {
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
                let (Ok(src_body), Ok(tgt_body)) = (bodies.get(source), bodies.get(leg.target)) else {
                    return None;
                };
                lut.get_transfer(
                    source,
                    leg.target,
                    &src_body.orbital_elements,
                    &tgt_body.orbital_elements,
                    leg.departure_day,
                    leg.tof_days,
                ).map(|s| s.total_dv)
            })
            .sum();

        if fleet.delta_v_remaining < uncommitted_dv {
            warn!(
                "Insufficient delta-v to commit plan: need {:.0} m/s, have {:.0} m/s",
                uncommitted_dv, fleet.delta_v_remaining
            );
            continue;
        }

        // Log what we're committing
        for i in plan.committed_count..plan.legs.len() {
            let leg = &plan.legs[i];
            let source = leg_source(location, &plan, i);
            let source_name = bodies.get(source).map(|b| b.name.as_str()).unwrap_or("?");
            let target_name = bodies.get(leg.target).map(|b| b.name.as_str()).unwrap_or("?");

            if let (Ok(src_body), Ok(tgt_body)) = (bodies.get(source), bodies.get(leg.target)) {
                if let Some(solution) = lut.get_transfer(
                    source,
                    leg.target,
                    &src_body.orbital_elements,
                    &tgt_body.orbital_elements,
                    leg.departure_day,
                    leg.tof_days,
                ) {
                    info!(
                        "Committing leg {} -> {} (day {}, {:.0} m/s)",
                        source_name, target_name, leg.departure_day, solution.total_dv
                    );
                }
            }
        }

        // Commit all
        plan.committed_count = plan.legs.len();
    }
}

/// Cancels the last leg when N is pressed.
/// Only operates on the selected fleet.
pub fn cancel_last_leg(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut fleets: Query<&mut FlightPlan, With<Selected>>,
    bodies: Query<&Body>,
) {
    if !keyboard.just_pressed(KeyCode::KeyN) {
        return;
    }

    for mut plan in &mut fleets {
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
                if was_committed {
                    "committed"
                } else {
                    "planned"
                },
                target_name,
                plan.legs.len()
            );
        } else {
            info!("No legs to cancel");
        }
    }
}

/// Counter for generating unique fleet names
static FLEET_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Generate a unique fleet name based on NATO phonetic alphabet
fn generate_fleet_name() -> String {
    const NAMES: &[&str] = &[
        "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliet", "Kilo",
        "Lima", "Mike", "November", "Oscar", "Papa", "Quebec", "Romeo", "Sierra",
        "Tango", "Uniform", "Victor", "Whiskey", "Xray", "Yankee", "Zulu",
    ];
    let idx = FLEET_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as usize;
    if idx < NAMES.len() {
        NAMES[idx].to_string()
    } else {
        format!("Fleet-{}", idx + 1)
    }
}

/// Splits the selected fleet in half when S is pressed.
/// Only works if fleet is at a body (not in transit) and has >1 ship.
pub fn split_fleet(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    selected: Query<(Entity, &Fleet, &FleetLocation), With<Selected>>,
    children_query: Query<&Children>,
    ships: Query<&LogicalShip>,
) {
    if !keyboard.just_pressed(KeyCode::KeyS) {
        return;
    }

    let Ok((fleet_entity, fleet, location)) = selected.single() else {
        return;
    };

    // Must be at a body
    let FleetLocation::AtBody(body) = location else {
        info!("Cannot split fleet while in transit");
        return;
    };

    // Count ships from children
    let total_ships = ship_count(fleet_entity, &children_query, &ships);

    // Must have more than 1 ship
    if total_ships <= 1 {
        info!("Cannot split fleet with only {} ship(s)", total_ships);
        return;
    }

    // Split in half (larger half stays with original)
    let split_count = total_ships / 2;

    // Collect LogicalShip children to move to new fleet
    let mut ships_to_move = Vec::new();
    if let Ok(children) = children_query.get(fleet_entity) {
        for child in children.iter() {
            if ships.contains(child) {
                ships_to_move.push(child);
                if ships_to_move.len() >= split_count as usize {
                    break;
                }
            }
        }
    }

    // Spawn new fleet at same body
    let new_name = generate_fleet_name();
    info!(
        "Split {} ships from {} to new fleet {}",
        split_count, fleet.name, new_name
    );

    let new_fleet = commands.spawn((
        Fleet {
            delta_v_remaining: fleet.delta_v_remaining,
            name: new_name,
        },
        FleetLocation::AtBody(*body),
        Faction::Player,
        FlightPlan::default(),
    )).id();

    // Reparent ships to new fleet
    for ship in ships_to_move {
        commands.entity(new_fleet).add_child(ship);
    }
}

/// Merges all fleets at the same body into the selected fleet when M is pressed.
/// Only works if there are 2+ fleets at the same body.
pub fn merge_fleets(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    selected: Query<(Entity, &Fleet, &FleetLocation), With<Selected>>,
    other_fleets: Query<(Entity, &Fleet, &FleetLocation, &Faction), Without<Selected>>,
    children_query: Query<&Children>,
    ships: Query<&LogicalShip>,
) {
    if !keyboard.just_pressed(KeyCode::KeyM) {
        return;
    }

    let Ok((selected_entity, selected_fleet, selected_location)) = selected.single() else {
        return;
    };

    // Must be at a body
    let FleetLocation::AtBody(body) = selected_location else {
        info!("Cannot merge fleets while in transit");
        return;
    };

    // Find other player fleets at the same body
    let fleets_to_merge: Vec<_> = other_fleets
        .iter()
        .filter(|(_, _, loc, faction)| {
            **faction == Faction::Player && matches!(loc, FleetLocation::AtBody(b) if *b == *body)
        })
        .collect();

    if fleets_to_merge.is_empty() {
        info!("No other fleets at this body to merge");
        return;
    }

    let mut merged_names = Vec::new();

    for (entity, fleet, _, _) in &fleets_to_merge {
        merged_names.push(fleet.name.as_str());

        // Reparent all LogicalShip children to selected fleet before despawning
        if let Ok(children) = children_query.get(*entity) {
            for child in children.iter() {
                if ships.contains(child) {
                    commands.entity(selected_entity).add_child(child);
                }
            }
        }

        // Despawn the empty fleet shell (children have been reparented)
        commands.entity(*entity).despawn();
    }

    // Keep the higher delta-v (they should be the same, but just in case)
    let max_dv = fleets_to_merge
        .iter()
        .map(|(_, f, _, _)| f.delta_v_remaining)
        .fold(selected_fleet.delta_v_remaining, f64::max);

    if max_dv != selected_fleet.delta_v_remaining {
        commands.entity(selected_entity).insert(Fleet {
            delta_v_remaining: max_dv,
            name: selected_fleet.name.clone(),
        });
    }

    info!(
        "Merged {} into {}",
        merged_names.join(", "),
        selected_fleet.name,
    );
}

/// Checks if all enemy fleets are destroyed and updates victory state.
pub fn check_objectives(
    fleets: Query<(Entity, &Faction)>,
    children_query: Query<&Children>,
    ships: Query<&LogicalShip>,
    mut victory: ResMut<VictoryState>,
    sim_time: Res<crate::simulation::SimulationTime>,
) {
    // Don't check if already won
    if victory.victory_achieved {
        return;
    }

    // Count enemy fleets remaining (fleets with at least one ship)
    let enemy_fleets_remaining: u32 = fleets
        .iter()
        .filter(|(_, faction)| **faction == Faction::Enemy)
        .filter(|(entity, _)| ship_count(*entity, &children_query, &ships) > 0)
        .count() as u32;

    if enemy_fleets_remaining == 0 {
        victory.victory_achieved = true;
        victory.victory_time = Some(sim_time.sim_time);
        info!("VICTORY! All enemies destroyed!");
    }
}

/// Syncs Transfer visualization entities to match FleetLocation + committed legs.
/// - InTransit -> one Transfer for active flight
/// - Committed legs -> one Transfer each for future arcs
pub fn sync_transfer_entities(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    fleets: Query<(Entity, &FleetLocation, &FlightPlan)>,
    lut: Res<TransferLut>,
    transfers: Query<(Entity, &transfer_vis::Transfer)>,
    bodies: Query<&Body>,
) {
    for (fleet_entity, location, plan) in &fleets {
        // Build list of (source, target, solution, departure_time) for active visualizations
        let mut active: Vec<(Entity, Entity, TransferSolution, f64)> = Vec::new();

        // Add active transfer if InTransit
        if let FleetLocation::InTransit {
            target,
            solution,
            departure_time,
        } = location
        {
            // Source for active transfer is where we departed from
            // We don't store it, but we can get the body the ship was at
            // For now, use PLACEHOLDER - the visualization doesn't need source entity
            active.push((Entity::PLACEHOLDER, *target, solution.clone(), *departure_time));
        }

        // Add committed future legs
        for i in 0..plan.committed_count.min(plan.legs.len()) {
            let leg = &plan.legs[i];
            let source = leg_source(location, plan, i);
            if let (Ok(src_body), Ok(tgt_body)) = (bodies.get(source), bodies.get(leg.target)) {
                if let Some(solution) = lut.get_transfer(
                    source,
                    leg.target,
                    &src_body.orbital_elements,
                    &tgt_body.orbital_elements,
                    leg.departure_day,
                    leg.tof_days,
                ) {
                    let departure_time = leg.departure_day as f64 * 86400.0;
                    active.push((source, leg.target, solution, departure_time));
                }
            }
        }

        // Find existing Transfer entities for this fleet
        let fleet_transfers: Vec<_> = transfers
            .iter()
            .filter(|(_, t)| t.fleet == fleet_entity)
            .collect();

        // Despawn entities that don't match any active transfer
        for (transfer_entity, transfer) in &fleet_transfers {
            let has_match = active.iter().any(|(_, target, _, dep_time)| {
                *target == transfer.target && (*dep_time - transfer.departure_time).abs() < 1.0
            });
            if !has_match {
                commands.entity(*transfer_entity).despawn();
            }
        }

        // Spawn entities for active transfers that don't have one
        for (source, target, solution, departure_time) in &active {
            let has_entity = fleet_transfers.iter().any(|(_, t)| {
                t.target == *target && (t.departure_time - *departure_time).abs() < 1.0
            });
            if !has_entity {
                transfer_vis::spawn_transfer_visualization(
                    &mut commands,
                    &mut gizmo_assets,
                    fleet_entity,
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

/// Player fleet colors (green)
pub const FLEET_PLAYER_SELECTED: Color = Color::srgb(0.4, 1.0, 0.4);
pub const FLEET_PLAYER_UNSELECTED: Color = Color::srgba(0.3, 0.8, 0.3, 0.6);

/// Enemy fleet colors (imperial red)
pub const FLEET_ENEMY_SELECTED: Color = Color::srgb(1.0, 0.3, 0.3);
pub const FLEET_ENEMY_UNSELECTED: Color = Color::srgba(0.8, 0.2, 0.2, 0.6);

/// Fleet size in visual units
pub const FLEET_SIZE: f32 = 3.0;

/// Offset distance from body center for fleets
const FLEET_OFFSET_DISTANCE: f32 = 6.0;

/// Computes visual positions for all fleets, offsetting multiple fleets at the same body.
/// Returns a map from fleet entity to (world_position, velocity_direction).
pub fn compute_fleet_positions<F: bevy::ecs::query::QueryFilter>(
    ships: &Query<(Entity, &Fleet, &FleetLocation, Option<&Selected>, &Faction), F>,
    bodies: &Query<&ComputedBody>,
    sim_time: &SimulationTime,
) -> bevy::platform::collections::HashMap<Entity, (Vec3, Vec3)> {
    use bevy::platform::collections::HashMap;
    use std::f32::consts::PI;

    let mut positions = HashMap::new();

    // First pass: count fleets at each body
    let mut fleets_at_body: HashMap<Entity, Vec<Entity>> = HashMap::new();
    for (fleet_entity, _, location, _, _) in ships.iter() {
        if let FleetLocation::AtBody(body) = location {
            fleets_at_body.entry(*body).or_default().push(fleet_entity);
        }
    }

    // Second pass: compute positions with offsets
    for (fleet_entity, _, location, is_selected, _) in ships.iter() {
        let size_mult = if is_selected.is_some() { 1.3 } else { 1.0 };

        let (position, velocity_dir) = match location {
            FleetLocation::AtBody(body) => {
                let body_pos = bodies.get(*body).map(|c| c.position).unwrap_or(Vec3::ZERO);

                // Get index of this fleet among all fleets at this body
                let fleets_here = fleets_at_body.get(body).map(|v| v.as_slice()).unwrap_or(&[]);
                let fleet_index = fleets_here.iter().position(|e| *e == fleet_entity).unwrap_or(0);
                let fleet_count = fleets_here.len();

                // Compute offset angle for this fleet
                let offset = if fleet_count == 1 {
                    // Single fleet: offset to the right
                    Vec3::new(FLEET_OFFSET_DISTANCE * size_mult, 0.0, 0.0)
                } else {
                    // Multiple fleets: fan out in a semicircle (top half)
                    let angle = PI * 0.25 + (fleet_index as f32 / (fleet_count - 1).max(1) as f32) * PI * 0.5;
                    let x = FLEET_OFFSET_DISTANCE * size_mult * angle.cos();
                    let y = FLEET_OFFSET_DISTANCE * size_mult * angle.sin();
                    Vec3::new(x, y, 0.0)
                };

                (body_pos + offset, Vec3::new(0.0, 1.0, 0.0))
            }
            FleetLocation::InTransit {
                solution,
                departure_time,
                ..
            } => {
                let elapsed = sim_time.sim_time - departure_time;
                if elapsed < 0.0 {
                    continue;
                }

                if let Some((r_vec, v_vec)) = propagate_kepler_full(
                    solution.departure_pos,
                    solution.departure_vel,
                    MU_SUN,
                    elapsed,
                ) {
                    let pos = phys_to_visual(r_vec);
                    let vel_dir = Vec3::new(v_vec.x as f32, v_vec.y as f32, 0.0).normalize_or_zero();
                    (pos, vel_dir)
                } else {
                    continue;
                }
            }
        };

        positions.insert(fleet_entity, (position, velocity_dir));
    }

    positions
}

/// Updates ComputedFleetPosition components for all fleets.
/// Run this before rendering to have positions available.
pub fn update_fleet_positions(
    mut commands: Commands,
    fleets: Query<(Entity, &Fleet, &FleetLocation, Option<&Selected>, &Faction)>,
    bodies: Query<&ComputedBody>,
    sim_time: Res<SimulationTime>,
) {
    let positions = compute_fleet_positions(&fleets, &bodies, &sim_time);

    for (fleet_entity, _, _, _, _) in &fleets {
        if let Some((position, velocity_dir)) = positions.get(&fleet_entity) {
            commands.entity(fleet_entity).insert(ComputedFleetPosition {
                position: *position,
                velocity_dir: *velocity_dir,
            });
        }
    }
}

/// Departure marker color (distinct from departure burn arrow)
const DEPARTURE_MARKER_COLOR: Color = Color::srgb(0.9, 0.9, 0.3); // Yellow

/// Renders fleets as triangles at their current positions.
/// Reads from ComputedFleetPosition (updated by update_fleet_positions).
pub fn render_fleets(
    fleets: Query<(Entity, &ComputedFleetPosition, Option<&Selected>, &Faction)>,
    children_query: Query<&Children>,
    logical_ships: Query<&LogicalShip>,
    mut painter: ShapePainter,
) {
    for (fleet_entity, computed, is_selected, faction) in &fleets {
        let is_selected = is_selected.is_some();
        let size_mult = if is_selected { 1.3 } else { 1.0 };
        let color = match (faction, is_selected) {
            (Faction::Player, true) => FLEET_PLAYER_SELECTED,
            (Faction::Player, false) => FLEET_PLAYER_UNSELECTED,
            (Faction::Enemy, true) => FLEET_ENEMY_SELECTED,
            (Faction::Enemy, false) => FLEET_ENEMY_UNSELECTED,
        };

        // Draw triangle pointing in velocity direction
        painter.set_translation(computed.position);

        let rotation = if computed.velocity_dir.length_squared() > 0.001 {
            Quat::from_rotation_arc(Vec3::Y, computed.velocity_dir)
        } else {
            Quat::IDENTITY
        };
        painter.set_rotation(rotation);

        painter.set_color(color);

        // Draw an isoceles triangle
        let half_base = FLEET_SIZE * 0.5 * size_mult;
        let height = FLEET_SIZE * size_mult;
        painter.thickness = if is_selected { 0.8 } else { 0.5 };
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

        // Draw ship count below the triangle for selected fleet
        if is_selected {
            painter.set_rotation(Quat::IDENTITY);
            let count_pos = computed.position + Vec3::new(0.0, -height * 0.8, 0.0);
            let count = ship_count(fleet_entity, &children_query, &logical_ships);
            draw_number(&mut painter, count as usize, count_pos, 2.0);
        }
    }
}

/// Enemy marker color (matches fleet color)
const ENEMY_MARKER_COLOR: Color = Color::srgba(0.8, 0.2, 0.2, 0.6);

/// Renders enemy presence markers at bodies with enemy fleets.
/// Shows red rings around occupied bodies.
pub fn render_objectives(
    fleets: Query<(Entity, &FleetLocation, &Faction)>,
    children_query: Query<&Children>,
    ships: Query<&LogicalShip>,
    bodies: Query<&ComputedBody>,
    mut painter: ShapePainter,
) {
    // Collect bodies with enemy fleets
    let mut enemy_bodies: bevy::platform::collections::HashSet<Entity> = bevy::platform::collections::HashSet::new();
    let mut total_enemy_fleets = 0u32;

    for (fleet_entity, location, faction) in &fleets {
        if *faction != Faction::Enemy {
            continue;
        }
        // Only count fleets with ships
        if ship_count(fleet_entity, &children_query, &ships) == 0 {
            continue;
        }
        total_enemy_fleets += 1;
        if let FleetLocation::AtBody(body) = location {
            enemy_bodies.insert(*body);
        }
    }

    // Draw red rings around enemy-occupied bodies
    for body_entity in &enemy_bodies {
        let Ok(computed) = bodies.get(*body_entity) else {
            continue;
        };

        let pos = computed.position;
        let ring_radius = computed.display_size + 5.0;

        painter.set_translation(pos);
        painter.set_rotation(Quat::IDENTITY);
        painter.set_color(ENEMY_MARKER_COLOR);
        painter.thickness = 1.5;
        painter.hollow = true;
        painter.circle(ring_radius);
    }

    // Draw enemy fleet counter in top-left corner (screen space handled by UI)
    // For now, draw at a fixed world position - we'll need UI text for proper screen-space
    // This is a placeholder - proper implementation would use egui or bevy_ui
    if total_enemy_fleets > 0 {
        // Draw a small indicator showing fleet count at origin offset
        // This will be replaced by proper UI text later
        painter.set_translation(Vec3::new(-45.0, 45.0, 0.0));
        painter.set_color(ENEMY_MARKER_COLOR);
        draw_number(&mut painter, total_enemy_fleets as usize, Vec3::ZERO, 4.0);
    }
}

/// Renders X markers at departure points for pending transfers.
pub fn render_departure_markers(
    transfers: Query<&transfer_vis::Transfer>,
    fleets: Query<&FleetLocation>,
    sim_time: Res<SimulationTime>,
    mut painter: ShapePainter,
) {
    for transfer in &transfers {
        // Only show marker if transfer hasn't started yet
        if transfer.departure_time <= sim_time.sim_time {
            continue;
        }

        // Only show if fleet is still at body (not already transferring)
        if let Ok(location) = fleets.get(transfer.fleet) {
            if !matches!(location, FleetLocation::AtBody(_)) {
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
    fleets: Query<&FlightPlan>,
    bodies: Query<&ComputedBody>,
    mut painter: ShapePainter,
) {
    for plan in &fleets {
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
    fleets: Query<(&FleetLocation, &FlightPlan)>,
    lut: Res<TransferLut>,
    bodies: Query<&Body>,
    mut painter: ShapePainter,
) {
    for (location, plan) in &fleets {
        // Only render uncommitted legs (index >= committed_count)
        for i in plan.committed_count..plan.legs.len() {
            let leg = &plan.legs[i];
            let source = leg_source(location, plan, i);

            // Look up solution from LUT
            let (Ok(src_body), Ok(tgt_body)) = (bodies.get(source), bodies.get(leg.target)) else {
                continue;
            };
            let Some(solution) = lut.get_transfer(
                source,
                leg.target,
                &src_body.orbital_elements,
                &tgt_body.orbital_elements,
                leg.departure_day,
                leg.tof_days,
            ) else {
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
