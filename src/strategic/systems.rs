//! Strategic mode systems.
//!
//! Command processors and game logic for strategic mode.

use bevy::ecs::message::MessageReader;
use bevy::prelude::*;

use crate::app_state::AppState;
use crate::common::SimulationTime;
use crate::model::{
    Body, CombatState, Faction, Fleet, FleetLocation, FlightPlan, LogicalShip, PlannedLeg,
    Selected, VictoryState, leg_source, ship_count,
};

use super::commands::StrategicCommand;
use super::rendering::generate_fleet_name;
use super::transfer_lut::TransferLut;

// ============================================================================
// State Transitions
// ============================================================================

/// Clears combat state when returning to strategic mode.
pub fn reset_combat_state(mut combat: ResMut<CombatState>) {
    combat.active = false;
    combat.arena = None;
    combat.body = None;
    combat.player_fleets.clear();
    combat.enemy_fleets.clear();
}

// ============================================================================
// Command Processors
// ============================================================================

/// Process SelectFleet and DeselectFleet commands.
pub fn process_select_fleet(
    mut commands: Commands,
    mut reader: MessageReader<StrategicCommand>,
    selected: Query<Entity, With<Selected>>,
    fleets: Query<&Fleet>,
) {
    for cmd in reader.read() {
        match cmd {
            StrategicCommand::SelectFleet(fleet_entity) => {
                // Verify the entity is a fleet
                if fleets.get(*fleet_entity).is_err() {
                    warn!("SelectFleet: entity {:?} is not a fleet", fleet_entity);
                    continue;
                }
                // Remove Selected from all
                for old in &selected {
                    commands.entity(old).remove::<Selected>();
                }
                // Add Selected to target
                commands.entity(*fleet_entity).insert(Selected);
            }
            StrategicCommand::DeselectFleet => {
                for old in &selected {
                    commands.entity(old).remove::<Selected>();
                }
            }
            _ => {}
        }
    }
}

/// Process PlanTransfer commands - add a transfer leg to a fleet's flight plan.
pub fn process_plan_transfer(
    mut reader: MessageReader<StrategicCommand>,
    mut fleets: Query<(&Fleet, &FleetLocation, &mut FlightPlan)>,
    bodies: Query<&Body>,
    lut: Res<TransferLut>,
    sim_time: Res<SimulationTime>,
) {
    for cmd in reader.read() {
        let StrategicCommand::PlanTransfer {
            fleet,
            target,
            departure_day,
            tof_days,
        } = cmd
        else {
            continue;
        };

        let Ok((ship, location, mut plan)) = fleets.get_mut(*fleet) else {
            warn!("PlanTransfer: fleet {:?} not found", fleet);
            continue;
        };

        let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

        // Source is where we'd depart from after all current legs
        let next_leg_index = plan.legs.len();
        let source_entity = leg_source(location, &plan, next_leg_index);

        // Calculate total delta-v needed (existing legs + new leg)
        let existing_dv: f64 = plan
            .legs
            .iter()
            .enumerate()
            .filter_map(|(i, leg)| {
                let src = leg_source(location, &plan, i);
                let (Ok(src_body), Ok(tgt_body)) = (bodies.get(src), bodies.get(leg.target)) else {
                    return None;
                };
                lut.get_transfer(
                    src,
                    leg.target,
                    &src_body.orbital_elements,
                    &tgt_body.orbital_elements,
                    leg.departure_day,
                    leg.tof_days,
                )
                .map(|s| s.total_dv)
            })
            .sum();

        // Look up the new transfer
        let (Ok(src_body), Ok(tgt_body)) = (bodies.get(source_entity), bodies.get(*target)) else {
            warn!("PlanTransfer: could not get body orbital elements");
            continue;
        };
        let Some(solution) = lut.get_transfer(
            source_entity,
            *target,
            &src_body.orbital_elements,
            &tgt_body.orbital_elements,
            *departure_day,
            *tof_days,
        ) else {
            warn!(
                "PlanTransfer: no transfer found for dep_day={}, tof={}",
                departure_day, tof_days
            );
            continue;
        };

        let total_required = existing_dv + solution.total_dv;
        if ship.delta_v_remaining < total_required {
            warn!(
                "PlanTransfer: insufficient delta-v! Need {:.0} m/s, have {:.0} m/s",
                total_required, ship.delta_v_remaining
            );
            continue;
        }

        info!(
            "Queueing leg {} -> {} (dep day {}, {} m/s)",
            src_body.name, tgt_body.name, departure_day, solution.total_dv as i32
        );

        plan.legs.push_back(PlannedLeg {
            target: *target,
            departure_day: *departure_day,
            tof_days: *tof_days,
        });
    }
}

