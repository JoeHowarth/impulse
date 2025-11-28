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
    /// If true, the user manually selected this transfer via popup.
    /// Auto-update will not overwrite user selections.
    pub user_selected: bool,
    /// Preview transfer entity (shown when hovering, dimmer color)
    pub preview_entity: Option<Entity>,
}

/// Marker for preview transfer arc (dimmer color, shown during hover)
#[derive(Component)]
pub struct PreviewTransferArc;

/// Preview arc color (dimmer orange)
const PREVIEW_TRANSFER_COLOR: Color = Color::srgba(1.0, 0.6, 0.2, 0.35);

/// Cache key for Lambert solutions: (target_name, departure_day, tof_days)
/// departure_day is days since J2000 epoch (can be negative)
type CacheKey = (String, i32, i32);

/// Cached Lambert transfer solutions from current body to all siblings.
/// Stores computed solutions keyed by (target_name, departure_day, tof_days).
#[derive(Resource, Default)]
pub struct TransferCache {
    /// Map from (target_name, departure_day, tof_days) to computed solution
    pub solutions: HashMap<CacheKey, TransferSolution>,
    /// Last sim_time day we updated the cache (for incremental updates)
    pub last_update_day: i32,
    /// Search window: how many days ahead to search for departures
    pub window_days: i32,
    /// Source body name (invalidate cache if this changes)
    pub source_body: Option<String>,
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

/// Populates the transfer cache with all solutions in the search window.
/// Computes transfers from CurrentBody to all siblings (same parent).
pub fn init_transfer_cache(
    bodies: Query<&Body>,
    sim_time: Res<SimulationTime>,
    current_body: Option<Res<crate::CurrentBody>>,
    mut cache: ResMut<TransferCache>,
) {
    let Some(ref current) = current_body else {
        warn!("CurrentBody not set, cannot initialize transfer cache");
        return;
    };

    let Some(ref current_name) = current.name else {
        warn!("CurrentBody has no name (ship in transit?)");
        return;
    };

    let Some(source_body) = bodies.iter().find(|b| b.name == *current_name) else {
        warn!("Source body '{}' not found", current_name);
        return;
    };

    // Find all siblings (bodies with the same parent)
    let siblings: Vec<&Body> = bodies
        .iter()
        .filter(|b| b.parent_name == source_body.parent_name && b.name != source_body.name)
        .collect();

    if siblings.is_empty() {
        warn!("No sibling bodies found for '{}'", source_body.name);
        return;
    }

    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
    cache.last_update_day = current_day;
    cache.window_days = SEARCH_WINDOW_DAYS;
    cache.source_body = Some(source_body.name.clone());

    // Compute all solutions in the window for each sibling
    let mut computed = 0;
    for target_body in &siblings {
        for dep_offset in 0..=SEARCH_WINDOW_DAYS {
            let departure_day = current_day + dep_offset;
            for &tof_days in &TOF_CANDIDATES {
                if let Some(solution) = compute_cached_transfer(source_body, target_body, departure_day, tof_days) {
                    cache.solutions.insert((target_body.name.clone(), departure_day, tof_days), solution);
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

/// Spawns the initial transfer visualization from the cache.
/// Shows the best transfer to Mars (default target for initial display).
pub fn spawn_initial_transfer(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    bodies: Query<(Entity, &Body)>,
    sim_time: Res<SimulationTime>,
    cache: Res<TransferCache>,
    current_body: Option<Res<crate::CurrentBody>>,
    mut active_transfer: ResMut<ActiveTransfer>,
) {
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    // Get current body for source entity
    let Some(ref current) = current_body else {
        warn!("CurrentBody not set");
        return;
    };

    let Some(ref current_name) = current.name else {
        warn!("CurrentBody has no name (ship in transit?)");
        return;
    };

    // Find best solution to Mars (default target)
    let best = find_best_transfer_today(&cache, current_day, Some("Mars"));
    let Some((key, solution)) = best else {
        warn!("No valid transfer found in cache for Mars");
        return;
    };

    let (target_name, dep_day, tof_days) = key;
    info!(
        "Best transfer to {}: dep_day={} tof={}d dv={:.0} m/s (arr_day={})",
        target_name, dep_day, tof_days, solution.total_dv, dep_day + tof_days
    );

    // Find source and target entities
    let Some((source_entity, _)) = bodies.iter().find(|(_, b)| b.name == *current_name) else {
        return;
    };
    let Some((target_entity, _)) = bodies.iter().find(|(_, b)| b.name == target_name) else {
        return;
    };

    // Spawn the visualization
    let transfer_entity = spawn_transfer_visualization(
        &mut commands,
        &mut gizmo_assets,
        source_entity,
        target_entity,
        solution,
        dep_day as f64 * 86400.0,
    );

    active_transfer.entity = Some(transfer_entity);
}

/// Finds the best (lowest delta-v) transfer departing on the current day.
/// Falls back to the nearest future day if no solutions exist for today.
/// If target_name is provided, only considers transfers to that target.
fn find_best_transfer_today<'a>(
    cache: &'a TransferCache,
    current_day: i32,
    target_name: Option<&str>,
) -> Option<(CacheKey, &'a TransferSolution)> {
    // Filter by target if specified
    let filter_target = |key: &&(String, i32, i32)| {
        target_name.map_or(true, |t| key.0 == t)
    };

    // First, try to find a solution departing today
    let today_best = cache
        .solutions
        .iter()
        .filter(|(key, _)| filter_target(&key) && key.1 == current_day)
        .min_by(|(_, a), (_, b)| a.total_dv.partial_cmp(&b.total_dv).unwrap());

    if let Some((k, v)) = today_best {
        return Some((k.clone(), v));
    }

    // Fall back to the earliest future day that has solutions
    let min_future_day = cache
        .solutions
        .keys()
        .filter(|key| filter_target(&key) && key.1 > current_day)
        .map(|key| key.1)
        .min()?;

    cache
        .solutions
        .iter()
        .filter(|(key, _)| filter_target(&key) && key.1 == min_future_day)
        .min_by(|(_, a), (_, b)| a.total_dv.partial_cmp(&b.total_dv).unwrap())
        .map(|(k, v)| (k.clone(), v))
}

/// Finds the best (lowest delta-v) transfer in a day range for a specific target.
/// Returns (departure_day, solution).
pub fn find_best_transfer_in_range<'a>(
    cache: &'a TransferCache,
    target_name: &str,
    start_day: i32,
    end_day: i32,
) -> Option<(i32, &'a TransferSolution)> {
    cache
        .solutions
        .iter()
        .filter(|((name, dep_day, _), _)| {
            name == target_name && *dep_day >= start_day && *dep_day < end_day
        })
        .min_by(|(_, a), (_, b)| a.total_dv.partial_cmp(&b.total_dv).unwrap())
        .map(|((_, dep_day, _), sol)| (*dep_day, sol))
}

/// Finds the best (lowest delta-v) transfer in the entire future window.
/// Useful for showing the optimal launch window.
#[allow(dead_code)]
fn find_best_transfer_in_window<'a>(
    cache: &'a TransferCache,
    current_day: i32,
    target_name: Option<&str>,
) -> Option<(CacheKey, &'a TransferSolution)> {
    let filter_target = |key: &&(String, i32, i32)| {
        target_name.map_or(true, |t| key.0 == t)
    };

    cache
        .solutions
        .iter()
        .filter(|(key, _)| filter_target(&key) && key.1 >= current_day)
        .min_by(|(_, a), (_, b)| a.total_dv.partial_cmp(&b.total_dv).unwrap())
        .map(|(k, v)| (k.clone(), v))
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
/// - Checks if source body changed (triggers full rebuild)
/// - Prunes old solutions (departure day < current day)
/// - Adds new solutions at the far end of the window
pub fn update_transfer_cache(
    bodies: Query<&Body>,
    sim_time: Res<SimulationTime>,
    current_body: Option<Res<crate::CurrentBody>>,
    mut cache: ResMut<TransferCache>,
) {
    let Some(ref current) = current_body else {
        return;
    };

    let Some(ref current_name) = current.name else {
        // Ship in transit - don't update cache
        return;
    };

    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    // Check if source body changed -> need full rebuild
    if cache.source_body.as_ref() != Some(current_name) {
        info!("Source body changed to {}, rebuilding cache...", current_name);

        cache.solutions.clear();
        cache.source_body = Some(current_name.clone());
        cache.last_update_day = current_day;
        cache.window_days = SEARCH_WINDOW_DAYS;

        // Find source body
        let Some(source_body) = bodies.iter().find(|b| b.name == *current_name) else {
            warn!("Source body '{}' not found", current_name);
            return;
        };

        // Find all siblings (bodies with the same parent)
        let siblings: Vec<&Body> = bodies
            .iter()
            .filter(|b| b.parent_name == source_body.parent_name && b.name != source_body.name)
            .collect();

        if siblings.is_empty() {
            warn!("No sibling bodies found for '{}'", source_body.name);
            return;
        }

        // Compute all solutions in the window for each sibling
        let mut computed = 0;
        for target_body in &siblings {
            for dep_offset in 0..=SEARCH_WINDOW_DAYS {
                let departure_day = current_day + dep_offset;
                for &tof_days in &TOF_CANDIDATES {
                    if let Some(solution) = compute_cached_transfer(source_body, target_body, departure_day, tof_days) {
                        cache.solutions.insert((target_body.name.clone(), departure_day, tof_days), solution);
                        computed += 1;
                    }
                }
            }
        }

        info!(
            "Transfer cache rebuilt: {} solutions for {} targets from {}",
            computed, siblings.len(), current_name
        );
        return;
    }

    // Only update if we've moved to a new day
    if current_day <= cache.last_update_day {
        return;
    }

    let Some(source_body) = bodies.iter().find(|b| b.name == *current_name) else {
        return;
    };

    // Find all siblings (same parent as source)
    let siblings: Vec<&Body> = bodies
        .iter()
        .filter(|b| b.parent_name == source_body.parent_name && b.name != source_body.name)
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
        for target_body in &siblings {
            for &tof_days in &TOF_CANDIDATES {
                if let Some(solution) = compute_cached_transfer(source_body, target_body, new_departure_day, tof_days) {
                    cache.solutions.insert((target_body.name.clone(), new_departure_day, tof_days), solution);
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

/// Checks if the active transfer has been completed (arrival time passed) and despawns it.
/// The arc stays visible during the entire transfer so the player can see the path.
pub fn check_transfer_expiration(
    mut commands: Commands,
    mut active_transfer: ResMut<ActiveTransfer>,
    transfers: Query<&Transfer>,
    sim_time: Res<SimulationTime>,
    old_transfers: Query<Entity, With<Transfer>>,
    old_arcs: Query<Entity, With<TransferArc>>,
    old_markers: Query<Entity, With<BurnMarker>>,
) {
    let Some(transfer_entity) = active_transfer.entity else {
        return;
    };

    let Ok(transfer) = transfers.get(transfer_entity) else {
        // Transfer entity no longer exists, clear the reference
        active_transfer.entity = None;
        return;
    };

    // Check if arrival time has passed (departure + TOF)
    let arrival_time = transfer.departure_time + transfer.solution.time_of_flight;
    if sim_time.sim_time > arrival_time {
        info!(
            "Transfer completed: arrival was at day {}, current day is {}",
            (arrival_time / 86400.0).floor() as i32,
            (sim_time.sim_time / 86400.0).floor() as i32
        );

        // Despawn all transfer-related entities
        for entity in old_transfers.iter() {
            commands.entity(entity).despawn();
        }
        for entity in old_arcs.iter() {
            commands.entity(entity).despawn();
        }
        for entity in old_markers.iter() {
            commands.entity(entity).despawn();
        }

        active_transfer.entity = None;
        active_transfer.user_selected = false; // Allow auto-update again
    }
}

/// Updates the transfer visualization based on current state:
/// - User-selected transfer: always shown (full color)
/// - Preview transfer: shown when hovering (dimmer color), can coexist with selected
pub fn update_transfer_visualization(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    bodies: Query<(Entity, &Body)>,
    sim_time: Res<SimulationTime>,
    current_body: Option<Res<crate::CurrentBody>>,
    popup: Res<crate::TransferPopup>,
    mut active_transfer: ResMut<ActiveTransfer>,
    preview_arcs: Query<Entity, With<PreviewTransferArc>>,
    // For cleaning up non-user-selected transfers
    non_preview_arcs: Query<Entity, (With<TransferArc>, Without<PreviewTransferArc>)>,
    transfers: Query<Entity, With<Transfer>>,
    markers: Query<Entity, With<BurnMarker>>,
) {
    let Some(ref current) = current_body else {
        return;
    };

    let Some(ref current_name) = current.name else {
        // Ship in transit - don't update visualization
        return;
    };

    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    // Check if we should show a preview (popup open AND hovering)
    let preview_data: Option<(String, &TransferSolution, i32)> =
        if let (Some(popup_target), Some(hover_idx)) = (popup.target_entity, popup.hovered_option) {
            if let Some(option) = popup.options.get(hover_idx) {
                let target_name = bodies
                    .get(popup_target)
                    .map(|(_, b)| b.name.clone())
                    .unwrap_or_default();
                let abs_dep_day = current_day + option.departure_day;
                Some((target_name, &option.solution, abs_dep_day))
            } else {
                None
            }
        } else {
            None
        };

    // Always despawn old preview arcs first
    for entity in preview_arcs.iter() {
        commands.entity(entity).despawn();
    }
    active_transfer.preview_entity = None;

    // If we have preview data, spawn a preview arc
    if let Some((target_name, solution, dep_day)) = preview_data {
        // Find source and target entities
        if let Some((source_entity, _)) = bodies.iter().find(|(_, b)| b.name == *current_name) {
            if let Some((target_entity, _)) = bodies.iter().find(|(_, b)| b.name == target_name) {
                // Check if this is different from the selected transfer (if any)
                let should_show_preview = if active_transfer.user_selected {
                    // Always show preview when user has a selection (it's a different target or option)
                    true
                } else {
                    // No user selection - this becomes the primary visualization
                    // Despawn any existing non-preview transfer and related entities
                    for entity in transfers.iter() {
                        commands.entity(entity).despawn();
                    }
                    for entity in non_preview_arcs.iter() {
                        commands.entity(entity).despawn();
                    }
                    for entity in markers.iter() {
                        commands.entity(entity).despawn();
                    }
                    active_transfer.entity = None;
                    false
                };

                if should_show_preview {
                    // Spawn preview arc (dimmer color, no burn markers)
                    let preview_entity = spawn_preview_arc(
                        &mut commands,
                        &mut gizmo_assets,
                        solution,
                    );
                    active_transfer.preview_entity = Some(preview_entity);
                } else {
                    // Spawn as primary transfer (full color with burn markers)
                    let transfer_entity = spawn_transfer_visualization(
                        &mut commands,
                        &mut gizmo_assets,
                        source_entity,
                        target_entity,
                        solution,
                        dep_day as f64 * 86400.0,
                    );
                    active_transfer.entity = Some(transfer_entity);
                }
            }
        }
    } else if !active_transfer.user_selected {
        // No preview and no user selection - despawn everything
        for entity in transfers.iter() {
            commands.entity(entity).despawn();
        }
        for entity in non_preview_arcs.iter() {
            commands.entity(entity).despawn();
        }
        for entity in markers.iter() {
            commands.entity(entity).despawn();
        }
        active_transfer.entity = None;
    }
}

/// Spawns just the arc for preview (no Transfer component, no burn markers)
fn spawn_preview_arc(
    commands: &mut Commands,
    gizmo_assets: &mut ResMut<Assets<GizmoAsset>>,
    solution: &TransferSolution,
) -> Entity {
    let mut gizmo = GizmoAsset::new();
    let tof = solution.time_of_flight;
    let step_dt = tof / TRANSFER_ARC_SEGMENTS as f64;

    let mut points = Vec::with_capacity(TRANSFER_ARC_SEGMENTS + 1);
    points.push(phys_to_visual(solution.departure_pos));

    let r0 = solution.departure_pos;
    let v0 = solution.departure_vel;

    for i in 1..TRANSFER_ARC_SEGMENTS {
        let t = i as f64 * step_dt;
        if let Some(r_vec) = propagate_kepler(r0, v0, MU_SUN, t) {
            points.push(phys_to_visual(r_vec));
        }
    }
    points.push(phys_to_visual(solution.arrival_pos));
    gizmo.linestrip(points, PREVIEW_TRANSFER_COLOR);

    commands.spawn((
        Gizmo {
            handle: gizmo_assets.add(gizmo),
            depth_bias: 0.04, // Slightly behind main arc
            ..default()
        },
        PreviewTransferArc,
    )).id()
}
