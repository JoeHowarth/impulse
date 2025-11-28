//! Transfer solution caching system.
//!
//! Precomputes and caches Lambert transfer solutions for all celestial body pairs
//! within a rolling time window. This allows instant UI response when selecting
//! transfer options.

use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use astrora_core::core::elements::coe_to_rv;

use crate::transfer::{compute_transfer, TransferSolution};
use crate::orbital_data::{Body, propagate_elliptic, MU_SUN};
use crate::simulation::SimulationTime;

// ============================================================================
// Resources
// ============================================================================

/// Cache key for Lambert solutions: (target_entity, departure_day, tof_days)
/// departure_day is days since J2000 epoch (can be negative)
type CacheKey = (Entity, i32, i32);

/// Cached Lambert transfer solutions from current body to all siblings.
/// Stores computed solutions keyed by (target_entity, departure_day, tof_days).
#[derive(Resource, Default)]
pub struct TransferCache {
    /// Map from (target_entity, departure_day, tof_days) to computed solution
    pub solutions: HashMap<CacheKey, TransferSolution>,
    /// Last sim_time day we updated the cache (for incremental updates)
    pub last_update_day: i32,
    /// Search window: how many days ahead to search for departures
    pub window_days: i32,
    /// Source body entity (invalidate cache if this changes)
    pub source_body: Option<Entity>,
}

// ============================================================================
// Constants
// ============================================================================

/// TOF candidates to evaluate (in days)
/// Wide range to ensure at least one valid solution exists for any departure day
const TOF_CANDIDATES: [i32; 13] = [100, 120, 150, 180, 200, 220, 250, 280, 300, 350, 400, 450, 500];

/// How far ahead to search for departure windows (days)
const SEARCH_WINDOW_DAYS: i32 = 500;

// ============================================================================
// Helper Functions
// ============================================================================

/// Computes a single Lambert transfer and returns the solution if valid.
fn compute_cached_transfer(
    source_body: &Body,
    target_body: &Body,
    departure_day: i32,
    tof_days: i32,
) -> Option<TransferSolution> {
    let departure_time = departure_day as f64 * 86400.0;
    let tof = tof_days as f64 * 86400.0;
    let arrival_time = departure_time + tof;

    // Get source body's heliocentric state at departure
    let source_elems = propagate_elliptic(source_body.orbital_elements, MU_SUN, departure_time)
        .unwrap_or(source_body.orbital_elements);
    let (source_pos, source_vel) = coe_to_rv(&source_elems, MU_SUN);

    // Get target body's heliocentric state at arrival
    let target_elems = propagate_elliptic(target_body.orbital_elements, MU_SUN, arrival_time)
        .unwrap_or(target_body.orbital_elements);
    let (target_pos, target_vel) = coe_to_rv(&target_elems, MU_SUN);

    // Solve Lambert's problem
    compute_transfer(source_pos, source_vel, target_pos, target_vel, tof, MU_SUN).ok()
}

// ============================================================================
// Systems
// ============================================================================

/// Populates the transfer cache with all solutions in the search window.
/// Computes transfers from player's current body to all siblings (same parent).
pub fn init_transfer_cache(
    bodies: Query<(Entity, &Body)>,
    sim_time: Res<SimulationTime>,
    player_query: Query<&crate::ship::ShipState, With<crate::ship::PlayerControlled>>,
    mut cache: ResMut<TransferCache>,
) {
    // Get player's current body
    let Ok(player_state) = player_query.single() else {
        warn!("No player ship found, cannot initialize transfer cache");
        return;
    };

    let current_entity = match player_state {
        crate::ship::ShipState::Orbiting { body } => *body,
        crate::ship::ShipState::Transferring { .. } => {
            warn!("Ship in transit, cannot initialize transfer cache");
            return;
        }
    };

    // Get source body by entity
    let Some(source_body) = bodies.iter()
        .find(|(e, _)| *e == current_entity)
        .map(|(_, b)| b) else {
        warn!("Source body entity not found");
        return;
    };

    // Find all siblings (bodies with the same parent entity)
    let siblings: Vec<(Entity, &Body)> = bodies
        .iter()
        .filter(|(e, b)| b.parent_entity == source_body.parent_entity && *e != current_entity)
        .collect();

    if siblings.is_empty() {
        warn!("No sibling bodies found for '{}'", source_body.name);
        return;
    }

    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
    cache.last_update_day = current_day;
    cache.window_days = SEARCH_WINDOW_DAYS;
    cache.source_body = Some(current_entity);

    // Compute all solutions in the window for each sibling
    let mut computed = 0;
    for (target_entity, target_body) in &siblings {
        for dep_offset in 0..=SEARCH_WINDOW_DAYS {
            let departure_day = current_day + dep_offset;
            for &tof_days in &TOF_CANDIDATES {
                if let Some(solution) = compute_cached_transfer(source_body, target_body, departure_day, tof_days) {
                    cache.solutions.insert((*target_entity, departure_day, tof_days), solution);
                    computed += 1;
                }
            }
        }
    }

    // Find the range of departure days in the cache
    let min_dep = cache.solutions.keys().map(|(_, d, _)| *d).min().unwrap_or(0);
    let max_dep = cache.solutions.keys().map(|(_, d, _)| *d).max().unwrap_or(0);
    info!(
        "Transfer cache initialized: {} solutions for {} targets, departure days {}-{}",
        computed, siblings.len(), min_dep, max_dep
    );
}

