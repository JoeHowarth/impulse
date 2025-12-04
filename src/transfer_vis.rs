//! Transfer trajectory visualization.
//!
//! Renders Lambert transfer arcs between celestial bodies with burn markers
//! showing departure and arrival delta-v requirements.

use astrora_core::core::Vector3;
use bevy::{asset::Assets, gizmos::GizmoAsset, prelude::*};
use bevy_vector_shapes::prelude::*;

use crate::{
    orbital_data::{Body, MU_SUN},
    phys_vec_to_vec3,
    simulation::SimulationTime,
    transfer::{TransferSolution, propagate_kepler},
};

// ============================================================================
// Constants
// ============================================================================

/// Number of line segments for transfer arc visualization
const TRANSFER_ARC_SEGMENTS: usize = 500;

impl TransferArcType {
    const fn color(self) -> Color {
        match self {
            TransferArcType::Committed => Color::srgba(1.0, 0.6, 0.2, 0.8),
            TransferArcType::Preview => Color::srgba(1.0, 0.6, 0.2, 0.25),
        }
    }
}

/// Departure burn arrow color (green)
const DEPARTURE_COLOR: Color = Color::srgb(0.3, 0.9, 0.3);

/// Arrival burn arrow color (red)
const ARRIVAL_COLOR: Color = Color::srgb(0.9, 0.3, 0.3);

// ============================================================================
// Components
// ============================================================================

/// A computed transfer trajectory between two bodies.
/// This is the single source of truth for scheduled/active transfers.
#[derive(Component, Clone)]
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
    // Type of transfer arc (committed or preview)
    pub arc_type: TransferArcType,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TransferArcType {
    Committed,
    Preview,
}

#[derive(Component)]
pub struct HoveredTransferArc;

impl HoveredTransferArc {
    const fn color() -> Color {
        Color::srgba(1.0, 0.6, 0.2, 0.1)
    }
}

// ============================================================================
// Visualization
// ============================================================================

/// Spawns the transfer visualization entities and returns the transfer entity.
/// Transfer is positioned in big_space hierarchy at the departure position.
pub fn spawn_transfer_visualization(
    commands: &mut Commands,
    gizmo_assets: &mut ResMut<Assets<GizmoAsset>>,
    parent_entity: Entity,
    fleet_entity: Entity,
    source_entity: Entity,
    target_entity: Entity,
    solution: &TransferSolution,
    departure_time: f64,
    cam_scale: f32,
    arc_type: TransferArcType,
) -> Entity {
    // Create the Transfer entity
    let transfer = Transfer {
        fleet: fleet_entity,
        source: source_entity,
        target: target_entity,
        solution: solution.clone(),
        departure_time,
        arc_type,
    };

    // Helper function to create a Gizmo from an asset
    let mut gizmo = |asset| -> Gizmo {
        Gizmo {
            handle: gizmo_assets.add(asset),
            depth_bias: 0.05,
            line_config: GizmoLineConfig {
                // In pixels
                width: 2.,
                ..default()
            },
            ..default()
        }
    };

    let arc_asset = create_transfer_arc(solution, MU_SUN, arc_type.color());
    // Create the transfer arc gizmo asset
    let core_bundle = (
        // Position at ZERO relative to parent
        Transform::default(),
        // Parent entity is part of big space hierarchy
        ChildOf(parent_entity),
        gizmo(arc_asset),
    );

    match arc_type {
        TransferArcType::Committed => {
            let (dep_arrow, departure_circle_bundle) =
                create_burn_arrow(&transfer, true, solution.departure_dv, cam_scale);
            let (arr_arrow, arrival_circle_bundle) =
                create_burn_arrow(&transfer, false, solution.arrival_dv, cam_scale);

            commands
                .spawn((core_bundle, transfer))
                .with_children(|builder| {
                    builder.spawn(departure_circle_bundle);
                    builder.spawn(gizmo(dep_arrow));
                    builder.spawn(arrival_circle_bundle);
                    builder.spawn(gizmo(arr_arrow));
                })
                .id()
        }
        TransferArcType::Preview => commands.spawn((core_bundle, transfer)).id(),
    }
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
    points.push(phys_vec_to_vec3(solution.departure_pos));

    // Propagate intermediate points
    for i in 1..segments {
        let t = i as f64 * step_dt;
        if let Some(r_vec) = propagate_kepler(solution.departure_pos, solution.departure_vel, mu, t)
        {
            points.push(phys_vec_to_vec3(r_vec));
        }
    }

    // Last point is the known arrival position
    points.push(phys_vec_to_vec3(solution.arrival_pos));

    points
}

