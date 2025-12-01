//! Transfer trajectory visualization.
//!
//! Renders Lambert transfer arcs between celestial bodies with burn markers
//! showing departure and arrival delta-v requirements.

use bevy::{
    asset::Assets,
    gizmos::GizmoAsset,
    prelude::*,
};
use bevy_vector_shapes::prelude::*;
use astrora_core::core::Vector3;

use crate::{
    orbital_data::MU_SUN,
    transfer::{TransferSolution, propagate_kepler},
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
    /// The fleet performing this transfer
    pub fleet: Entity,
    /// Departure body entity
    #[allow(dead_code)]
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

// ============================================================================
// Visualization
// ============================================================================

/// Spawns the transfer visualization entities and returns the transfer entity.
pub fn spawn_transfer_visualization(
    commands: &mut Commands,
    gizmo_assets: &mut ResMut<Assets<GizmoAsset>>,
    fleet_entity: Entity,
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
            fleet: fleet_entity,
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

/// Generates arc points by propagating the transfer orbit using Kepler's equations.
/// Returns a vector of visual coordinates for drawing the transfer arc.
fn generate_arc_points(solution: &TransferSolution, mu: f64, segments: usize) -> Vec<Vec3> {
    let tof = solution.time_of_flight;
    let step_dt = tof / segments as f64;
    let mut points = Vec::with_capacity(segments + 1);

    // First point is the known departure position
    points.push(phys_to_visual(solution.departure_pos));

    // Propagate intermediate points
    for i in 1..segments {
        let t = i as f64 * step_dt;
        if let Some(r_vec) = propagate_kepler(solution.departure_pos, solution.departure_vel, mu, t) {
            points.push(phys_to_visual(r_vec));
        }
    }

    // Last point is the known arrival position
    points.push(phys_to_visual(solution.arrival_pos));

    points
}

/// Creates a GizmoAsset containing the transfer arc linestrip.
/// Uses universal variable propagation for accurate trajectory.
fn create_transfer_arc(solution: &TransferSolution, mu: f64) -> GizmoAsset {
    let mut gizmo = GizmoAsset::new();

    // Generate arc points using shared helper
    let points = generate_arc_points(solution, mu, TRANSFER_ARC_SEGMENTS);

    // Verify propagation matches expected endpoints (warn only if mismatch)
    let r0 = solution.departure_pos;
    let v0 = solution.departure_vel;
    let tof = solution.time_of_flight;
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

    // Generate arc points using shared helper
    let points = generate_arc_points(solution, MU_SUN, TRANSFER_ARC_SEGMENTS);
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
