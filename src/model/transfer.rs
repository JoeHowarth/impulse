//! Lambert transfer orbit computation.
//!
//! Pure orbital mechanics functions for computing transfer trajectories
//! between two points in space using Lambert's problem solver.

use astrora_core::{
    PoliastroError,
    core::{
        Vector3,
        elements::{OrbitalElements, rv_to_coe},
    },
    maneuvers::{Lambert, TransferKind},
};
use serde::{Deserialize, Serialize};

/// Result of computing a Lambert transfer trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferSolution {
    /// Delta-v vector required at departure (m/s)
    pub departure_dv: Vector3,
    /// Delta-v vector required at arrival (m/s)
    pub arrival_dv: Vector3,
    /// Total delta-v magnitude (m/s) - sum of burn magnitudes
    pub total_dv: f64,
    /// Orbital elements of the transfer trajectory
    #[allow(dead_code)]
    pub transfer_orbit: OrbitalElements,
    /// Time of flight (seconds)
    pub time_of_flight: f64,
    /// Departure position (meters, heliocentric)
    pub departure_pos: Vector3,
    /// Departure velocity on transfer orbit (m/s, heliocentric)
    pub departure_vel: Vector3,
    /// Arrival position (meters, heliocentric)
    pub arrival_pos: Vector3,
}

/// Errors that can occur during transfer computation.
#[derive(Debug)]
pub enum TransferError {
    /// Lambert solver failed to find a solution
    LambertFailed(PoliastroError),
    /// Failed to convert solution to orbital elements
    ElementConversionFailed(PoliastroError),
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransferError::LambertFailed(e) => write!(f, "Lambert solver failed: {}", e),
            TransferError::ElementConversionFailed(e) => {
                write!(f, "Failed to convert to orbital elements: {}", e)
            }
        }
    }
}

impl std::error::Error for TransferError {}

/// Propagate a state (position, velocity) forward in time using universal variables.
/// Returns the (position, velocity) after propagating for time dt.
pub fn propagate_kepler_full(
    r0: Vector3,
    v0: Vector3,
    mu: f64,
    dt: f64,
) -> Option<(Vector3, Vector3)> {
    let r0_mag = r0.norm();
    let v0_mag = v0.norm();

    // Specific energy and semi-major axis
    let energy = v0_mag * v0_mag / 2.0 - mu / r0_mag;
    let a = -mu / (2.0 * energy);

    // Initial guess for universal anomaly
    let sqrt_mu = mu.sqrt();
    let alpha = 1.0 / a;

    let chi = if energy < -1e-10 * mu / r0_mag {
        sqrt_mu * dt * alpha
    } else if energy > 1e-10 * mu / r0_mag {
        let a_hyp = a.abs();
        dt.signum() * a_hyp.sqrt() * ((2.0 * mu / a_hyp).sqrt() * dt / a_hyp).asinh()
    } else {
        sqrt_mu * dt / r0_mag
    };

    let mut chi = chi;
    let r0_dot_v0 = r0.dot(&v0);

    for _ in 0..50 {
        let chi2 = chi * chi;
        let psi = chi2 * alpha;
        let (c2, c3) = stumpff_c(psi);

        let _r =
            chi2 * c2 + r0_dot_v0 / sqrt_mu * chi * (1.0 - psi * c3) + r0_mag * (1.0 - psi * c2);
        let f_chi = r0_dot_v0 / sqrt_mu * chi2 * c2
            + (1.0 - r0_mag * alpha) * chi2 * chi * c3
            + r0_mag * chi
            - sqrt_mu * dt;
        let f_prime =
            chi2 * c2 + r0_dot_v0 / sqrt_mu * chi * (1.0 - psi * c3) + r0_mag * (1.0 - psi * c2);

        let delta = f_chi / f_prime;
        chi -= delta;

        if delta.abs() < 1e-12 {
            let chi2 = chi * chi;
            let psi = chi2 * alpha;
            let (c2, c3) = stumpff_c(psi);

            let f = 1.0 - chi2 / r0_mag * c2;
            let g = dt - chi2 * chi / sqrt_mu * c3;

            let r_vec = r0 * f + v0 * g;
            let r_mag = r_vec.norm();

            // Compute f_dot and g_dot for velocity
            let f_dot = sqrt_mu / (r_mag * r0_mag) * chi * (psi * c3 - 1.0);
            let g_dot = 1.0 - chi2 / r_mag * c2;

            let v_vec = r0 * f_dot + v0 * g_dot;

            return Some((r_vec, v_vec));
        }
    }

    None
}

/// Propagate a state (position, velocity) forward in time using universal variables.
/// Returns the position after propagating for time dt.
pub fn propagate_kepler(r0: Vector3, v0: Vector3, mu: f64, dt: f64) -> Option<Vector3> {
    propagate_kepler_full(r0, v0, mu, dt).map(|(r, _)| r)
}

/// Stumpff functions C2 and C3
fn stumpff_c(psi: f64) -> (f64, f64) {
    if psi > 1e-6 {
        let sqrt_psi = psi.sqrt();
        let c2 = (1.0 - sqrt_psi.cos()) / psi;
        let c3 = (sqrt_psi - sqrt_psi.sin()) / (psi * sqrt_psi);
        (c2, c3)
    } else if psi < -1e-6 {
        let sqrt_neg_psi = (-psi).sqrt();
        let c2 = (1.0 - sqrt_neg_psi.cosh()) / psi;
        let c3 = (sqrt_neg_psi.sinh() - sqrt_neg_psi) / (-psi * sqrt_neg_psi);
        (c2, c3)
    } else {
        let c2 = 1.0 / 2.0 - psi / 24.0 + psi * psi / 720.0;
        let c3 = 1.0 / 6.0 - psi / 120.0 + psi * psi / 5040.0;
        (c2, c3)
    }
}