/// Incrementally updates the transfer cache as simulation time advances.
/// - Checks if source body changed (triggers full rebuild)
/// - Prunes old solutions (departure day < current day)
/// - Adds new solutions at the far end of the window
pub fn update_transfer_cache(
    bodies: Query<(Entity, &Body)>,
    sim_time: Res<SimulationTime>,
    player_query: Query<&crate::ship::ShipState, With<crate::ship::PlayerControlled>>,
    mut cache: ResMut<TransferCache>,
) {
    // Get player's current body
    let Ok(player_state) = player_query.single() else {
        return;
    };

    let current_entity = match player_state {
        crate::ship::ShipState::Orbiting { body } => *body,
        crate::ship::ShipState::Transferring { .. } => {
            // Ship in transit - don't update cache
            return;
        }
    };

    // Get source body by entity
    let Some(source_body) = bodies.iter()
        .find(|(e, _)| *e == current_entity)
        .map(|(_, b)| b) else {
        return;
    };

    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    // Check if source body changed -> need full rebuild
    if cache.source_body != Some(current_entity) {
        info!("Source body changed to {}, rebuilding cache...", source_body.name);

        cache.solutions.clear();
        cache.source_body = Some(current_entity);
        cache.last_update_day = current_day;
        cache.window_days = SEARCH_WINDOW_DAYS;

        // Find all siblings (bodies with the same parent entity)
        let siblings: Vec<(Entity, &Body)> = bodies
            .iter()
            .filter(|(e, b)| b.parent_entity == source_body.parent_entity && *e != current_entity)
            .collect();

        if siblings.is_empty() {
            warn!("No sibling bodies found for '{}'", source_body.name);
            return;
        }

        // Compute all solutions in the window for each sibling
        let mut computed = 0;
        for (target_entity, target_body) in &siblings {
            for dep_offset in 0..=SEARCH_WINDOW_DAYS {
                let departure_day = current_day + dep_offset;
                for &tof_days in &TOF_CANDIDATES {
                    if let Some(solution) = compute_cached_transfer(source_body, target_body, departure_day, tof_days) {
                        cache.solutions.insert((*target_entity, departure_day, tof_days), solution);
                        computed += 1;
                    }
                }
            }
        }

        info!(
            "Transfer cache rebuilt: {} solutions for {} targets from {}",
            computed, siblings.len(), source_body.name
        );
        return;
    }

    // Only update if we've moved to a new day
    if current_day <= cache.last_update_day {
        return;
    }

    // Find all siblings (same parent entity as source)
    let siblings: Vec<(Entity, &Body)> = bodies
        .iter()
        .filter(|(e, b)| b.parent_entity == source_body.parent_entity && *e != current_entity)
        .collect();

    let before_count = cache.solutions.len();

    // Prune old solutions (departure day in the past)
    cache.solutions.retain(|(_, dep_day, _), _| *dep_day >= current_day);

    let after_prune = cache.solutions.len();
    let pruned = before_count - after_prune;

    // Add new solutions at the far end of the window for all siblings
    let days_advanced = current_day - cache.last_update_day;
    let mut added = 0;
    for offset in 0..days_advanced {
        let new_departure_day = cache.last_update_day + cache.window_days + 1 + offset;
        for (target_entity, target_body) in &siblings {
            for &tof_days in &TOF_CANDIDATES {
                if let Some(solution) = compute_cached_transfer(source_body, target_body, new_departure_day, tof_days) {
                    cache.solutions.insert((*target_entity, new_departure_day, tof_days), solution);
                    added += 1;
                }
            }
        }
    }

    if pruned > 0 || added > 0 {
        info!(
            "Cache update: day {} -> {}, pruned {}, added {}, total {}",
            cache.last_update_day, current_day, pruned, added, cache.solutions.len()
        );
    }

    cache.last_update_day = current_day;
}

// ============================================================================
// Queries
// ============================================================================

/// Finds the best (lowest delta-v) transfer in a day range for a specific target.
/// Returns (departure_day, solution).
pub fn find_best_transfer_in_range<'a>(
    cache: &'a TransferCache,
    target_entity: Entity,
    start_day: i32,
    end_day: i32,
) -> Option<(i32, &'a TransferSolution)> {
    cache
        .solutions
        .iter()
        .filter(|((entity, dep_day, _), _)| {
            *entity == target_entity && *dep_day >= start_day && *dep_day < end_day
        })
        .min_by(|(_, a), (_, b)| a.total_dv.partial_cmp(&b.total_dv).unwrap())
        .map(|((_, dep_day, _), sol)| (*dep_day, sol))
}
