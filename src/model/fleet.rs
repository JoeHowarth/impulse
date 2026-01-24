//! Fleet data types and pure computations.
//!
//! This module contains fleet-related components and resources.
//! No Bevy systems - just types and pure helper functions.

use std::collections::VecDeque;

use bevy::prelude::*;

use crate::model::TransferSolution;

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
