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
use std::sync::atomic::{AtomicUsize, Ordering};

use bevy::gizmos::GizmoAsset;
use bevy::math::{DVec3, Isometry3d};
use bevy::prelude::*;
use bevy_vector_shapes::prelude::*; // TODO: Remove once retained shapes migrated
use big_space::prelude::*;

use crate::ComputedBody;
use crate::app_state::AppState;
use crate::camera::{BigSpaceRoot, CameraScale};
use crate::orbital_data::{Body, MU_SUN};
use crate::simulation::SimulationTime;
use crate::transfer::{TransferSolution, propagate_kepler_full};
use crate::transfer_lut::TransferLut;
use crate::transfer_vis::{HoveredTransferArc, TransferArcType};
use crate::{phys_vec_to_vec3, transfer_vis};

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

/// Marker for objective ring entities (retained shape showing enemy presence at a body).
/// These are spawned as children of Body entities.
#[derive(Component)]
pub struct ObjectiveRing;

/// Clears combat state when returning to strategic mode.
pub fn reset_combat_state(mut combat: ResMut<CombatState>) {
    combat.active = false;
    combat.arena = None;
    combat.body = None;
    combat.player_fleets.clear();
    combat.enemy_fleets.clear();
}

/// Despawn strategic-only marker entities when entering tactical mode.
pub fn despawn_strategic_markers(
    mut commands: Commands,
    fleet_shapes: Query<Entity, With<FleetShape>>,
    rings: Query<Entity, With<ObjectiveRing>>,
) {
    for entity in &fleet_shapes {
        commands.entity(entity).despawn();
    }
    for entity in &rings {
        commands.entity(entity).despawn();
    }
}

/// Marker for fleet visual entities (retained shape for strategic map).
/// Links the shape to its logical fleet entity.
#[derive(Component)]
pub struct FleetShape {
    pub fleet_entity: Entity,
    /// True if shape was spawned for InTransit (has CellCoord, parented to BigSpace).
    /// False if spawned for AtBody (parented to body entity).
    pub is_transit_shape: bool,
}

/// Marker for flight plan waypoint gizmos.
/// Spawned as children of target body entities.
#[derive(Component)]
pub struct PlanMarker {
    /// Which fleet's plan this marker belongs to
    pub fleet: Entity,
    /// Index in the flight plan
    pub leg_index: usize,
}

