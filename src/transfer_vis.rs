//! Transfer trajectory visualization.
//!
//! Renders Lambert transfer arcs between celestial bodies with burn markers
//! showing departure and arrival delta-v requirements.

use std::collections::HashMap;

use bevy::{
    asset::Assets,
    gizmos::GizmoAsset,
    prelude::*,
};
use bevy_vector_shapes::prelude::*;
use astrora_core::core::{Vector3, elements::coe_to_rv};

use crate::{
    orbital_data::{Body, MU_SUN, propagate_elliptic, PlanetaryElements},
    transfer::{compute_transfer, TransferSolution, propagate_kepler},
    simulation::SimulationTime,
    phys_to_visual,
};

// ============================================================================
// Constants
// ============================================================================

/// Number of line segments for transfer arc visualization
const TRANSFER_ARC_SEGMENTS: usize = 500;

/// Transfer arc color (orange, semi-transparent)
const TRANSFER_COLOR: Color = Color::srgba(1.0, 0.6, 0.2, 0.8);

/// Departure burn arrow color (green)
const DEPARTURE_COLOR: Color = Color::srgb(0.3, 0.9, 0.3);

/// Arrival burn arrow color (red)
const ARRIVAL_COLOR: Color = Color::srgb(0.9, 0.3, 0.3);

// ============================================================================
// Components
// ============================================================================

/// A computed transfer trajectory between two bodies.
#[derive(Component)]
pub struct Transfer {
    /// Departure body entity
    pub source: Entity,
    /// Arrival body entity
    pub target: Entity,
    /// Computed transfer solution (delta-v, orbit, etc.)
    pub solution: TransferSolution,
    /// Simulation time at departure (used for future animation/progress tracking)
    #[allow(dead_code)]
    pub departure_time: f64,
}

/// Marker for the transfer arc gizmo entity.
#[derive(Component)]
pub struct TransferArc {
    /// Parent transfer entity (used for future arc updates)
    #[allow(dead_code)]
    pub transfer: Entity,
}

/// Marker for burn visualization points.
#[derive(Component)]
pub struct BurnMarker {
    /// Parent transfer entity
    pub transfer: Entity,
    /// True = departure burn, False = arrival burn
    pub is_departure: bool,
    /// Delta-v vector in physics coordinates (m/s)
    pub delta_v: Vector3,
}

// ============================================================================
// Resources
// ============================================================================

/// Currently active transfer for UI display.
#[derive(Resource, Default)]
pub struct ActiveTransfer {
    pub entity: Option<Entity>,
}

/// Cache key for Lambert solutions: (departure_day, tof_days)
/// departure_day is days since J2000 epoch (can be negative)
type CacheKey = (i32, i32);

/// Cached Lambert transfer solutions.
/// Stores computed solutions keyed by (departure_day, tof_days).
#[derive(Resource, Default)]
pub struct TransferCache {
    /// Map from (departure_day, tof_days) to computed solution
    pub solutions: HashMap<CacheKey, TransferSolution>,
    /// Last sim_time day we updated the cache (for incremental updates)
    pub last_update_day: i32,
    /// Search window: how many days ahead to search for departures
    pub window_days: i32,
}

/// TOF candidates to evaluate (in days)
/// Wide range to ensure at least one valid solution exists for any departure day
const TOF_CANDIDATES: [i32; 13] = [100, 120, 150, 180, 200, 220, 250, 280, 300, 350, 400, 450, 500];

/// How far ahead to search for departure windows (days)
const SEARCH_WINDOW_DAYS: i32 = 500;

// ============================================================================
// Startup Systems
// ============================================================================

/// Computes a single Lambert transfer and returns the solution if valid.
fn compute_cached_transfer(
    earth_body: &Body,
    mars_body: &Body,
    departure_day: i32,
    tof_days: i32,
) -> Option<TransferSolution> {
    let departure_time = departure_day as f64 * 86400.0;
    let tof = tof_days as f64 * 86400.0;
    let arrival_time = departure_time + tof;

    // Get Earth's heliocentric state at departure
    let earth_elems = propagate_elliptic(earth_body.orbital_elements, MU_SUN, departure_time)
        .unwrap_or(earth_body.orbital_elements);
    let (earth_pos, earth_vel) = coe_to_rv(&earth_elems, MU_SUN);

    // Get Mars's heliocentric state at arrival
    let mars_elems = propagate_elliptic(mars_body.orbital_elements, MU_SUN, arrival_time)
        .unwrap_or(mars_body.orbital_elements);
    let (mars_pos, mars_vel) = coe_to_rv(&mars_elems, MU_SUN);

    // Solve Lambert's problem
    compute_transfer(earth_pos, earth_vel, mars_pos, mars_vel, tof, MU_SUN).ok()
}

