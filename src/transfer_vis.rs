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
/// This is the single source of truth for scheduled/active transfers.
#[derive(Component)]
pub struct Transfer {
    /// The ship performing this transfer
    pub ship: Entity,
    /// Departure body entity
    pub source: Entity,
    /// Arrival body entity
    pub target: Entity,
    /// Computed transfer solution (delta-v, orbit, etc.)
    pub solution: TransferSolution,
    /// Simulation time at departure
    pub departure_time: f64,
}

/// Marker for the transfer arc gizmo entity.
#[derive(Component)]
pub struct TransferArc;

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

    // Find all siblings (bodies with the same parent)
    let siblings: Vec<&Body> = bodies
        .iter()
        .map(|(_, b)| b)
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

/// Spawns the transfer visualization entities and returns the transfer entity.
pub fn spawn_transfer_visualization(
    commands: &mut Commands,
    gizmo_assets: &mut ResMut<Assets<GizmoAsset>>,
    ship_entity: Entity,
    source_entity: Entity,
    target_entity: Entity,
    solution: &TransferSolution,
    departure_time: f64,
) -> Entity {
    // Create the transfer arc gizmo
    let arc_asset = create_transfer_arc(solution, MU_SUN);

    // Spawn the Transfer entity
    let transfer_entity = commands
        .spawn(Transfer {
            ship: ship_entity,
            source: source_entity,
            target: target_entity,
            solution: solution.clone(),
            departure_time,
        })
        .id();

    // Spawn the arc gizmo as a child of the transfer
    let arc_entity = commands.spawn((
        Gizmo {
            handle: gizmo_assets.add(arc_asset),
            depth_bias: 0.05,
            ..default()
        },
        TransferArc,
    )).id();

    // Spawn burn markers as children of the transfer
    let departure_marker = commands.spawn(BurnMarker {
        transfer: transfer_entity,
        is_departure: true,
        delta_v: solution.departure_dv,
    }).id();

    let arrival_marker = commands.spawn(BurnMarker {
        transfer: transfer_entity,
        is_departure: false,
        delta_v: solution.arrival_dv,
    }).id();

    // Set up parent-child relationships
    commands.entity(transfer_entity).add_children(&[arc_entity, departure_marker, arrival_marker]);

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

    let current_name = &source_body.name;
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    // Check if source body changed -> need full rebuild
    if cache.source_body.as_ref() != Some(current_name) {
        info!("Source body changed to {}, rebuilding cache...", current_name);

        cache.solutions.clear();
        cache.source_body = Some(current_name.clone());
        cache.last_update_day = current_day;
        cache.window_days = SEARCH_WINDOW_DAYS;

        // Find all siblings (bodies with the same parent)
        let siblings: Vec<&Body> = bodies
            .iter()
            .map(|(_, b)| b)
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

    // Find all siblings (same parent as source)
    let siblings: Vec<&Body> = bodies
        .iter()
        .map(|(_, b)| b)
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

/// Checks if any transfers have been completed (arrival time passed) and despawns them.
/// The arc stays visible during the entire transfer so the player can see the path.
pub fn check_transfer_expiration(
    mut commands: Commands,
    transfers: Query<(Entity, &Transfer)>,
    sim_time: Res<SimulationTime>,
) {
    for (transfer_entity, transfer) in transfers.iter() {
        // Check if arrival time has passed (departure + TOF)
        let arrival_time = transfer.departure_time + transfer.solution.time_of_flight;
        if sim_time.sim_time > arrival_time {
            info!(
                "Transfer completed: arrival was at day {}, current day is {}",
                (arrival_time / 86400.0).floor() as i32,
                (sim_time.sim_time / 86400.0).floor() as i32
            );

            // Despawn the transfer entity and all children (arc and burn markers)
            commands.entity(transfer_entity).despawn();
        }
    }
}

/// Updates preview arcs based on popup hover state.
/// Committed transfers are managed separately (spawned on selection, despawned on expiration).
pub fn update_preview_arc(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    popup: Res<crate::ui::TransferPopup>,
    preview_arcs: Query<Entity, With<PreviewTransferArc>>,
) {
    // Always despawn old preview arcs first
    for entity in preview_arcs.iter() {
        commands.entity(entity).despawn();
    }

    // Check if we should show a preview (popup open AND hovering)
    if let (Some(_popup_target), Some(hover_idx)) = (popup.target_entity, popup.hovered_option) {
        if let Some(option) = popup.options.get(hover_idx) {
            // Spawn preview arc
            spawn_preview_arc(&mut commands, &mut gizmo_assets, &option.solution);
        }
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