/// Fleet's current location - either at a body or in transit.
#[derive(Component)]
pub enum FleetLocation {
    /// Fleet is at a celestial body
    AtBody(Entity),
    /// Fleet is in transit between bodies
    InTransit {
        source: Entity,
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
pub fn expire_stale_uncommitted_legs(
    mut ships: Query<&mut FlightPlan>,
    sim_time: Res<SimulationTime>,
) {
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

        let mut i = 0;
        plan.legs.retain(|leg| {
            i += 1;
            i - 1 < committed || leg.departure_day >= current_day
        });

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
        let (Ok(source_body), Ok(target_body)) = (bodies.get(current_body), bodies.get(leg.target))
        else {
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
            source: current_body,
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
            source: _,
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
    mut next_state: ResMut<NextState<AppState>>,
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
            next_state.set(AppState::Tactical);

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
                let (Ok(src_body), Ok(tgt_body)) = (bodies.get(source), bodies.get(leg.target))
                else {
                    return None;
                };
                lut.get_transfer(
                    source,
                    leg.target,
                    &src_body.orbital_elements,
                    &tgt_body.orbital_elements,
                    leg.departure_day,
                    leg.tof_days,
                )
                .map(|s| s.total_dv)
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
            let target_name = bodies
                .get(leg.target)
                .map(|b| b.name.as_str())
                .unwrap_or("?");

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
        "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliet", "Kilo", "Lima", "Mike",
        "November", "Oscar", "Papa", "Quebec", "Romeo", "Sierra", "Tango", "Uniform", "Victor",
        "Whiskey", "Xray", "Yankee", "Zulu",
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

    let new_fleet = commands
        .spawn((
            Fleet {
                delta_v_remaining: fleet.delta_v_remaining,
                name: new_name,
            },
            FleetLocation::AtBody(*body),
            Faction::Player,
            FlightPlan::default(),
        ))
        .id();

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
    transfers: Query<(Entity, &transfer_vis::Transfer), Without<HoveredTransferArc>>,
    bodies: Query<&Body>,
    cam_scale: Res<CameraScale>,
) {
    for (fleet_entity, location, plan) in &fleets {
        // Build list of (source, target, solution, departure_time) for active visualizations
        let mut active: Vec<(Entity, Entity, TransferSolution, f64, TransferArcType)> = Vec::new();

        // Add active transfer if InTransit
        if let FleetLocation::InTransit {
            source,
            target,
            solution,
            departure_time,
        } = location
        {
            active.push((
                *source,
                *target,
                solution.clone(),
                *departure_time,
                TransferArcType::Committed,
            ));
        }

        for (i, leg) in plan.legs.iter().enumerate() {
            let source = leg_source(location, plan, i);

            let [src_body, tgt_body] = bodies
                .get_many([source, leg.target])
                .expect("Source and target bodies not found");

            let solution = lut
                .get_transfer(
                    source,
                    leg.target,
                    &src_body.orbital_elements,
                    &tgt_body.orbital_elements,
                    leg.departure_day,
                    leg.tof_days,
                )
                .expect("No transfer solution found for leg");

            let departure_time = leg.departure_day as f64 * 86400.0;
            let ty = if i < plan.committed_count {
                TransferArcType::Committed
            } else {
                TransferArcType::Preview
            };
            active.push((source, leg.target, solution, departure_time, ty));
        }

        // Find existing Transfer entities for this fleet
        let fleet_transfers: Vec<_> = transfers
            .iter()
            .filter(|(_, t)| t.fleet == fleet_entity)
            .collect();

        // Despawn entities that don't match any active transfer
        for (transfer_entity, transfer) in &fleet_transfers {
            let has_match = active.iter().any(|(_, target, _, dep_time, _)| {
                *target == transfer.target && (*dep_time - transfer.departure_time).abs() < 1.0
            });

            if !has_match {
                commands.entity(*transfer_entity).despawn();
            }
        }

        // Spawn entities for active transfers that don't have one
        for (source, target, solution, departure_time, arc_type) in &active {
            let transfer_ent = fleet_transfers.iter().find(|(_, t)| {
                t.target == *target && (t.departure_time - *departure_time).abs() < 1.0
            });

            match transfer_ent {
                Some((transfer_entity, t)) => {
                    // Wrong type of transfer arc, despawn and recreate as an active transfer arc
                    if t.arc_type == *arc_type {
                        continue;
                    }
                    commands.entity(*transfer_entity).despawn();
                }
                None => {}
            }
            let parent_entity = bodies
                .get(*source)
                .map(|b| b.parent_entity)
                .expect("Body has no parent")
                .expect("Body has no parent");
            transfer_vis::spawn_transfer_visualization(
                &mut commands,
                &mut gizmo_assets,
                parent_entity,
                fleet_entity,
                *source,
                *target,
                solution,
                *departure_time,
                cam_scale.0,
                *arc_type,
            );
        }

        // Debug logging
        static DEDUP_LOG_THRESHOLD: AtomicUsize = AtomicUsize::new(0);
        let dedup_log_threshold = DEDUP_LOG_THRESHOLD.load(Ordering::Relaxed);
        if dedup_log_threshold < 10 {
            DEDUP_LOG_THRESHOLD.fetch_add(1, Ordering::Relaxed);
        } else {
            let names: Vec<String> = active
                .iter()
                .map(|(src, tgt, _, _, _)| {
                    let src_name = bodies.get(*src).map(|b| b.name.as_str()).unwrap_or("?");
                    let tgt_name = bodies.get(*tgt).map(|b| b.name.as_str()).unwrap_or("?");
                    format!("{}->{}", src_name, tgt_name)
                })
                .collect();
            debug!("sync_transfer_entities: [{}]", names.join(", "));
            DEDUP_LOG_THRESHOLD.store(0, Ordering::Relaxed);
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

/// Fleet size in pixels (scale = world units per pixel)
const FLEET_SIZE_PIXELS: f32 = 10.0;

/// Offset distance in pixels
const FLEET_OFFSET_PIXELS: f32 = 10.0;

/// Computes visual positions for all fleets, offsetting multiple fleets at the same body.
/// Returns a map from fleet entity to (world_position, velocity_direction).
/// Note: Uses GlobalTransform for body positions (camera-relative via big_space).
pub fn compute_fleet_positions<F: bevy::ecs::query::QueryFilter>(
    ships: &Query<(Entity, &Fleet, &FleetLocation, Option<&Selected>, &Faction), F>,
    bodies: &Query<&GlobalTransform, With<Body>>,
    sim_time: &SimulationTime,
    cam_scale: f32,
) -> bevy::platform::collections::HashMap<Entity, (Vec3, Vec3)> {
    use bevy::platform::collections::HashMap;
    use std::f32::consts::PI;

    let mut positions = HashMap::new();
    let offset_distance = cam_scale * FLEET_OFFSET_PIXELS;

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
                let body_pos = bodies
                    .get(*body)
                    .map(|gt| gt.translation())
                    .unwrap_or(Vec3::ZERO);

                // Get index of this fleet among all fleets at this body
                let fleets_here = fleets_at_body
                    .get(body)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let fleet_index = fleets_here
                    .iter()
                    .position(|e| *e == fleet_entity)
                    .unwrap_or(0);
                let fleet_count = fleets_here.len();

                // Compute offset angle for this fleet
                let offset = if fleet_count == 1 {
                    // Single fleet: offset to the right
                    Vec3::new(offset_distance * size_mult, 0.0, 0.0)
                } else {
                    // Multiple fleets: fan out in a semicircle (top half)
                    let angle = PI * 0.25
                        + (fleet_index as f32 / (fleet_count - 1).max(1) as f32) * PI * 0.5;
                    let x = offset_distance * size_mult * angle.cos();
                    let y = offset_distance * size_mult * angle.sin();
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
                    let pos = phys_vec_to_vec3(r_vec);
                    let vel_dir =
                        Vec3::new(v_vec.x as f32, v_vec.y as f32, 0.0).normalize_or_zero();
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
    bodies: Query<&GlobalTransform, With<Body>>,
    sim_time: Res<SimulationTime>,
    cam_scale: Res<CameraScale>,
) {
    let positions = compute_fleet_positions(&fleets, &bodies, &sim_time, cam_scale.0);

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

///// Renders fleets as triangles at their current positions.
///// Reads from ComputedFleetPosition (updated by update_fleet_positions).
///// Skipped during tactical combat (tactical.rs handles ship rendering).
// pub fn render_fleets(
//     combat: Res<CombatState>,
//     fleets: Query<(Entity, &ComputedFleetPosition, Option<&Selected>, &Faction)>,
//     children_query: Query<&Children>,
//     logical_ships: Query<&LogicalShip>,
//     cam_scale: Res<CameraScale>,
//     mut painter: ShapePainter,
// ) {
//     // Skip strategic fleet rendering during tactical mode
//     if combat.active {
//         return;
//     }

//     let fleet_size = cam_scale.0 * FLEET_SIZE_PIXELS;

//     for (fleet_entity, computed, is_selected, faction) in &fleets {
//         let is_selected = is_selected.is_some();
//         let size_mult = if is_selected { 1.3 } else { 1.0 };
//         let color = match (faction, is_selected) {
//             (Faction::Player, true) => FLEET_PLAYER_SELECTED,
//             (Faction::Player, false) => FLEET_PLAYER_UNSELECTED,
//             (Faction::Enemy, true) => FLEET_ENEMY_SELECTED,
//             (Faction::Enemy, false) => FLEET_ENEMY_UNSELECTED,
//         };

//         // Draw triangle pointing in velocity direction
//         painter.set_translation(computed.position);

//         let rotation = if computed.velocity_dir.length_squared() > 0.001 {
//             Quat::from_rotation_arc(Vec3::Y, computed.velocity_dir)
//         } else {
//             Quat::IDENTITY
//         };
//         painter.set_rotation(rotation);

//         painter.set_color(color);

//         // Draw an isoceles triangle (size scales with camera)
//         let half_base = fleet_size * 0.5 * size_mult;
//         let height = fleet_size * size_mult;
//         painter.thickness = fleet_size * 0.1; // Line thickness scales too
//         painter.line(
//             Vec3::new(0.0, height * 0.5, 0.0),
//             Vec3::new(-half_base, -height * 0.5, 0.0),
//         );
//         painter.line(
//             Vec3::new(-half_base, -height * 0.5, 0.0),
//             Vec3::new(half_base, -height * 0.5, 0.0),
//         );
//         painter.line(
//             Vec3::new(half_base, -height * 0.5, 0.0),
//             Vec3::new(0.0, height * 0.5, 0.0),
//         );

//         // Draw ship count below the triangle for selected fleet
//         if is_selected {
//             painter.set_rotation(Quat::IDENTITY);
//             let count_pos = computed.position + Vec3::new(0.0, -height * 0.8, 0.0);
//             let count = ship_count(fleet_entity, &children_query, &logical_ships);
//             draw_number(&mut painter, count as usize, count_pos, fleet_size * 0.5);
//         }
//     }
// }

/// Syncs fleet shape entities with fleet positions.
/// Spawns shapes for new fleets, updates existing shapes, despawns orphaned shapes.
/// - AtBody fleets: shape is child of body entity (inherits body's Transform)
/// - InTransit fleets: shape has CellCoord + Transform from orbital position
pub fn sync_fleet_shapes(
    mut commands: Commands,
    combat: Res<CombatState>,
    big_space_root: Res<BigSpaceRoot>,
    grid_query: Query<&Grid, With<BigSpace>>,
    fleets: Query<(Entity, &Fleet, &FleetLocation, Option<&Selected>, &Faction)>,
    existing_shapes: Query<(Entity, &FleetShape)>,
    mut shape_transforms: Query<&mut Transform, With<FleetShape>>,
    sim_time: Res<SimulationTime>,
    cam_scale: Res<CameraScale>,
) {
    use bevy::platform::collections::{HashMap, HashSet};

    // Skip during tactical mode
    if combat.active {
        return;
    }

    let Ok(grid) = grid_query.single() else {
        return;
    };

    let cam_scale = cam_scale.0;
    let fleet_size = cam_scale * FLEET_SIZE_PIXELS;

    // Track which fleets currently exist and have shapes
    let mut fleets_with_shapes: HashMap<Entity, Entity> = HashMap::new();
    for (shape_entity, fleet_shape) in &existing_shapes {
        fleets_with_shapes.insert(fleet_shape.fleet_entity, shape_entity);
    }

    // Track which fleets we've processed
    let mut processed_fleets: HashSet<Entity> = HashSet::new();

    // Process all fleets
    for (fleet_entity, _fleet, location, is_selected, faction) in &fleets {
        processed_fleets.insert(fleet_entity);

        let is_selected = is_selected.is_some();
        let size_mult = if is_selected { 1.3 } else { 1.0 };
        let color = match (faction, is_selected) {
            (Faction::Player, true) => FLEET_PLAYER_SELECTED,
            (Faction::Player, false) => FLEET_PLAYER_UNSELECTED,
            (Faction::Enemy, true) => FLEET_ENEMY_SELECTED,
            (Faction::Enemy, false) => FLEET_ENEMY_UNSELECTED,
        };

        // Compute velocity direction for triangle orientation
        let velocity_dir = match location {
            FleetLocation::AtBody(_) => Vec3::Y, // Default up when stationary
            FleetLocation::InTransit {
                solution,
                departure_time,
                ..
            } => {
                let elapsed = sim_time.sim_time - departure_time;
                if elapsed >= 0.0 {
                    if let Some((_, v_vec)) = propagate_kepler_full(
                        solution.departure_pos,
                        solution.departure_vel,
                        MU_SUN,
                        elapsed,
                    ) {
                        phys_vec_to_vec3(v_vec).normalize_or_zero()
                    } else {
                        Vec3::Y
                    }
                } else {
                    Vec3::Y
                }
            }
        };

        let rotation = if velocity_dir.length_squared() > 0.001 {
            Quat::from_rotation_arc(Vec3::Y, velocity_dir)
        } else {
            Quat::IDENTITY
        };

        // Build triangle vertices (scaled, Vec2 for bevy_vector_shapes)
        let half_base = fleet_size * 0.5 * size_mult;
        let height = fleet_size * size_mult;
        let v_top = Vec2::new(0.0, height * 0.5);
        let v_left = Vec2::new(-half_base, -height * 0.5);
        let v_right = Vec2::new(half_base, -height * 0.5);

        let is_in_transit = matches!(location, FleetLocation::InTransit { .. });

        if let Some(&shape_entity) = fleets_with_shapes.get(&fleet_entity) {
            // Check if we have a matching shape
            let shape_info = existing_shapes.get(shape_entity).ok();

            // If shape type doesn't match location type, despawn and let respawn happen
            if let Some((_, fleet_shape)) = shape_info {
                if fleet_shape.is_transit_shape != is_in_transit {
                    // Location type changed - despawn old shape, spawn new one below
                    commands.entity(shape_entity).despawn();
                    // Fall through to spawn new shape
                } else {
                    // Update existing shape
                    if let Ok(mut transform) = shape_transforms.get_mut(shape_entity) {
                        match location {
                            FleetLocation::AtBody(_body) => {
                                // Shape is child of body - just update local offset and rotation
                                transform.translation = Vec3::new(cam_scale * 15.0, 0.0, 0.2);
                                transform.rotation = rotation;
                            }
                            FleetLocation::InTransit {
                                solution,
                                departure_time,
                                ..
                            } => {
                                // Compute position from orbital mechanics
                                let elapsed = sim_time.sim_time - departure_time;
                                if elapsed >= 0.0 {
                                    if let Some((r_vec, _)) = propagate_kepler_full(
                                        solution.departure_pos,
                                        solution.departure_vel,
                                        MU_SUN,
                                        elapsed,
                                    ) {
                                        // Convert nalgebra Vector3 to DVec3, then to CellCoord + local
                                        let helio_pos = DVec3::new(r_vec.x, r_vec.y, r_vec.z);
                                        let (cell, local) = grid.translation_to_grid(helio_pos);
                                        // Update CellCoord component
                                        commands.entity(shape_entity).insert(cell);
                                        transform.translation = local;
                                        transform.translation.z = 0.2; // Slight Z offset for visibility
                                    }
                                }
                                transform.rotation = rotation;
                            }
                        }
                    }

                    // Update triangle component and color
                    commands.entity(shape_entity).insert((
                        TriangleComponent::new(
                            &ShapeConfig {
                                color,
                                hollow: false,
                                ..ShapeConfig::default_3d()
                            },
                            v_top,
                            v_left,
                            v_right,
                        ),
                        ShapeFill {
                            color,
                            ty: FillType::Fill,
                        },
                    ));
                    continue; // Shape updated, move to next fleet
                }
            }
        }

        // Spawn new shape (either no existing shape, or old one was despawned due to type change)
        {
            match location {
                FleetLocation::AtBody(body) => {
                    // Spawn as child of body entity
                    let local_transform =
                        Transform::from_xyz(cam_scale * 15.0, 0.0, 0.2).with_rotation(rotation);
                    let config = ShapeConfig {
                        color,
                        thickness: cam_scale * 1.0,
                        hollow: false,
                        transform: local_transform,
                        ..ShapeConfig::default_3d()
                    };
                    commands.spawn((
                        ShapeBundle::triangle(&config, v_top, v_left, v_right).insert_3d(),
                        FleetShape {
                            fleet_entity,
                            is_transit_shape: false,
                        },
                        ChildOf(*body),
                    ));
                }
                FleetLocation::InTransit {
                    solution,
                    departure_time,
                    ..
                } => {
                    // Compute position and spawn with CellCoord
                    let elapsed = sim_time.sim_time - departure_time;
                    let helio_pos = if elapsed >= 0.0 {
                        if let Some((r_vec, _)) = propagate_kepler_full(
                            solution.departure_pos,
                            solution.departure_vel,
                            MU_SUN,
                            elapsed,
                        ) {
                            // Convert nalgebra Vector3 to bevy DVec3
                            DVec3::new(r_vec.x, r_vec.y, r_vec.z)
                        } else {
                            DVec3::ZERO
                        }
                    } else {
                        DVec3::ZERO
                    };

                    let (cell, local) = grid.translation_to_grid(helio_pos);
                    let local_transform =
                        Transform::from_translation(local + Vec3::Z * 0.2).with_rotation(rotation);
                    let config = ShapeConfig {
                        color,
                        thickness: cam_scale * 1.0,
                        hollow: false,
                        transform: local_transform,
                        ..ShapeConfig::default_3d()
                    };

                    commands.spawn((
                        ShapeBundle::triangle(&config, v_top, v_left, v_right).insert_3d(),
                        FleetShape {
                            fleet_entity,
                            is_transit_shape: true,
                        },
                        cell,
                        ChildOf(big_space_root.0),
                    ));
                }
            }
        }
    }

    // Despawn shapes for fleets that no longer exist
    for (shape_entity, fleet_shape) in &existing_shapes {
        if !processed_fleets.contains(&fleet_shape.fleet_entity) {
            commands.entity(shape_entity).despawn();
        }
    }
}

/// Enemy marker color (matches fleet color)
const ENEMY_MARKER_COLOR: Color = Color::srgba(0.8, 0.2, 0.2, 0.6);

/// Syncs objective ring entities with enemy fleet presence.
/// Spawns rings as children of bodies with enemies, despawns when enemies leave.
/// Ring size updates each frame based on camera scale.
pub fn sync_objective_rings(
    mut commands: Commands,
    combat: Res<CombatState>,
    fleets: Query<(Entity, &FleetLocation, &Faction)>,
    fleet_children: Query<&Children>,
    ships: Query<&LogicalShip>,
    bodies: Query<(Entity, &ComputedBody), With<Body>>,
    existing_rings: Query<(Entity, &ChildOf), With<ObjectiveRing>>,
    mut ring_shapes: Query<(&mut DiscComponent, &mut ShapeFill), With<ObjectiveRing>>,
    cam_scale: Res<CameraScale>,
) {
    use bevy::platform::collections::HashSet;

    // Hide rings during tactical mode
    if combat.active {
        // Could set Visibility::Hidden instead of despawning, but for now just skip updates
        return;
    }

    let cam_scale = cam_scale.0;

    // Collect bodies with enemy fleets
    let mut enemy_bodies: HashSet<Entity> = HashSet::new();
    for (fleet_entity, location, faction) in &fleets {
        if *faction != Faction::Enemy {
            continue;
        }
        if ship_count(fleet_entity, &fleet_children, &ships) == 0 {
            continue;
        }
        if let FleetLocation::AtBody(body) = location {
            enemy_bodies.insert(*body);
        }
    }

    // Track which bodies already have rings
    let mut bodies_with_rings: HashSet<Entity> = HashSet::new();
    for (ring_entity, child_of) in &existing_rings {
        let parent = child_of.parent();
        if enemy_bodies.contains(&parent) {
            // Body still has enemies - keep ring, update size
            bodies_with_rings.insert(parent);
            if let Ok((mut disc, mut fill)) = ring_shapes.get_mut(ring_entity) {
                // Update radius based on body's display size + offset
                if let Ok((_, computed)) = bodies.get(parent) {
                    disc.radius = computed.display_size + cam_scale * 5.0;
                    fill.ty = FillType::Stroke(cam_scale * 1.5, ThicknessType::World);
                }
            }
        } else {
            // No enemies at this body - despawn ring
            commands.entity(ring_entity).despawn();
        }
    }

    // Spawn rings for bodies that need them but don't have one
    for body_entity in &enemy_bodies {
        if bodies_with_rings.contains(body_entity) {
            continue;
        }
        let Ok((_, computed)) = bodies.get(*body_entity) else {
            continue;
        };

        let ring_radius = computed.display_size + cam_scale * 5.0;
        let config = ShapeConfig {
            color: ENEMY_MARKER_COLOR,
            thickness: cam_scale * 1.5,
            hollow: true,
            transform: Transform::from_xyz(0.0, 0.0, 0.1), // Slight Z offset
            ..ShapeConfig::default_3d()
        };

        commands.spawn((
            ShapeBundle::circle(&config, ring_radius).insert_3d(),
            ObjectiveRing,
            ChildOf(*body_entity),
        ));
    }
}


/// Queue waypoint marker color (cyan, dimmed)
const QUEUE_MARKER_COLOR: Color = Color::srgba(0.3, 0.8, 0.8, 0.7);

/// Size of plan marker in pixels (scaled by cam_scale)
const PLAN_MARKER_SIZE_PIXELS: f32 = 8.0;

/// Syncs plan marker gizmo entities with flight plan state.
/// Spawns markers as children of target body entities, despawns when legs are removed.
/// Uses Transform.scale to adjust size based on camera scale.
pub fn sync_plan_markers(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    fleets: Query<(Entity, &FlightPlan)>,
    existing_markers: Query<(Entity, &PlanMarker, &ChildOf)>,
    mut marker_transforms: Query<&mut Transform, With<PlanMarker>>,
    cam_scale: Res<CameraScale>,
) {
    use bevy::platform::collections::HashSet;

    let cam_scale = cam_scale.0;
    let marker_scale = cam_scale * PLAN_MARKER_SIZE_PIXELS;

    // Build set of desired markers: (fleet, leg_index, target_body)
    let mut desired: HashSet<(Entity, usize, Entity)> = HashSet::new();
    for (fleet_entity, plan) in &fleets {
        for (leg_index, leg) in plan.legs.iter().enumerate() {
            desired.insert((fleet_entity, leg_index, leg.target));
        }
    }

    // Track which desired markers already exist
    let mut existing_set: HashSet<(Entity, usize)> = HashSet::new();

    // Update existing markers or despawn if no longer needed
    for (marker_entity, marker, child_of) in &existing_markers {
        let parent_body = child_of.parent();
        let key = (marker.fleet, marker.leg_index, parent_body);

        if desired.contains(&key) {
            // Marker still valid - update scale
            existing_set.insert((marker.fleet, marker.leg_index));
            if let Ok(mut transform) = marker_transforms.get_mut(marker_entity) {
                transform.scale = Vec3::splat(marker_scale);
            }
        } else {
            // Marker no longer needed - despawn
            commands.entity(marker_entity).despawn();
        }
    }

    // Spawn markers for legs that don't have one
    for (fleet_entity, plan) in &fleets {
        for (leg_index, leg) in plan.legs.iter().enumerate() {
            if existing_set.contains(&(fleet_entity, leg_index)) {
                continue;
            }

            // Create a unit circle gizmo asset (radius 1.0, scaled by Transform)
            let mut gizmo = GizmoAsset::new();
            gizmo.circle(Isometry3d::IDENTITY, 1.0, QUEUE_MARKER_COLOR);

            commands.spawn((
                Gizmo {
                    handle: gizmo_assets.add(gizmo),
                    depth_bias: 0.08,
                    ..default()
                },
                PlanMarker {
                    fleet: fleet_entity,
                    leg_index,
                },
                Transform::from_xyz(0.0, 0.0, 0.15).with_scale(Vec3::splat(marker_scale)),
                ChildOf(leg.target),
            ));
        }
    }
}