/// Populates the transfer cache with all solutions in the search window.
pub fn init_transfer_cache(
    bodies: Query<&Body>,
    sim_time: Res<SimulationTime>,
    mut cache: ResMut<TransferCache>,
) {
    let Some(earth_body) = bodies.iter().find(|b| b.name == "Earth") else {
        warn!("Earth not found, cannot initialize transfer cache");
        return;
    };
    let Some(mars_body) = bodies.iter().find(|b| b.name == "Mars") else {
        warn!("Mars not found, cannot initialize transfer cache");
        return;
    };

    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
    cache.last_update_day = current_day;
    cache.window_days = SEARCH_WINDOW_DAYS;

    // Compute all solutions in the window
    let mut computed = 0;
    for dep_offset in 0..=SEARCH_WINDOW_DAYS {
        let departure_day = current_day + dep_offset;
        for &tof_days in &TOF_CANDIDATES {
            if let Some(solution) = compute_cached_transfer(earth_body, mars_body, departure_day, tof_days) {
                cache.solutions.insert((departure_day, tof_days), solution);
                computed += 1;
            }
        }
    }

    // Find the range of departure days in the cache
    let min_dep = cache.solutions.keys().map(|(d, _)| *d).min().unwrap_or(0);
    let max_dep = cache.solutions.keys().map(|(d, _)| *d).max().unwrap_or(0);
    info!(
        "Transfer cache initialized: {} solutions, departure days {}-{}",
        computed, min_dep, max_dep
    );
}

/// Spawns the initial transfer visualization from the cache.
pub fn spawn_initial_transfer(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    bodies: Query<(Entity, &Body)>,
    sim_time: Res<SimulationTime>,
    cache: Res<TransferCache>,
    mut active_transfer: ResMut<ActiveTransfer>,
) {
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    // Find best solution in cache
    let best = find_best_transfer_today(&cache, current_day);
    let Some((key, solution)) = best else {
        warn!("No valid transfer found in cache");
        return;
    };

    info!(
        "Best transfer: dep_day={} tof={}d dv={:.0} m/s (arr_day={})",
        key.0, key.1, solution.total_dv, key.0 + key.1
    );

    // Find Earth and Mars entities
    let Some((earth_entity, _)) = bodies.iter().find(|(_, b)| b.name == "Earth") else {
        return;
    };
    let Some((mars_entity, _)) = bodies.iter().find(|(_, b)| b.name == "Mars") else {
        return;
    };

    // Spawn the visualization
    let transfer_entity = spawn_transfer_visualization(
        &mut commands,
        &mut gizmo_assets,
        earth_entity,
        mars_entity,
        solution,
        key.0 as f64 * 86400.0,
    );

    active_transfer.entity = Some(transfer_entity);
}

/// Finds the best (lowest delta-v) transfer departing on the current day.
/// Falls back to the nearest future day if no solutions exist for today.
fn find_best_transfer_today(cache: &TransferCache, current_day: i32) -> Option<(CacheKey, &TransferSolution)> {
    // First, try to find a solution departing today
    let today_best = cache
        .solutions
        .iter()
        .filter(|((dep_day, _), _)| *dep_day == current_day)
        .min_by(|(_, a), (_, b)| a.total_dv.partial_cmp(&b.total_dv).unwrap());

    if let Some((k, v)) = today_best {
        return Some((*k, v));
    }

    // Fall back to the earliest future day that has solutions
    let min_future_day = cache
        .solutions
        .keys()
        .filter(|(dep_day, _)| *dep_day > current_day)
        .map(|(dep_day, _)| *dep_day)
        .min()?;

    cache
        .solutions
        .iter()
        .filter(|((dep_day, _), _)| *dep_day == min_future_day)
        .min_by(|(_, a), (_, b)| a.total_dv.partial_cmp(&b.total_dv).unwrap())
        .map(|(k, v)| (*k, v))
}

/// Finds the best (lowest delta-v) transfer in the entire future window.
/// Useful for showing the optimal launch window.
#[allow(dead_code)]
fn find_best_transfer_in_window(cache: &TransferCache, current_day: i32) -> Option<(CacheKey, &TransferSolution)> {
    cache
        .solutions
        .iter()
        .filter(|((dep_day, _), _)| *dep_day >= current_day)
        .min_by(|(_, a), (_, b)| a.total_dv.partial_cmp(&b.total_dv).unwrap())
        .map(|(k, v)| (*k, v))
}