/// Creates a GizmoAsset containing the transfer arc linestrip.
/// Uses universal variable propagation for accurate trajectory.
fn create_transfer_arc(solution: &TransferSolution, mu: f64, color: Color) -> GizmoAsset {
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
                error,
                error_pct,
                transfer_angle.to_degrees()
            );
        }
    }

    gizmo.linestrip(points, color);
    gizmo
}

pub fn create_burn_arrow(
    transfer: &Transfer,
    is_departure: bool,
    delta_v: Vector3,
    cam_scale: f32,
) -> (GizmoAsset, ShapeBundle<DiscComponent>) {
    let mut gizmo = GizmoAsset::new();
    let Transfer { solution, .. } = transfer;

    // Calculate position for this marker (heliocentric + sun offset for floating origin)
    let relative_pos = if is_departure {
        // Departure: at the computed departure position
        phys_vec_to_vec3(solution.departure_pos)
    } else {
        // Arrival: at the computed arrival position (where Mars will be)
        phys_vec_to_vec3(solution.arrival_pos)
    };
    // no need to add sun position because we're using local coordinates since sun is parent
    let position = relative_pos;

    // Determine color based on burn type
    let color = if is_departure {
        DEPARTURE_COLOR
    } else {
        ARRIVAL_COLOR
    };

    // Scale arrow length by delta-v magnitude (in pixels, then scaled)
    let dv_mag = delta_v.norm();
    let arrow_len_pixels = (dv_mag / 1000.0).clamp(5.0, 30.0) as f32;
    let arrow_len = cam_scale * arrow_len_pixels;

    // Direction of delta-v in visual space
    let dv_dir =
        Vec3::new(delta_v.x as f32, delta_v.y as f32, delta_v.z as f32).normalize_or_zero();

    // painter.thickness = cam_scale * 0.5;

    // Draw the arrow
    gizmo.line(position, position + dv_dir * arrow_len, color);

    // Draw arrowhead (small lines at angle)
    let arrow_end = position + dv_dir * arrow_len;
    let perp = if dv_dir.x.abs() < 0.9 {
        dv_dir.cross(Vec3::X).normalize()
    } else {
        dv_dir.cross(Vec3::Y).normalize()
    };
    let head_size = arrow_len * 0.2;

    // Each barb is a diagonal direction from the tip, equal length
    let barb_dir1 = (-dv_dir + perp * 0.5).normalize();
    let barb_dir2 = (-dv_dir - perp * 0.5).normalize();

    gizmo.line(arrow_end, arrow_end + barb_dir1 * head_size, color);
    gizmo.line(arrow_end, arrow_end + barb_dir2 * head_size, color);

    let circle_bundle = bevy_vector_shapes::prelude::ShapeBundle::circle(
        &ShapeConfig::default_3d(),
        cam_scale * 3.0,
    );

    (gizmo, circle_bundle)
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

// ============================================================================
// Update Systems
// ============================================================================

/// Updates preview arcs based on popup hover state.
/// Committed transfers are managed separately (spawned on selection, despawned on expiration).
pub fn update_hovered_arc(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    popup: Res<crate::ui::TransferPopup>,
    preview_arcs: Query<Entity, With<HoveredTransferArc>>,
    bodies: Query<&Body>,
) {
    // Always despawn old preview arcs first
    for entity in &preview_arcs {
        commands.entity(entity).despawn();
    }

    // Check if we should show a hovered arc (popup open AND hovering)
    let (Some(popup_target), Some(hover_idx)) = (popup.target_entity, popup.hovered_option) else {
        return;
    };

    // Find the option that is being hovered and create the gizmo asset
    let option = popup.options.get(hover_idx).expect("Invalid hover index");
    let gizmo_asset = create_transfer_arc(&option.solution, MU_SUN, HoveredTransferArc::color());

    // Spawn hovered arc
    commands.spawn((
        HoveredTransferArc,
        Transform::default(),
        // Parent entity is part of big space hierarchy
        ChildOf(bodies.get(popup_target).unwrap().parent_entity.unwrap()),
        Gizmo {
            handle: gizmo_assets.add(gizmo_asset),
            depth_bias: 0.05,
            line_config: GizmoLineConfig {
                // In pixels
                width: 2.,
                ..default()
            },
            ..default()
        },
    ));
}