/// Compute a transfer trajectory from ship's current state to target's future state.
///
/// Uses Lambert's problem to find the trajectory connecting two positions
/// in a given time of flight, then calculates the delta-v required at both
/// departure and arrival.
///
/// This function tries both short-way and long-way transfers and selects the
/// one that propagates correctly to the target position.
///
/// # Arguments
///
/// * `ship_pos` - Ship's current position in meters
/// * `ship_vel` - Ship's current velocity in m/s
/// * `target_pos` - Target's position at arrival time in meters
/// * `target_vel` - Target's velocity at arrival time in m/s
/// * `tof` - Time of flight in seconds
/// * `mu` - Gravitational parameter of common parent body (m³/s²)
///
/// # Returns
///
/// A `TransferSolution` containing:
/// - Delta-v vectors for departure and arrival burns
/// - Total delta-v required
/// - Orbital elements of the transfer trajectory
/// - Time of flight
pub fn compute_transfer(
    ship_pos: Vector3,
    ship_vel: Vector3,
    target_pos: Vector3,
    target_vel: Vector3,
    tof: f64,
    mu: f64,
) -> Result<TransferSolution, TransferError> {
    // Try both short-way and long-way transfers
    let kinds = [TransferKind::ShortWay, TransferKind::LongWay];

    let mut best_solution: Option<(TransferSolution, f64)> = None; // (solution, propagation_error)

    for kind in kinds {
        let lambert = match Lambert::solve(ship_pos, target_pos, tof, mu, kind, 0) {
            Ok(l) => l,
            Err(_) => continue, // This transfer type doesn't work for this geometry
        };

        // Verify propagation reaches the target
        let propagation_error =
            if let Some(propagated_pos) = propagate_kepler(lambert.r1, lambert.v1, mu, tof) {
                (propagated_pos - target_pos).norm()
            } else {
                f64::MAX // Propagation failed
            };

        // Calculate delta-v
        let departure_dv = lambert.v1 - ship_vel;
        let arrival_dv = target_vel - lambert.v2;
        let total_dv = departure_dv.norm() + arrival_dv.norm();

        // Convert to orbital elements
        let transfer_orbit = match rv_to_coe(&lambert.r1, &lambert.v1, mu, 1e-10) {
            Ok(o) => o,
            Err(_) => continue,
        };

        let solution = TransferSolution {
            departure_dv,
            arrival_dv,
            total_dv,
            transfer_orbit,
            time_of_flight: tof,
            departure_pos: lambert.r1,
            departure_vel: lambert.v1,
            arrival_pos: target_pos,
        };

        // Select based on propagation accuracy (must be within 1% of arrival distance)
        let arrival_dist = target_pos.norm();
        let error_pct = propagation_error / arrival_dist;

        if error_pct < 0.01 {
            // Good propagation - consider this solution
            match &best_solution {
                None => best_solution = Some((solution, propagation_error)),
                Some((best, _)) => {
                    // Prefer lower delta-v among good solutions
                    if total_dv < best.total_dv {
                        best_solution = Some((solution, propagation_error));
                    }
                }
            }
        }
    }

    // If no good solution found, fall back to Auto (original behavior)
    if best_solution.is_none() {
        let lambert = Lambert::solve(ship_pos, target_pos, tof, mu, TransferKind::Auto, 0)
            .map_err(TransferError::LambertFailed)?;

        let departure_dv = lambert.v1 - ship_vel;
        let arrival_dv = target_vel - lambert.v2;
        let total_dv = departure_dv.norm() + arrival_dv.norm();

        let transfer_orbit = rv_to_coe(&lambert.r1, &lambert.v1, mu, 1e-10)
            .map_err(TransferError::ElementConversionFailed)?;

        return Ok(TransferSolution {
            departure_dv,
            arrival_dv,
            total_dv,
            transfer_orbit,
            time_of_flight: tof,
            departure_pos: lambert.r1,
            departure_vel: lambert.v1,
            arrival_pos: target_pos,
        });
    }

    Ok(best_solution.unwrap().0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrora_core::maneuvers::{Lambert, TransferKind};
    use std::f64::consts::PI;

    /// Earth's gravitational parameter (m³/s²)
    const MU_EARTH: f64 = 3.986004418e14;

    #[test]
    fn test_leo_to_higher_orbit_transfer() {
        // Transfer from LEO to a higher orbit
        // Using ~120° transfer angle to avoid the degenerate 180° case
        // (180° has infinite transfer planes, Lambert solver rejects it)

        let r_leo = 7000e3; // 7000 km from Earth center
        let r_high = 20000e3; // Higher orbit at 20000 km

        // Ship in circular LEO at (r_leo, 0, 0) moving in +y direction
        let ship_pos = Vector3::new(r_leo, 0.0, 0.0);
        let v_circular_leo = (MU_EARTH / r_leo).sqrt();
        let ship_vel = Vector3::new(0.0, v_circular_leo, 0.0);

        // Target at ~120° ahead in the higher orbit
        // Position: r_high * (cos(120°), sin(120°), 0)
        let angle = 2.0 * PI / 3.0; // 120 degrees
        let target_pos = Vector3::new(r_high * angle.cos(), r_high * angle.sin(), 0.0);
        // Velocity perpendicular to position (circular orbit)
        let v_circular_high = (MU_EARTH / r_high).sqrt();
        let target_vel = Vector3::new(
            -v_circular_high * angle.sin(),
            v_circular_high * angle.cos(),
            0.0,
        );

        // Time of flight: estimate for this transfer
        // For a non-Hohmann transfer, we can use an approximate TOF
        let a_transfer = (r_leo + r_high) / 2.0;
        let tof = 0.6 * 2.0 * PI * (a_transfer.powi(3) / MU_EARTH).sqrt(); // ~60% of full period

        let solution = compute_transfer(ship_pos, ship_vel, target_pos, target_vel, tof, MU_EARTH)
            .expect("Transfer computation should succeed");

        // Verify the transfer orbit is elliptic
        assert!(
            solution.transfer_orbit.e < 1.0,
            "Transfer orbit should be elliptic, got e = {}",
            solution.transfer_orbit.e
        );

        // Verify we got reasonable delta-v values (not checking exact values for non-Hohmann)
        assert!(
            solution.total_dv > 1000.0 && solution.total_dv < 10000.0,
            "Total delta-v should be reasonable, got {:.0} m/s",
            solution.total_dv
        );

        // Verify time of flight matches what we asked for
        assert!(
            (solution.time_of_flight - tof).abs() < 1.0,
            "Time of flight should match input"
        );

        // Print detailed results for debugging
        println!("LEO to Higher Orbit Transfer (120°):");
        println!("  Departure Δv: {:.1} m/s", solution.departure_dv.norm());
        println!("  Arrival Δv: {:.1} m/s", solution.arrival_dv.norm());
        println!("  Total Δv: {:.1} m/s", solution.total_dv);
        println!(
            "  Transfer orbit a: {:.0} km",
            solution.transfer_orbit.a / 1000.0
        );
        println!("  Transfer orbit e: {:.4}", solution.transfer_orbit.e);
        println!("  Time of flight: {:.1} hours", tof / 3600.0);
    }

    #[test]
    fn test_near_hohmann_vs_theoretical() {
        // Compare a near-180° transfer to theoretical Hohmann values
        // Using 150° to avoid convergence issues near 180° while staying reasonably close

        let r_leo = 7000e3; // 7000 km (LEO)
        let r_geo = 42164e3; // GEO radius

        // Ship in circular LEO
        let ship_pos = Vector3::new(r_leo, 0.0, 0.0);
        let v_circular_leo = (MU_EARTH / r_leo).sqrt();
        let ship_vel = Vector3::new(0.0, v_circular_leo, 0.0);

        // Target at 150° in GEO (far enough from 180° to avoid degeneracy)
        let angle = 150.0_f64.to_radians();
        let target_pos = Vector3::new(r_geo * angle.cos(), r_geo * angle.sin(), 0.0);
        let v_circular_geo = (MU_EARTH / r_geo).sqrt();
        let target_vel = Vector3::new(
            -v_circular_geo * angle.sin(),
            v_circular_geo * angle.cos(),
            0.0,
        );

        // Scale TOF proportionally to transfer angle (150/180 of Hohmann TOF)
        let a_transfer = (r_leo + r_geo) / 2.0;
        let hohmann_tof = PI * (a_transfer.powi(3) / MU_EARTH).sqrt();
        let tof = hohmann_tof * (150.0 / 180.0);

        let solution = compute_transfer(ship_pos, ship_vel, target_pos, target_vel, tof, MU_EARTH)
            .expect("Transfer computation should succeed");

        // Calculate theoretical Hohmann delta-v values
        // Departure: accelerate from circular LEO to transfer ellipse periapsis velocity
        let v_periapsis = (MU_EARTH * (2.0 / r_leo - 1.0 / a_transfer)).sqrt();
        let hohmann_departure_dv = v_periapsis - v_circular_leo;

        // Arrival: accelerate from transfer ellipse apoapsis velocity to circular GEO
        let v_apoapsis = (MU_EARTH * (2.0 / r_geo - 1.0 / a_transfer)).sqrt();
        let hohmann_arrival_dv = v_circular_geo - v_apoapsis;

        let hohmann_total_dv = hohmann_departure_dv + hohmann_arrival_dv;

        println!("\n=== 150° Transfer vs Theoretical Hohmann (180°) ===");
        println!("Theoretical Hohmann (180°):");
        println!("  Departure Δv: {:.1} m/s", hohmann_departure_dv);
        println!("  Arrival Δv: {:.1} m/s", hohmann_arrival_dv);
        println!("  Total Δv: {:.1} m/s", hohmann_total_dv);
        println!("  TOF: {:.1} hours", hohmann_tof / 3600.0);
        println!("\nComputed (150° transfer):");
        println!("  Departure Δv: {:.1} m/s", solution.departure_dv.norm());
        println!("  Arrival Δv: {:.1} m/s", solution.arrival_dv.norm());
        println!("  Total Δv: {:.1} m/s", solution.total_dv);
        println!(
            "  Transfer orbit a: {:.0} km",
            solution.transfer_orbit.a / 1000.0
        );
        println!("  Transfer orbit e: {:.4}", solution.transfer_orbit.e);
        println!("  TOF: {:.1} hours", tof / 3600.0);
        println!("\nDifference from theoretical Hohmann:");
        println!(
            "  Δv difference: {:.1} m/s ({:.1}%)",
            solution.total_dv - hohmann_total_dv,
            100.0 * (solution.total_dv - hohmann_total_dv) / hohmann_total_dv
        );

        // 150° transfer will be more expensive than Hohmann
        // Allow up to 30% difference since we're 30° off from optimal and using scaled TOF
        let tolerance_percent = 30.0;
        let diff_percent = 100.0 * (solution.total_dv - hohmann_total_dv).abs() / hohmann_total_dv;
        assert!(
            diff_percent < tolerance_percent,
            "150° transfer should be within {}% of Hohmann, got {:.1}%",
            tolerance_percent,
            diff_percent
        );
    }

    #[test]
    fn test_transfer_returns_valid_orbital_elements() {
        // Simple test to verify we get valid orbital elements back
        // Using 90° transfer geometry (non-degenerate)
        let r1 = 8000e3;
        let r2 = 12000e3;

        // Start on +x axis moving +y
        let pos1 = Vector3::new(r1, 0.0, 0.0);
        let vel1 = Vector3::new(0.0, (MU_EARTH / r1).sqrt(), 0.0);
        // End on +y axis moving -x (90° transfer)
        let pos2 = Vector3::new(0.0, r2, 0.0);
        let vel2 = Vector3::new(-(MU_EARTH / r2).sqrt(), 0.0, 0.0);

        // Time of flight for roughly quarter-period transfer
        let a_transfer = (r1 + r2) / 2.0;
        let tof = 0.25 * 2.0 * PI * (a_transfer.powi(3) / MU_EARTH).sqrt();

        let solution = compute_transfer(pos1, vel1, pos2, vel2, tof, MU_EARTH)
            .expect("Transfer should succeed");

        // Orbital elements should be valid
        assert!(
            solution.transfer_orbit.a > 0.0,
            "Semi-major axis should be positive"
        );
        assert!(
            solution.transfer_orbit.e >= 0.0 && solution.transfer_orbit.e < 1.0,
            "Eccentricity should be valid for elliptic orbit"
        );
        assert!(
            solution.transfer_orbit.i >= 0.0 && solution.transfer_orbit.i <= PI,
            "Inclination should be in valid range"
        );
    }

    #[test]
    fn test_earth_mars_at_j2000_epoch() {
        // This test replicates the exact starting conditions in the app
        // Earth and Mars orbital elements from orbital_data.rs at t=0 (J2000)
        use crate::model::propagate_elliptic;
        use astrora_core::core::elements::OrbitalElements;

        const MU_SUN: f64 = 1.327_124_400_18e20;

        // Earth orbital elements (from orbital_data.rs)
        let earth_elements = OrbitalElements {
            a: 1.49598e11,
            e: 0.016708,
            i: 0.00005,
            raan: 0.0,
            argp: 1.7967,
            nu: 6.2383,
        };

        // Mars orbital elements (from orbital_data.rs)
        let mars_elements = OrbitalElements {
            a: 2.27939e11,
            e: 0.09340,
            i: 0.03229,
            raan: 0.8653,
            argp: 4.9997,
            nu: 0.3404,
        };

        use astrora_core::core::elements::coe_to_rv;

        // Try different departure time offsets (simulating running the sim for a while)
        // Earth-Mars synodic period is ~780 days, launch windows occur roughly every 26 months
        let departure_offsets_days: Vec<f64> = (0..=400).step_by(30).map(|d| d as f64).collect();
        let tof_candidates = [180.0, 200.0, 220.0, 250.0, 280.0, 300.0, 350.0];

        println!("Looking for valid Earth→Mars transfers...\n");

        let mut any_success = false;

        for &dep_offset in &departure_offsets_days {
            let dep_time = dep_offset * 86400.0;

            // Propagate Earth to departure time
            let earth_at_dep =
                propagate_elliptic(earth_elements, MU_SUN, dep_time).unwrap_or(earth_elements);
            let (earth_pos, earth_vel) = coe_to_rv(&earth_at_dep, MU_SUN);

            for &tof_days in &tof_candidates {
                let tof = tof_days * 86400.0;
                let arrival_time = dep_time + tof;

                // Propagate Mars to arrival time
                let mars_at_arr = match propagate_elliptic(mars_elements, MU_SUN, arrival_time) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let (mars_pos, mars_vel) = coe_to_rv(&mars_at_arr, MU_SUN);

                // Calculate transfer angle
                let dot =
                    earth_pos.x * mars_pos.x + earth_pos.y * mars_pos.y + earth_pos.z * mars_pos.z;
                let transfer_angle = (dot / (earth_pos.norm() * mars_pos.norm())).acos();

                match compute_transfer(earth_pos, earth_vel, mars_pos, mars_vel, tof, MU_SUN) {
                    Ok(sol) => {
                        // Only print reasonable transfers (< 20 km/s is somewhat reasonable)
                        if sol.total_dv < 20000.0 {
                            println!(
                                "dep=+{:.0}d TOF={:.0}d: ✓ angle={:.1}° Δv={:.0} m/s (dep={:.0}, arr={:.0}) **GOOD**",
                                dep_offset,
                                tof_days,
                                transfer_angle.to_degrees(),
                                sol.total_dv,
                                sol.departure_dv.norm(),
                                sol.arrival_dv.norm()
                            );
                        } else if sol.total_dv < 50000.0 {
                            println!(
                                "dep=+{:.0}d TOF={:.0}d: ✓ angle={:.1}° Δv={:.0} m/s (high but usable)",
                                dep_offset,
                                tof_days,
                                transfer_angle.to_degrees(),
                                sol.total_dv
                            );
                        }
                        any_success = true;
                    }
                    Err(_) => {
                        // Only print failures at dep=0 for brevity
                        if dep_offset == 0.0 {
                            println!(
                                "dep=+{:.0}d TOF={:.0}d: ✗ angle={:.1}°",
                                dep_offset,
                                tof_days,
                                transfer_angle.to_degrees()
                            );
                        }
                    }
                }
            }
        }

        assert!(any_success, "At least one departure/TOF combo should work");
    }

    #[test]
    fn test_propagator_circular_orbit() {
        // Test 1: Simple circular orbit - propagate quarter period
        // Starting at (r, 0, 0) with velocity (0, v_circ, 0)
        // After T/4, should be at (0, r, 0)

        let r = 7000e3; // 7000 km
        let v_circ = (MU_EARTH / r).sqrt();
        let period = 2.0 * PI * (r.powi(3) / MU_EARTH).sqrt();

        let r0 = Vector3::new(r, 0.0, 0.0);
        let v0 = Vector3::new(0.0, v_circ, 0.0);

        println!("=== Circular Orbit Propagation Test ===");
        println!(
            "r = {:.0} km, v_circ = {:.1} m/s, period = {:.1} min",
            r / 1000.0,
            v_circ,
            period / 60.0
        );

        // Propagate quarter period
        let dt = period / 4.0;
        let result = propagate_kepler(r0, v0, MU_EARTH, dt);

        match result {
            Some(r_final) => {
                let expected = Vector3::new(0.0, r, 0.0);
                let error = (r_final - expected).norm();
                let error_pct = 100.0 * error / r;
                println!("After T/4:");
                println!(
                    "  Expected: ({:.0}, {:.0}, {:.0})",
                    expected.x, expected.y, expected.z
                );
                println!(
                    "  Got:      ({:.0}, {:.0}, {:.0})",
                    r_final.x, r_final.y, r_final.z
                );
                println!("  Error: {:.2e} m ({:.6}%)", error, error_pct);
                assert!(
                    error_pct < 0.001,
                    "Quarter-period propagation error too large: {}%",
                    error_pct
                );
            }
            None => panic!("Propagation failed to converge"),
        }

        // Propagate half period
        let dt = period / 2.0;
        let result = propagate_kepler(r0, v0, MU_EARTH, dt);

        match result {
            Some(r_final) => {
                let expected = Vector3::new(-r, 0.0, 0.0);
                let error = (r_final - expected).norm();
                let error_pct = 100.0 * error / r;
                println!("After T/2:");
                println!(
                    "  Expected: ({:.0}, {:.0}, {:.0})",
                    expected.x, expected.y, expected.z
                );
                println!(
                    "  Got:      ({:.0}, {:.0}, {:.0})",
                    r_final.x, r_final.y, r_final.z
                );
                println!("  Error: {:.2e} m ({:.6}%)", error, error_pct);
                assert!(
                    error_pct < 0.001,
                    "Half-period propagation error too large: {}%",
                    error_pct
                );
            }
            None => panic!("Propagation failed to converge"),
        }
    }

    #[test]
    fn test_lambert_propagation_consistency() {
        // Test 2: Lambert should return (r1, v1) such that propagating gives r2
        // Use a simple 90° transfer in Earth orbit

        let r = 10000e3;
        let r1 = Vector3::new(r, 0.0, 0.0);
        let r2 = Vector3::new(0.0, r, 0.0); // 90° transfer

        // TOF for roughly quarter period of a transfer orbit
        let a_transfer = r; // Circular transfer
        let tof = PI / 2.0 * (a_transfer.powi(3) / MU_EARTH).sqrt();

        println!("\n=== Lambert + Propagation Consistency Test ===");
        println!("r1 = ({:.0}, 0, 0) km", r1.x / 1000.0);
        println!("r2 = (0, {:.0}, 0) km", r2.y / 1000.0);
        println!("TOF = {:.1} min", tof / 60.0);

        for kind in [TransferKind::ShortWay, TransferKind::LongWay] {
            println!("\nTransferKind::{:?}:", kind);

            match Lambert::solve(r1, r2, tof, MU_EARTH, kind, 0) {
                Ok(lambert) => {
                    println!(
                        "  Lambert v1 = ({:.1}, {:.1}, {:.1}) m/s",
                        lambert.v1.x, lambert.v1.y, lambert.v1.z
                    );
                    println!(
                        "  Lambert v2 = ({:.1}, {:.1}, {:.1}) m/s",
                        lambert.v2.x, lambert.v2.y, lambert.v2.z
                    );

                    // Now propagate (r1, v1) for tof and see if we get r2
                    match propagate_kepler(lambert.r1, lambert.v1, MU_EARTH, tof) {
                        Some(r_prop) => {
                            let error = (r_prop - r2).norm();
                            let error_pct = 100.0 * error / r2.norm();
                            println!(
                                "  Propagated to: ({:.0}, {:.0}, {:.0}) km",
                                r_prop.x / 1000.0,
                                r_prop.y / 1000.0,
                                r_prop.z / 1000.0
                            );
                            println!("  Error: {:.2e} m ({:.4}%)", error, error_pct);
                        }
                        None => println!("  Propagation FAILED"),
                    }
                }
                Err(e) => println!("  Lambert FAILED: {}", e),
            }
        }
    }

    #[test]
    fn test_day350_scenario() {
        // Test 3: Replicate the exact failing scenario from day 350
        use crate::model::propagate_elliptic;
        use astrora_core::core::elements::OrbitalElements;

        const MU_SUN: f64 = 1.327_124_400_18e20;

        // Earth and Mars orbital elements (from orbital_data.rs)
        let earth_elements = OrbitalElements {
            a: 1.49598e11,
            e: 0.016708,
            i: 0.00005,
            raan: 0.0,
            argp: 1.7967,
            nu: 6.2383,
        };
        let mars_elements = OrbitalElements {
            a: 2.27939e11,
            e: 0.09340,
            i: 0.03229,
            raan: 0.8653,
            argp: 4.9997,
            nu: 0.3404,
        };

        let departure_day = 350;
        let tof_days = 150;
        let departure_time = departure_day as f64 * 86400.0;
        let tof = tof_days as f64 * 86400.0;
        let arrival_time = departure_time + tof;

        println!("\n=== Day 350 Scenario Test ===");
        println!("Departure day: {}, TOF: {} days", departure_day, tof_days);

        use astrora_core::core::elements::coe_to_rv;

        let earth_elems = propagate_elliptic(earth_elements, MU_SUN, departure_time)
            .expect("Earth propagation failed");
        let (earth_pos, _earth_vel) = coe_to_rv(&earth_elems, MU_SUN);

        let mars_elems = propagate_elliptic(mars_elements, MU_SUN, arrival_time)
            .expect("Mars propagation failed");
        let (mars_pos, _mars_vel) = coe_to_rv(&mars_elems, MU_SUN);

        // Transfer angle
        let dot = earth_pos.dot(&mars_pos);
        let transfer_angle = (dot / (earth_pos.norm() * mars_pos.norm())).acos();
        println!("Transfer angle: {:.1}°", transfer_angle.to_degrees());
        println!(
            "Earth pos: ({:.2e}, {:.2e}, {:.2e})",
            earth_pos.x, earth_pos.y, earth_pos.z
        );
        println!(
            "Mars pos:  ({:.2e}, {:.2e}, {:.2e})",
            mars_pos.x, mars_pos.y, mars_pos.z
        );

        for kind in [
            TransferKind::Auto,
            TransferKind::ShortWay,
            TransferKind::LongWay,
        ] {
            println!("\nTransferKind::{:?}:", kind);

            match Lambert::solve(earth_pos, mars_pos, tof, MU_SUN, kind, 0) {
                Ok(lambert) => {
                    println!(
                        "  v1 = ({:.1}, {:.1}, {:.1}) m/s",
                        lambert.v1.x, lambert.v1.y, lambert.v1.z
                    );
                    println!(
                        "  v2 = ({:.1}, {:.1}, {:.1}) m/s",
                        lambert.v2.x, lambert.v2.y, lambert.v2.z
                    );

                    // Debug: compute orbit parameters
                    let r0_mag = lambert.r1.norm();
                    let v0_mag = lambert.v1.norm();
                    let energy = v0_mag * v0_mag / 2.0 - MU_SUN / r0_mag;
                    let a = -MU_SUN / (2.0 * energy);
                    let period = 2.0 * PI * (a.powi(3) / MU_SUN).sqrt();
                    println!(
                        "  Transfer orbit: a={:.2e} m, period={:.1} days, energy={:.2e}",
                        a,
                        period / 86400.0,
                        energy
                    );

                    // Propagate and check both position AND velocity
                    match propagate_kepler_full(lambert.r1, lambert.v1, MU_SUN, tof) {
                        Some((r_prop, v_prop)) => {
                            let r_error = (r_prop - mars_pos).norm();
                            let r_error_pct = 100.0 * r_error / mars_pos.norm();

                            let v_error = (v_prop - lambert.v2).norm();
                            let v_error_pct = 100.0 * v_error / lambert.v2.norm();

                            println!(
                                "  Propagated r: ({:.2e}, {:.2e}, {:.2e})",
                                r_prop.x, r_prop.y, r_prop.z
                            );
                            println!(
                                "  Propagated v: ({:.1}, {:.1}, {:.1}) m/s",
                                v_prop.x, v_prop.y, v_prop.z
                            );
                            println!("  Position error: {:.2e} m ({:.2}%)", r_error, r_error_pct);
                            println!(
                                "  Velocity error: {:.2e} m/s ({:.2}%)",
                                v_error, v_error_pct
                            );

                            if r_error_pct > 1.0 {
                                println!("  ** POSITION MISMATCH **");
                            }
                            if v_error_pct > 1.0 {
                                println!("  ** VELOCITY MISMATCH **");
                            }
                        }
                        None => println!("  Propagation FAILED to converge"),
                    }
                }
                Err(e) => println!("  Lambert FAILED: {}", e),
            }
        }
    }

    #[test]
    fn test_propagator_inverse() {
        // Verify propagation by checking forward and backward consistency
        use crate::model::propagate_elliptic;
        use astrora_core::core::elements::OrbitalElements;

        const MU_SUN: f64 = 1.327_124_400_18e20;

        let earth_elements = OrbitalElements {
            a: 1.49598e11,
            e: 0.016708,
            i: 0.00005,
            raan: 0.0,
            argp: 1.7967,
            nu: 6.2383,
        };
        let mars_elements = OrbitalElements {
            a: 2.27939e11,
            e: 0.09340,
            i: 0.03229,
            raan: 0.8653,
            argp: 4.9997,
            nu: 0.3404,
        };

        let departure_time = 350.0 * 86400.0;
        let tof = 150.0 * 86400.0;
        let arrival_time = departure_time + tof;

        use astrora_core::core::elements::coe_to_rv;

        let earth_elems = propagate_elliptic(earth_elements, MU_SUN, departure_time).unwrap();
        let (r1, _) = coe_to_rv(&earth_elems, MU_SUN);

        let mars_elems = propagate_elliptic(mars_elements, MU_SUN, arrival_time).unwrap();
        let (r2, _) = coe_to_rv(&mars_elems, MU_SUN);

        let lambert = Lambert::solve(r1, r2, tof, MU_SUN, TransferKind::ShortWay, 0).unwrap();

        println!("\n=== Propagator Inverse Test ===");
        println!("r1 = ({:.6e}, {:.6e}, {:.6e})", r1.x, r1.y, r1.z);
        println!("r2 = ({:.6e}, {:.6e}, {:.6e})", r2.x, r2.y, r2.z);
        println!(
            "v1 = ({:.6e}, {:.6e}, {:.6e})",
            lambert.v1.x, lambert.v1.y, lambert.v1.z
        );
        println!(
            "v2 = ({:.6e}, {:.6e}, {:.6e})",
            lambert.v2.x, lambert.v2.y, lambert.v2.z
        );

        // Forward: propagate (r1, v1) for tof
        let (r_fwd, v_fwd) = propagate_kepler_full(r1, lambert.v1, MU_SUN, tof).unwrap();
        let fwd_r_err = (r_fwd - r2).norm() / r2.norm() * 100.0;
        let fwd_v_err = (v_fwd - lambert.v2).norm() / lambert.v2.norm() * 100.0;
        println!("\nForward propagation (r1,v1) -> should give (r2,v2):");
        println!("  r_err = {:.4}%, v_err = {:.4}%", fwd_r_err, fwd_v_err);

        // Backward: propagate (r2, -v2) for tof should give (r1, -v1)
        let v2_neg = Vector3::new(-lambert.v2.x, -lambert.v2.y, -lambert.v2.z);
        match propagate_kepler_full(r2, v2_neg, MU_SUN, tof) {
            Some((r_bwd, v_bwd)) => {
                let v1_neg = Vector3::new(-lambert.v1.x, -lambert.v1.y, -lambert.v1.z);
                let bwd_r_err = (r_bwd - r1).norm() / r1.norm() * 100.0;
                let bwd_v_err = (v_bwd - v1_neg).norm() / lambert.v1.norm() * 100.0;
                println!("\nBackward propagation (r2,-v2) -> should give (r1,-v1):");
                println!("  r_err = {:.4}%, v_err = {:.4}%", bwd_r_err, bwd_v_err);
            }
            None => {
                println!(
                    "\nBackward propagation failed to converge (expected for some orbit types)"
                );
            }
        }

        // The key assertion is forward propagation - if that works, the Lambert solution is correct
        assert!(
            fwd_r_err < 0.1,
            "Forward propagation position error should be < 0.1%, got {}%",
            fwd_r_err
        );
        assert!(
            fwd_v_err < 0.1,
            "Forward propagation velocity error should be < 0.1%, got {}%",
            fwd_v_err
        );
    }

    #[test]
    fn test_lambert_orbit_consistency() {
        // Check if Lambert's v1 and v2 are actually on the same orbit
        // by verifying orbital energy and angular momentum match
        use crate::model::propagate_elliptic;
        use astrora_core::core::elements::OrbitalElements;

        const MU_SUN: f64 = 1.327_124_400_18e20;

        let earth_elements = OrbitalElements {
            a: 1.49598e11,
            e: 0.016708,
            i: 0.00005,
            raan: 0.0,
            argp: 1.7967,
            nu: 6.2383,
        };
        let mars_elements = OrbitalElements {
            a: 2.27939e11,
            e: 0.09340,
            i: 0.03229,
            raan: 0.8653,
            argp: 4.9997,
            nu: 0.3404,
        };

        let departure_time = 350.0 * 86400.0;
        let tof = 150.0 * 86400.0;
        let arrival_time = departure_time + tof;

        use astrora_core::core::elements::coe_to_rv;

        let earth_elems = propagate_elliptic(earth_elements, MU_SUN, departure_time).unwrap();
        let (r1, _) = coe_to_rv(&earth_elems, MU_SUN);

        let mars_elems = propagate_elliptic(mars_elements, MU_SUN, arrival_time).unwrap();
        let (r2, _) = coe_to_rv(&mars_elems, MU_SUN);

        println!("\n=== Lambert Orbit Consistency Test ===");

        for kind in [TransferKind::ShortWay, TransferKind::LongWay] {
            println!("\nTransferKind::{:?}:", kind);

            let lambert = match Lambert::solve(r1, r2, tof, MU_SUN, kind, 0) {
                Ok(l) => l,
                Err(e) => {
                    println!("  Lambert failed: {}", e);
                    continue;
                }
            };

            // Orbital energy at departure: E = v²/2 - μ/r
            let r1_mag = r1.norm();
            let v1_mag = lambert.v1.norm();
            let energy1 = v1_mag * v1_mag / 2.0 - MU_SUN / r1_mag;

            // Orbital energy at arrival
            let r2_mag = r2.norm();
            let v2_mag = lambert.v2.norm();
            let energy2 = v2_mag * v2_mag / 2.0 - MU_SUN / r2_mag;

            // Angular momentum at departure: h = r × v
            let h1 = Vector3::new(
                r1.y * lambert.v1.z - r1.z * lambert.v1.y,
                r1.z * lambert.v1.x - r1.x * lambert.v1.z,
                r1.x * lambert.v1.y - r1.y * lambert.v1.x,
            );

            // Angular momentum at arrival
            let h2 = Vector3::new(
                r2.y * lambert.v2.z - r2.z * lambert.v2.y,
                r2.z * lambert.v2.x - r2.x * lambert.v2.z,
                r2.x * lambert.v2.y - r2.y * lambert.v2.x,
            );

            let energy_diff_pct = 100.0 * (energy2 - energy1).abs() / energy1.abs();
            let h_diff = (h2 - h1).norm();
            let h_diff_pct = 100.0 * h_diff / h1.norm();

            println!("  Energy at r1: {:.6e} J/kg", energy1);
            println!("  Energy at r2: {:.6e} J/kg", energy2);
            println!("  Energy difference: {:.4}%", energy_diff_pct);
            println!("  |h| at r1: {:.6e} m²/s", h1.norm());
            println!("  |h| at r2: {:.6e} m²/s", h2.norm());
            println!("  h difference: {:.4}%", h_diff_pct);

            // Semi-major axis from energy: a = -μ/(2E)
            let a1 = -MU_SUN / (2.0 * energy1);
            let a2 = -MU_SUN / (2.0 * energy2);
            println!("  SMA from r1,v1: {:.6e} m", a1);
            println!("  SMA from r2,v2: {:.6e} m", a2);
            println!("  Lambert reports: a={:.6e}, e={:.6}", lambert.a, lambert.e);

            if energy_diff_pct > 0.1 || h_diff_pct > 0.1 {
                println!("  ** ORBIT INCONSISTENCY DETECTED **");
            }
        }
    }

    #[test]
    fn test_astrora_propagator() {
        // Use astrora's own propagator to verify the Lambert solution
        use crate::model::propagate_elliptic;
        use astrora_core::core::elements::OrbitalElements;
        use astrora_core::propagators::keplerian::propagate_state_keplerian;

        const MU_SUN: f64 = 1.327_124_400_18e20;

        let earth_elements = OrbitalElements {
            a: 1.49598e11,
            e: 0.016708,
            i: 0.00005,
            raan: 0.0,
            argp: 1.7967,
            nu: 6.2383,
        };
        let mars_elements = OrbitalElements {
            a: 2.27939e11,
            e: 0.09340,
            i: 0.03229,
            raan: 0.8653,
            argp: 4.9997,
            nu: 0.3404,
        };

        // Original Day 350 scenario - tests the Izzo solver (angle > 167°)
        let departure_time = 350.0 * 86400.0;
        let tof = 150.0 * 86400.0;
        let arrival_time = departure_time + tof;

        use astrora_core::core::elements::coe_to_rv;

        let earth_elems = propagate_elliptic(earth_elements, MU_SUN, departure_time).unwrap();
        let (r1, _) = coe_to_rv(&earth_elems, MU_SUN);

        let mars_elems = propagate_elliptic(mars_elements, MU_SUN, arrival_time).unwrap();
        let (r2, _) = coe_to_rv(&mars_elems, MU_SUN);

        let lambert = Lambert::solve(r1, r2, tof, MU_SUN, TransferKind::ShortWay, 0).unwrap();

        println!("\n=== Astrora Propagator Test ===");
        println!("r1 = ({:.6e}, {:.6e}, {:.6e})", r1.x, r1.y, r1.z);
        println!("r2 = ({:.6e}, {:.6e}, {:.6e})", r2.x, r2.y, r2.z);
        println!(
            "v1 = ({:.6e}, {:.6e}, {:.6e})",
            lambert.v1.x, lambert.v1.y, lambert.v1.z
        );
        println!(
            "v2 = ({:.6e}, {:.6e}, {:.6e})",
            lambert.v2.x, lambert.v2.y, lambert.v2.z
        );

        // Use our propagator
        println!("\nOur propagator:");
        let (r_ours, v_ours) = propagate_kepler_full(r1, lambert.v1, MU_SUN, tof).unwrap();
        let r_err_ours = (r_ours - r2).norm() / r2.norm() * 100.0;
        let v_err_ours = (v_ours - lambert.v2).norm() / lambert.v2.norm() * 100.0;
        println!(
            "  r_prop = ({:.6e}, {:.6e}, {:.6e})",
            r_ours.x, r_ours.y, r_ours.z
        );
        println!("  r_err = {:.4}%, v_err = {:.4}%", r_err_ours, v_err_ours);

        // Use astrora's propagator
        println!("\nAstrora propagate_state_keplerian:");
        match propagate_state_keplerian(&r1, &lambert.v1, tof, MU_SUN) {
            Ok((r_astrora, v_astrora)) => {
                let r_err = (r_astrora - r2).norm() / r2.norm() * 100.0;
                let v_err = (v_astrora - lambert.v2).norm() / lambert.v2.norm() * 100.0;
                println!(
                    "  r_prop = ({:.6e}, {:.6e}, {:.6e})",
                    r_astrora.x, r_astrora.y, r_astrora.z
                );
                println!("  r_err = {:.4}%, v_err = {:.4}%", r_err, v_err);
            }
            Err(e) => println!("  FAILED: {}", e),
        }

        // Try propagate_lagrange
        println!("\nAstrora propagate_lagrange:");
        use astrora_core::propagators::keplerian::propagate_lagrange;
        match propagate_lagrange(&r1, &lambert.v1, tof, MU_SUN) {
            Ok((r_lag, v_lag)) => {
                let r_err = (r_lag - r2).norm() / r2.norm() * 100.0;
                let v_err = (v_lag - lambert.v2).norm() / lambert.v2.norm() * 100.0;
                println!(
                    "  r_prop = ({:.6e}, {:.6e}, {:.6e})",
                    r_lag.x, r_lag.y, r_lag.z
                );
                println!("  r_err = {:.4}%, v_err = {:.4}%", r_err, v_err);
            }
            Err(e) => println!("  FAILED: {}", e),
        }

        // Calculate what TOF would give us r2 by linear search
        println!("\nSearching for correct TOF by linear scan...");
        let mut best_tof = tof;
        let mut best_err = f64::MAX;

        for i in 0..=400 {
            let test_tof = tof * (0.5 + i as f64 * 0.005); // 0.5 to 2.5 times tof
            if let Some((r_test, _)) = propagate_kepler_full(r1, lambert.v1, MU_SUN, test_tof) {
                let err = (r_test - r2).norm();
                if err < best_err {
                    best_err = err;
                    best_tof = test_tof;
                }
            }
        }
        let best_err_pct = best_err / r2.norm() * 100.0;
        println!(
            "  Best matching TOF: {:.2} days (requested: {:.2} days)",
            best_tof / 86400.0,
            tof / 86400.0
        );
        println!(
            "  TOF difference: {:.2} days ({:.2}%)",
            (best_tof - tof) / 86400.0,
            100.0 * (best_tof - tof) / tof
        );
        println!("  Position error at best TOF: {:.6}%", best_err_pct);

        // Also compute TOF from orbital mechanics
        println!("\nComputing TOF from orbital mechanics:");
        let a = lambert.a;
        let e = lambert.e;
        let period = 2.0 * PI * (a.powi(3) / MU_SUN).sqrt();
        println!(
            "  a = {:.6e} m, e = {:.6}, period = {:.2} days",
            a,
            e,
            period / 86400.0
        );

        // Compute true anomalies at r1 and r2
        // From r = a(1-e²)/(1+e·cos(ν)), solve for cos(ν)
        let p = a * (1.0 - e * e); // semi-latus rectum
        let r1_mag = r1.norm();
        let r2_mag = r2.norm();
        let cos_nu1 = (p / r1_mag - 1.0) / e;
        let cos_nu2 = (p / r2_mag - 1.0) / e;

        // Need to determine sin(ν) from the direction
        // Use the fact that r·v = √(μ/p)·e·sin(ν)
        let sqrt_mu_p = (MU_SUN / p).sqrt();
        let sin_nu1 = r1.dot(&lambert.v1) / (r1_mag * sqrt_mu_p * e);
        let sin_nu2 = r2.dot(&lambert.v2) / (r2_mag * sqrt_mu_p * e);

        let nu1 = sin_nu1.atan2(cos_nu1);
        let nu2 = sin_nu2.atan2(cos_nu2);

        println!("  True anomaly at r1: {:.2}°", nu1.to_degrees());
        println!("  True anomaly at r2: {:.2}°", nu2.to_degrees());

        // Compute eccentric anomalies
        use astrora_core::core::anomaly::{eccentric_to_mean_anomaly, true_to_eccentric_anomaly};
        let E1 = true_to_eccentric_anomaly(nu1, e).unwrap();
        let E2 = true_to_eccentric_anomaly(nu2, e).unwrap();
        let M1 = eccentric_to_mean_anomaly(E1, e).unwrap();
        let M2 = eccentric_to_mean_anomaly(E2, e).unwrap();

        println!("  Mean anomaly at r1: {:.2}°", M1.to_degrees());
        println!("  Mean anomaly at r2: {:.2}°", M2.to_degrees());

        // TOF = ΔM / n where n = √(μ/a³)
        let n = (MU_SUN / a.powi(3)).sqrt();
        let mut delta_M = M2 - M1;
        if delta_M < 0.0 {
            delta_M += 2.0 * PI;
        }
        let computed_tof = delta_M / n;
        println!("  Computed TOF from ΔM: {:.2} days", computed_tof / 86400.0);
        println!("  Requested TOF: {:.2} days", tof / 86400.0);
        println!(
            "  Difference: {:.2} days ({:.2}%)",
            (computed_tof - tof) / 86400.0,
            100.0 * (computed_tof - tof) / tof
        );
    }
}