/// Spawns the transfer visualization entities and returns the transfer entity.
fn spawn_transfer_visualization(
    commands: &mut Commands,
    gizmo_assets: &mut ResMut<Assets<GizmoAsset>>,
    earth_entity: Entity,
    mars_entity: Entity,
    solution: &TransferSolution,
    departure_time: f64,
) -> Entity {
    // Create the transfer arc gizmo
    let arc_asset = create_transfer_arc(solution, MU_SUN);

    // Spawn the Transfer entity
    let transfer_entity = commands
        .spawn(Transfer {
            source: earth_entity,
            target: mars_entity,
            solution: solution.clone(),
            departure_time,
        })
        .id();

    // Spawn the arc gizmo
    commands.spawn((
        Gizmo {
            handle: gizmo_assets.add(arc_asset),
            depth_bias: 0.05,
            ..default()
        },
        TransferArc { transfer: transfer_entity },
    ));

    // Spawn burn markers
    commands.spawn(BurnMarker {
        transfer: transfer_entity,
        is_departure: true,
        delta_v: solution.departure_dv,
    });
    commands.spawn(BurnMarker {
        transfer: transfer_entity,
        is_departure: false,
        delta_v: solution.arrival_dv,
    });

    transfer_entity
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Creates a GizmoAsset containing the transfer arc linestrip.
/// Uses universal variable propagation for accurate trajectory.
fn create_transfer_arc(solution: &TransferSolution, mu: f64) -> GizmoAsset {
    let mut gizmo = GizmoAsset::new();

    let tof = solution.time_of_flight;
    let step_dt = tof / TRANSFER_ARC_SEGMENTS as f64;

    // Collect arc points by propagating state vector (r, v) using universal variables
    let mut points = Vec::with_capacity(TRANSFER_ARC_SEGMENTS + 1);

    // First point is the known departure position
    points.push(phys_to_visual(solution.departure_pos));

    let r0 = solution.departure_pos;
    let v0 = solution.departure_vel;

    for i in 1..TRANSFER_ARC_SEGMENTS {
        let t = i as f64 * step_dt;
        if let Some(r_vec) = propagate_kepler(r0, v0, mu, t) {
            points.push(phys_to_visual(r_vec));
        }
    }

    // Last point is the known arrival position
    points.push(phys_to_visual(solution.arrival_pos));

    // Verify propagation matches expected endpoints (warn only if mismatch)
    if let Some(propagated_end) = propagate_kepler(r0, v0, mu, tof) {
        let error = (propagated_end - solution.arrival_pos).norm();
        let arrival_dist = solution.arrival_pos.norm();
        let error_pct = 100.0 * error / arrival_dist;
        if error_pct > 1.0 {
            // Calculate transfer angle to understand the geometry
            let dot = r0.dot(&solution.arrival_pos);
            let transfer_angle = (dot / (r0.norm() * solution.arrival_pos.norm())).acos();
            warn!(
                "Transfer arc endpoint mismatch: {:.2e} m error ({:.1}%), transfer_angle={:.1}°",
                error, error_pct, transfer_angle.to_degrees()
            );
        }
    }

    gizmo.linestrip(points, TRANSFER_COLOR);
    gizmo
}

// ============================================================================
// Update Systems
// ============================================================================

/// Renders burn arrows using the shape painter.
pub fn render_burn_arrows(
    transfers: Query<&Transfer>,
    markers: Query<&BurnMarker>,
    mut painter: ShapePainter,
) {
    for marker in markers.iter() {
        let Ok(transfer) = transfers.get(marker.transfer) else {
            continue;
        };

        // Calculate position for this marker
        let position = if marker.is_departure {
            // Departure: at the computed departure position
            phys_to_visual(transfer.solution.departure_pos)
        } else {
            // Arrival: at the computed arrival position (where Mars will be)
            phys_to_visual(transfer.solution.arrival_pos)
        };

        // Determine color based on burn type
        let color = if marker.is_departure {
            DEPARTURE_COLOR
        } else {
            ARRIVAL_COLOR
        };

        // Scale arrow length by delta-v magnitude
        let dv_mag = marker.delta_v.norm();
        let arrow_len = (dv_mag / 1000.0).clamp(2.0, 15.0) as f32;

        // Direction of delta-v in visual space
        let dv_dir = Vec3::new(
            marker.delta_v.x as f32,
            marker.delta_v.y as f32,
            marker.delta_v.z as f32,
        )
        .normalize_or_zero();

        // Draw the arrow
        painter.set_translation(position);
        painter.set_color(color);
        painter.thickness = 0.3;
        painter.line(Vec3::ZERO, dv_dir * arrow_len);

        // Draw arrowhead (small lines at angle)
        let arrow_end = dv_dir * arrow_len;
        let perp = if dv_dir.x.abs() < 0.9 {
            dv_dir.cross(Vec3::X).normalize()
        } else {
            dv_dir.cross(Vec3::Y).normalize()
        };
        let head_size = arrow_len * 0.2;
        let head_back = -dv_dir * head_size;
        painter.line(arrow_end, arrow_end + head_back + perp * head_size * 0.5);
        painter.line(arrow_end, arrow_end + head_back - perp * head_size * 0.5);

        // Draw a circle marker at the position (departure=green, arrival=red)
        painter.circle(1.5);
    }
}

/// Incrementally updates the transfer cache as simulation time advances.
/// - Prunes old solutions (departure day < current day)
/// - Adds new solutions at the far end of the window
pub fn update_transfer_cache(
    bodies: Query<&Body>,
    sim_time: Res<SimulationTime>,
    mut cache: ResMut<TransferCache>,
) {
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    // Only update if we've moved to a new day
    if current_day <= cache.last_update_day {
        return;
    }

    let Some(earth_body) = bodies.iter().find(|b| b.name == "Earth") else {
        return;
    };
    let Some(mars_body) = bodies.iter().find(|b| b.name == "Mars") else {
        return;
    };

    let before_count = cache.solutions.len();

    // Prune old solutions (departure day in the past)
    cache.solutions.retain(|&(dep_day, _), _| dep_day >= current_day);

    let after_prune = cache.solutions.len();
    let pruned = before_count - after_prune;

    // Add new solutions at the far end of the window
    let days_advanced = current_day - cache.last_update_day;
    let mut added = 0;
    for offset in 0..days_advanced {
        let new_departure_day = cache.last_update_day + cache.window_days + 1 + offset;
        for &tof_days in &TOF_CANDIDATES {
            if let Some(solution) = compute_cached_transfer(earth_body, mars_body, new_departure_day, tof_days) {
                cache.solutions.insert((new_departure_day, tof_days), solution);
                added += 1;
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

/// Updates the transfer visualization when the best solution changes.
pub fn update_transfer_visualization(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    bodies: Query<(Entity, &Body)>,
    sim_time: Res<SimulationTime>,
    cache: Res<TransferCache>,
    mut active_transfer: ResMut<ActiveTransfer>,
    current_transfers: Query<&Transfer>,
    old_transfers: Query<Entity, With<Transfer>>,
    old_arcs: Query<Entity, With<TransferArc>>,
    old_markers: Query<Entity, With<BurnMarker>>,
) {
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    // Find the best solution in the cache
    let Some((best_key, best_solution)) = find_best_transfer_today(&cache, current_day) else {
        return;
    };

    // Check if we need to update the visualization
    let needs_update = if let Some(entity) = active_transfer.entity {
        if let Ok(current) = current_transfers.get(entity) {
            // Update if the displayed transfer's departure day doesn't match the best available
            let current_dep_day = (current.departure_time / 86400.0).floor() as i32;
            current_dep_day != best_key.0
        } else {
            true // Entity doesn't exist anymore
        }
    } else {
        true // No current transfer
    };

    if !needs_update {
        return;
    }

    info!(
        "Updating transfer: day {} dep={} tof={} dv={:.0}",
        current_day, best_key.0, best_key.1, best_solution.total_dv
    );

    // Despawn old entities
    for entity in old_transfers.iter() {
        commands.entity(entity).despawn();
    }
    for entity in old_arcs.iter() {
        commands.entity(entity).despawn();
    }
    for entity in old_markers.iter() {
        commands.entity(entity).despawn();
    }

    // Find Earth and Mars entities
    let Some((earth_entity, _)) = bodies.iter().find(|(_, b)| b.name == "Earth") else {
        return;
    };
    let Some((mars_entity, _)) = bodies.iter().find(|(_, b)| b.name == "Mars") else {
        return;
    };

    // Spawn new visualization
    let transfer_entity = spawn_transfer_visualization(
        &mut commands,
        &mut gizmo_assets,
        earth_entity,
        mars_entity,
        best_solution,
        best_key.0 as f64 * 86400.0,
    );

    active_transfer.entity = Some(transfer_entity);
}