/// Process CommitPlan commands - commit all uncommitted legs.
pub fn process_commit_plan(
    mut reader: MessageReader<StrategicCommand>,
    mut fleets: Query<(&Fleet, &FleetLocation, &mut FlightPlan)>,
    lut: Res<TransferLut>,
    bodies: Query<&Body>,
) {
    for cmd in reader.read() {
        let StrategicCommand::CommitPlan(fleet_entity) = cmd else {
            continue;
        };

        let Ok((fleet, location, mut plan)) = fleets.get_mut(*fleet_entity) else {
            warn!("CommitPlan: fleet {:?} not found", fleet_entity);
            continue;
        };

        // Check if there are uncommitted legs
        if plan.committed_count >= plan.legs.len() {
            info!("CommitPlan: no uncommitted legs");
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
                "CommitPlan: insufficient delta-v! Need {:.0} m/s, have {:.0} m/s",
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

/// Process CancelLeg commands - remove the last leg from flight plan.
pub fn process_cancel_leg(
    mut reader: MessageReader<StrategicCommand>,
    mut fleets: Query<&mut FlightPlan>,
    bodies: Query<&Body>,
) {
    for cmd in reader.read() {
        let StrategicCommand::CancelLeg(fleet_entity) = cmd else {
            continue;
        };

        let Ok(mut plan) = fleets.get_mut(*fleet_entity) else {
            warn!("CancelLeg: fleet {:?} not found", fleet_entity);
            continue;
        };

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
            info!("CancelLeg: no legs to cancel");
        }
    }
}

/// Process SplitFleet commands - split a fleet in half.
pub fn process_split_fleet(
    mut commands: Commands,
    mut reader: MessageReader<StrategicCommand>,
    fleets: Query<(&Fleet, &FleetLocation)>,
    children_query: Query<&Children>,
    ships: Query<&LogicalShip>,
) {
    for cmd in reader.read() {
        let StrategicCommand::SplitFleet(fleet_entity) = cmd else {
            continue;
        };

        let Ok((fleet, location)) = fleets.get(*fleet_entity) else {
            warn!("SplitFleet: fleet {:?} not found", fleet_entity);
            continue;
        };

        // Must be at a body
        let FleetLocation::AtBody(body) = location else {
            info!("SplitFleet: cannot split fleet while in transit");
            continue;
        };

        // Count ships from children
        let total_ships = ship_count(*fleet_entity, &children_query, &ships);

        // Must have more than 1 ship
        if total_ships <= 1 {
            info!(
                "SplitFleet: cannot split fleet with only {} ship(s)",
                total_ships
            );
            continue;
        }

        // Split in half (larger half stays with original)
        let split_count = total_ships / 2;

        // Collect LogicalShip children to move to new fleet
        let mut ships_to_move = Vec::new();
        if let Ok(children) = children_query.get(*fleet_entity) {
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
}

/// Process time control commands - set paused state and time scale.
pub fn process_time_control(
    mut reader: MessageReader<StrategicCommand>,
    mut sim_time: ResMut<SimulationTime>,
) {
    for cmd in reader.read() {
        match cmd {
            StrategicCommand::SetPaused(paused) => {
                sim_time.paused = *paused;
                info!("Simulation {}", if *paused { "paused" } else { "resumed" });
            }
            StrategicCommand::SetTimeScale(scale) => {
                sim_time.time_scale = *scale;
                info!("Time scale set to {:.1}x", scale / 86400.0);
            }
            _ => {}
        }
    }
}

/// Process MergeFleets commands - merge all player fleets at same body into target fleet.
pub fn process_merge_fleets(
    mut commands: Commands,
    mut reader: MessageReader<StrategicCommand>,
    fleets: Query<(Entity, &Fleet, &FleetLocation, &Faction)>,
    children_query: Query<&Children>,
    ships: Query<&LogicalShip>,
) {
    for cmd in reader.read() {
        let StrategicCommand::MergeFleets(fleet_entity) = cmd else {
            continue;
        };

        let Ok((_, selected_fleet, selected_location, _)) = fleets.get(*fleet_entity) else {
            warn!("MergeFleets: fleet {:?} not found", fleet_entity);
            continue;
        };

        // Must be at a body
        let FleetLocation::AtBody(body) = selected_location else {
            info!("MergeFleets: cannot merge fleets while in transit");
            continue;
        };

        // Find other player fleets at the same body
        let fleets_to_merge: Vec<_> = fleets
            .iter()
            .filter(|(e, _, loc, faction)| {
                *e != *fleet_entity
                    && **faction == Faction::Player
                    && matches!(loc, FleetLocation::AtBody(b) if *b == *body)
            })
            .collect();

        if fleets_to_merge.is_empty() {
            info!("MergeFleets: no other fleets at this body to merge");
            continue;
        }

        let mut merged_names = Vec::new();

        for (entity, fleet, _, _) in &fleets_to_merge {
            merged_names.push(fleet.name.as_str());

            // Reparent all LogicalShip children to selected fleet before despawning
            if let Ok(children) = children_query.get(*entity) {
                for child in children.iter() {
                    if ships.contains(child) {
                        commands.entity(*fleet_entity).add_child(child);
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
            commands.entity(*fleet_entity).insert(Fleet {
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
}

// ============================================================================
// Simulation Systems
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

/// Checks if all enemy fleets are destroyed and updates victory state.
pub fn check_objectives(
    fleets: Query<(Entity, &Faction)>,
    children_query: Query<&Children>,
    ships: Query<&LogicalShip>,
    mut victory: ResMut<VictoryState>,
    sim_time: Res<SimulationTime>,
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
