//! Transfer solution caching system.
//!
//! Precomputes and caches Lambert transfer solutions between celestial bodies
//! within a rolling time window. Supports multiple source bodies to enable
//! multi-hop transfer planning. This allows instant UI response when selecting
//! transfer options.
//!
//! Environment variables:
//! - `CACHE_LOG_INTERVAL`: How often to log cache updates (in days). Default: 0 (every update).

use std::sync::OnceLock;

use astrora_core::core::elements::{OrbitalElements, coe_to_rv};
use bevy::platform::collections::HashMap;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};

use crate::orbital_data::{Body, MU_SUN, propagate_elliptic};
use crate::simulation::SimulationTime;
use crate::transfer::{TransferSolution, compute_transfer};

/// Returns the cache log interval from CACHE_LOG_INTERVAL env var (default: 0 = every update)
fn cache_log_interval() -> i32 {
    static INTERVAL: OnceLock<i32> = OnceLock::new();
    *INTERVAL.get_or_init(|| {
        std::env::var("CACHE_LOG_INTERVAL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    })
}

// ============================================================================
// Resources
// ============================================================================

/// Cache key for Lambert solutions: (source_entity, target_entity, departure_day, tof_days)
/// departure_day is days since J2000 epoch (can be negative)
type CacheKey = (Entity, Entity, i32, i32);

/// Cached Lambert transfer solutions between celestial bodies.
/// Stores computed solutions keyed by (source, target, departure_day, tof_days).
/// Supports multiple source bodies for multi-hop transfer planning.
#[derive(Resource, Default)]
pub struct TransferCache {
    /// Map from (source, target, departure_day, tof_days) to computed solution
    pub solutions: HashMap<CacheKey, TransferSolution>,
    /// Last sim_time day we updated the cache (for incremental updates)
    pub last_update_day: i32,
    /// Search window: how many days ahead to search for departures
    pub window_days: i32,
    /// Source bodies that have been cached (for incremental updates)
    pub cached_sources: HashSet<Entity>,
    /// Last day we logged an update (for CACHE_LOG_INTERVAL)
    pub last_log_day: i32,
}

// ============================================================================
// Async Computation Types
// ============================================================================

/// Minimal cloned body data needed for async cache computation.
/// All fields are Send + Sync, allowing computation on worker threads.
#[derive(Clone)]
pub struct BodySnapshot {
    pub entity: Entity,
    pub orbital_elements: OrbitalElements,
}

impl BodySnapshot {
    pub fn from_body(entity: Entity, body: &Body) -> Self {
        Self {
            entity,
            orbital_elements: body.orbital_elements,
        }
    }
}

/// Component tracking a pending async cache computation.
/// Spawned when ship enters transfer or when queueing from a new source.
#[derive(Component)]
pub struct PendingCacheCompute {
    task: Task<(Entity, HashMap<CacheKey, TransferSolution>, i32)>,
    pub source: Entity,
}

// ============================================================================
// Constants
// ============================================================================

/// TOF candidates to evaluate (in days)
/// Wide range to ensure at least one valid solution exists for any departure day
const TOF_CANDIDATES: [i32; 13] = [
    100, 120, 150, 180, 200, 220, 250, 280, 300, 350, 400, 450, 500,
];

/// How far ahead to search for departure windows (days)
const SEARCH_WINDOW_DAYS: i32 = 500;

// ============================================================================
// Helper Functions
// ============================================================================

/// Computes a single Lambert transfer 
fn compute_cached_transfer(
    source: &OrbitalElements,
    target: &OrbitalElements,
    departure_day: i32,
    tof_days: i32,
) -> Option<TransferSolution> {
    let departure_time = departure_day as f64 * 86400.0;
    let tof = tof_days as f64 * 86400.0;
    let arrival_time = departure_time + tof;

    // Get source body's heliocentric state at departure
    let source_elems = propagate_elliptic(*source, MU_SUN, departure_time).unwrap_or(*source);
    let (source_pos, source_vel) = coe_to_rv(&source_elems, MU_SUN);

    // Get target body's heliocentric state at arrival
    let target_elems = propagate_elliptic(*target, MU_SUN, arrival_time).unwrap_or(*target);
    let (target_pos, target_vel) = coe_to_rv(&target_elems, MU_SUN);

    // Solve Lambert's problem
    compute_transfer(source_pos, source_vel, target_pos, target_vel, tof, MU_SUN).ok()
}

/// Pure function to compute all transfers from source to all targets.
/// Designed to run on a worker thread - no ECS access.
/// Returns (source_entity, solutions_map, last_update_day).
fn compute_cache_for_body(
    source: BodySnapshot,
    targets: Vec<BodySnapshot>,
    start_day: i32,
) -> (Entity, HashMap<CacheKey, TransferSolution>, i32) {
    let mut solutions = HashMap::new();
    let source_entity = source.entity;

    for target in &targets {
        for dep_offset in 0..=SEARCH_WINDOW_DAYS {
            let departure_day = start_day + dep_offset;
            for &tof_days in &TOF_CANDIDATES {
                if let Some(solution) = compute_cached_transfer(
                    &source.orbital_elements,
                    &target.orbital_elements,
                    departure_day,
                    tof_days,
                ) {
                    solutions.insert(
                        (source_entity, target.entity, departure_day, tof_days),
                        solution,
                    );
                }
            }
        }
    }

    (source_entity, solutions, start_day)
}

// ============================================================================
// Systems
// ============================================================================

/// Populates the transfer cache with all solutions in the search window.
/// Computes transfers from player's current body to all other bodies.
pub fn init_transfer_cache(
    bodies: Query<(Entity, &Body)>,
    sim_time: Res<SimulationTime>,
    player_query: Query<&crate::ship::ShipLocation, With<crate::ship::PlayerControlled>>,
    mut cache: ResMut<TransferCache>,
) {
    // Get player's current body
    let Ok(location) = player_query.single() else {
        warn!("No player ship found, cannot initialize transfer cache");
        return;
    };

    let current_entity = match location {
        crate::ship::ShipLocation::AtBody(body) => *body,
        crate::ship::ShipLocation::InTransit { .. } => {
            warn!("Ship in transit, cannot initialize transfer cache");
            return;
        }
    };

    // Get source body by entity
    let Some(source_body) = bodies
        .iter()
        .find(|(e, _)| *e == current_entity)
        .map(|(_, b)| b)
    else {
        warn!("Source body entity not found");
        return;
    };

    // Find all other bodies (transfer targets)
    let targets: Vec<(Entity, &Body)> = bodies
        .iter()
        .filter(|(e, _)| *e != current_entity)
        .collect();

    if targets.is_empty() {
        warn!("No target bodies found for '{}'", source_body.name);
        return;
    }

    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
    cache.last_update_day = current_day;
    cache.window_days = SEARCH_WINDOW_DAYS;
    cache.cached_sources.insert(current_entity);

    // Compute all solutions in the window for each target
    let mut computed = 0;
    for (target_entity, target_body) in &targets {
        for dep_offset in 0..=SEARCH_WINDOW_DAYS {
            let departure_day = current_day + dep_offset;
            for &tof_days in &TOF_CANDIDATES {
                if let Some(solution) =
                    compute_cached_transfer(&source_body.orbital_elements, &target_body.orbital_elements, departure_day, tof_days)
                {
                    cache.solutions.insert(
                        (current_entity, *target_entity, departure_day, tof_days),
                        solution,
                    );
                    computed += 1;
                }
            }
        }
    }

    // Find the range of departure days in the cache
    let min_dep = cache
        .solutions
        .keys()
        .map(|(_, _, d, _)| *d)
        .min()
        .unwrap_or(0);
    let max_dep = cache
        .solutions
        .keys()
        .map(|(_, _, d, _)| *d)
        .max()
        .unwrap_or(0);
    info!(
        "Transfer cache initialized: {} solutions for {} targets, departure days {}-{}",
        computed,
        targets.len(),
        min_dep,
        max_dep
    );
}

/// Incrementally updates the transfer cache as simulation time advances.
/// - Adds current body to cache if not already cached
/// - Prunes unused sources (keeps current body, transfer destination, planned destinations)
/// - Prunes old solutions (departure day < current day)
/// - Adds new solutions at the far end of the window for all cached sources
pub fn update_transfer_cache(
    bodies: Query<(Entity, &Body)>,
    sim_time: Res<SimulationTime>,
    player_query: Query<
        (&crate::ship::ShipLocation, &crate::ship::FlightPlan),
        With<crate::ship::PlayerControlled>,
    >,
    pending_tasks: Query<&PendingCacheCompute>,
    mut cache: ResMut<TransferCache>,
) {
    // Get player's current state and plan
    let Ok((location, plan)) = player_query.single() else {
        return;
    };

    // Build set of relevant sources to keep:
    // - Current body (if at body)
    // - Current transfer destination (if in transit)
    // - All planned leg destinations
    let mut relevant_sources: HashSet<Entity> = HashSet::default();

    let current_entity = match location {
        crate::ship::ShipLocation::AtBody(body) => {
            relevant_sources.insert(*body);
            *body
        }
        crate::ship::ShipLocation::InTransit { target, .. } => {
            // Add current transfer destination
            relevant_sources.insert(*target);
            // Ship in transit - don't update cache (but still prune below)
            Entity::PLACEHOLDER
        }
    };

    // Add all planned destinations (committed or not)
    for leg in &plan.legs {
        relevant_sources.insert(leg.target);
    }

    // Prune sources that are no longer relevant
    let removed_sources: Vec<Entity> = cache
        .cached_sources
        .iter()
        .filter(|e| !relevant_sources.contains(*e))
        .copied()
        .collect();

    if !removed_sources.is_empty() {
        // Get names for logging
        let removed_names: Vec<&str> = removed_sources
            .iter()
            .filter_map(|e| bodies.iter().find(|(be, _)| be == e).map(|(_, b)| b.name.as_str()))
            .collect();

        // Remove from cached_sources
        for entity in &removed_sources {
            cache.cached_sources.remove(entity);
        }

        // Remove solutions for these sources
        let solutions_before = cache.solutions.len();
        cache.solutions.retain(|(src, _, _, _), _| !removed_sources.contains(src));
        let solutions_removed = solutions_before - cache.solutions.len();

        info!(
            "Pruned {} unused source(s): [{}], removed {} solutions",
            removed_sources.len(),
            removed_names.join(", "),
            solutions_removed
        );
    }

    // If in transit, don't do incremental updates
    if matches!(location, crate::ship::ShipLocation::InTransit { .. }) {
        return;
    }

    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    // Check if current body needs to be added to cache
    if !cache.cached_sources.contains(&current_entity) {
        // Check if async task is already computing cache for this source
        let async_pending = pending_tasks.iter().any(|p| p.source == current_entity);
        if async_pending {
            // Async task will handle it
            return;
        }

        // Get source body by entity
        let Some(source_body) = bodies
            .iter()
            .find(|(e, _)| *e == current_entity)
            .map(|(_, b)| b)
        else {
            return;
        };

        info!("Adding {} to cache...", source_body.name);

        // Find all other bodies (transfer targets)
        let targets: Vec<(Entity, &Body)> = bodies
            .iter()
            .filter(|(e, _)| *e != current_entity)
            .collect();

        if targets.is_empty() {
            warn!("No target bodies found for '{}'", source_body.name);
            return;
        }

        // Compute all solutions in the window for each target
        let mut computed = 0;
        for (target_entity, target_body) in &targets {
            for dep_offset in 0..=SEARCH_WINDOW_DAYS {
                let departure_day = current_day + dep_offset;
                for &tof_days in &TOF_CANDIDATES {
                    if let Some(solution) =
                        compute_cached_transfer(&source_body.orbital_elements, &target_body.orbital_elements, departure_day, tof_days)
                    {
                        cache.solutions.insert(
                            (current_entity, *target_entity, departure_day, tof_days),
                            solution,
                        );
                        computed += 1;
                    }
                }
            }
        }

        cache.cached_sources.insert(current_entity);
        cache.last_update_day = current_day;
        cache.window_days = SEARCH_WINDOW_DAYS;

        info!(
            "Cache updated: added {} solutions from {}, total sources: {}",
            computed,
            source_body.name,
            cache.cached_sources.len()
        );
        return;
    }

    // Only update if we've moved to a new day
    if current_day <= cache.last_update_day {
        return;
    }

    let before_count = cache.solutions.len();

    // Prune old solutions (departure day in the past) for all sources
    cache
        .solutions
        .retain(|(_, _, dep_day, _), _| *dep_day >= current_day);

    let after_prune = cache.solutions.len();
    let pruned = before_count - after_prune;

    // Cap the number of days to process per frame to avoid FPS drops at high time scales
    const MAX_DAYS_PER_FRAME: i32 = 5;
    let days_advanced = (current_day - cache.last_update_day).min(MAX_DAYS_PER_FRAME);
    let mut added = 0;

    // Collect source entities and names to iterate over (to avoid borrow issues)
    let cached_sources: Vec<(Entity, String)> = cache
        .cached_sources
        .iter()
        .filter_map(|&e| {
            bodies
                .iter()
                .find(|(be, _)| *be == e)
                .map(|(_, b)| (e, b.name.clone()))
        })
        .collect();

    let source_names: Vec<&str> = cached_sources.iter().map(|(_, n)| n.as_str()).collect();

    for (source_entity, _) in &cached_sources {
        let Some(source_body) = bodies
            .iter()
            .find(|(e, _)| e == source_entity)
            .map(|(_, b)| b)
        else {
            continue;
        };

        let targets: Vec<(Entity, &Body)> =
            bodies.iter().filter(|(e, _)| e != source_entity).collect();

        for offset in 0..days_advanced {
            let new_departure_day = cache.last_update_day + cache.window_days + 1 + offset;
            for (target_entity, target_body) in &targets {
                for &tof_days in &TOF_CANDIDATES {
                    if let Some(solution) = compute_cached_transfer(
                        &source_body.orbital_elements,
                        &target_body.orbital_elements,
                        new_departure_day,
                        tof_days,
                    ) {
                        cache.solutions.insert(
                            (*source_entity, *target_entity, new_departure_day, tof_days),
                            solution,
                        );
                        added += 1;
                    }
                }
            }
        }
    }

    // Update last_update_day by only the days we actually processed
    let new_last_update_day = cache.last_update_day + days_advanced;

    // Check if we should log based on interval
    let log_interval = cache_log_interval();
    let should_log = if log_interval <= 0 {
        // Log every update if interval is 0 or negative
        pruned > 0 || added > 0
    } else {
        // Log every N days
        new_last_update_day - cache.last_log_day >= log_interval
    };

    if should_log && (pruned > 0 || added > 0) {
        info!(
            "Cache update: day {} -> {}, pruned {}, added {}, total {}, sources: [{}]",
            cache.last_update_day,
            new_last_update_day,
            pruned,
            added,
            cache.solutions.len(),
            source_names.join(", ")
        );
        cache.last_log_day = new_last_update_day;
    }

    cache.last_update_day = new_last_update_day;
}

// ============================================================================
// Queries
// ============================================================================

/// Finds the best (lowest delta-v) transfer in a day range for a specific source and target.
/// Returns (departure_day, solution).
/// Returns (departure_day, tof_days, solution) for the best transfer in range.
/// The tof_days is the exact cache key - use this when storing PlannedLeg.
pub fn find_best_transfer_in_range<'a>(
    cache: &'a TransferCache,
    source_entity: Entity,
    target_entity: Entity,
    start_day: i32,
    end_day: i32,
) -> Option<(i32, i32, &'a TransferSolution)> {
    cache
        .solutions
        .iter()
        .filter(|((src, tgt, dep_day, _), _)| {
            *src == source_entity
                && *tgt == target_entity
                && *dep_day >= start_day
                && *dep_day < end_day
        })
        .min_by(|(_, a), (_, b)| a.total_dv.partial_cmp(&b.total_dv).unwrap())
        .map(|((_, _, dep_day, tof_days), sol)| (*dep_day, *tof_days, sol))
}

/// Checks if a source body has been cached.
pub fn is_source_cached(cache: &TransferCache, source_entity: Entity) -> bool {
    cache.cached_sources.contains(&source_entity)
}

// ============================================================================
// Async Cache Systems
// ============================================================================

/// Spawns an async task to precompute the transfer cache when ship enters transfer.
/// Triggered when ShipLocation changes to InTransit.
pub fn spawn_cache_compute_task(
    mut commands: Commands,
    ships: Query<
        &crate::ship::ShipLocation,
        (
            With<crate::ship::PlayerControlled>,
            Changed<crate::ship::ShipLocation>,
        ),
    >,
    bodies: Query<(Entity, &Body)>,
    pending: Query<&PendingCacheCompute>,
    cache: Res<TransferCache>,
) {
    for location in &ships {
        // Only trigger on InTransit state
        let crate::ship::ShipLocation::InTransit {
            target,
            solution,
            departure_time,
        } = location
        else {
            continue;
        };

        let arrival_time = departure_time + solution.time_of_flight;

        // Skip if already cached or pending
        if cache.cached_sources.contains(target) {
            continue;
        }
        if pending.iter().any(|p| p.source == *target) {
            continue;
        }

        // Get destination body
        let Some((_, dest_body)) = bodies.iter().find(|(e, _)| *e == *target) else {
            warn!("Destination body not found for async cache compute");
            continue;
        };

        // Find all other bodies (transfer targets from destination)
        let targets: Vec<BodySnapshot> = bodies
            .iter()
            .filter(|(e, _)| *e != *target)
            .map(|(e, b)| BodySnapshot::from_body(e, b))
            .collect();

        if targets.is_empty() {
            warn!("No target bodies found at destination");
            continue;
        }

        // Create snapshot of destination body
        let source_snapshot = BodySnapshot::from_body(*target, dest_body);
        let arrival_day = (arrival_time / 86400.0).floor() as i32;
        let num_targets = targets.len();

        info!(
            "Spawning async cache compute for {} targets at {}, starting day {}",
            num_targets, dest_body.name, arrival_day
        );

        // Spawn the async task
        let task = AsyncComputeTaskPool::get()
            .spawn(async move { compute_cache_for_body(source_snapshot, targets, arrival_day) });

        commands.spawn(PendingCacheCompute {
            task,
            source: *target,
        });
    }
}

/// Spawns an async task to compute cache for a specific source body.
/// Called when queueing transfers from a body that isn't cached yet.
pub fn request_cache_for_source(
    commands: &mut Commands,
    bodies: &Query<(Entity, &Body)>,
    pending: &Query<&PendingCacheCompute>,
    cache: &TransferCache,
    source_entity: Entity,
    start_day: i32,
) {
    // Skip if already cached or pending
    if cache.cached_sources.contains(&source_entity) {
        return;
    }
    if pending.iter().any(|p| p.source == source_entity) {
        return;
    }

    // Get source body
    let Some((_, source_body)) = bodies.iter().find(|(e, _)| *e == source_entity) else {
        warn!("Source body not found for cache compute");
        return;
    };

    // Find all other bodies (transfer targets)
    let targets: Vec<BodySnapshot> = bodies
        .iter()
        .filter(|(e, _)| *e != source_entity)
        .map(|(e, b)| BodySnapshot::from_body(e, b))
        .collect();

    if targets.is_empty() {
        warn!("No target bodies found for cache compute");
        return;
    }

    let source_snapshot = BodySnapshot::from_body(source_entity, source_body);
    let num_targets = targets.len();

    info!(
        "Spawning async cache compute for {} targets from {}, starting day {}",
        num_targets, source_body.name, start_day
    );

    let task = AsyncComputeTaskPool::get()
        .spawn(async move { compute_cache_for_body(source_snapshot, targets, start_day) });

    commands.spawn(PendingCacheCompute {
        task,
        source: source_entity,
    });
}

/// Polls pending async cache tasks and applies results when complete.
pub fn poll_cache_compute_task(
    mut commands: Commands,
    mut pending: Query<(Entity, &mut PendingCacheCompute)>,
    mut cache: ResMut<TransferCache>,
) {
    for (entity, mut pending_task) in &mut pending {
        if let Some((source_entity, solutions, last_day)) =
            block_on(future::poll_once(&mut pending_task.task))
        {
            let count = solutions.len();

            // Merge results into cache (don't replace - we support multiple sources)
            cache.solutions.extend(solutions);
            cache.cached_sources.insert(source_entity);
            cache.last_update_day = cache.last_update_day.max(last_day);
            cache.window_days = SEARCH_WINDOW_DAYS;

            info!(
                "Async cache ready: {} solutions from source, total sources: {}",
                count,
                cache.cached_sources.len()
            );

            // Clean up the task entity
            commands.entity(entity).despawn();
        }
    }
}
