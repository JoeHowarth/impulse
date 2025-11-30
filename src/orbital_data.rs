use astrora_core::{
    PoliastroResult,
    core::{
        anomaly::{mean_to_true_anomaly, true_to_mean_anomaly},
        elements::OrbitalElements,
    },
};
// Note: In newer Bevy versions, HashMap is usually in bevy::utils::HashMap
use bevy::{platform::collections::HashMap, prelude::*};
use std::f64::consts::PI;

// --- Physical Constants (SI Units: m, s, kg) ---
pub const AU: f64 = 1.495_978_707e11; 
pub const MU_SUN: f64 = 1.327_124_400_18e20;

/// Advance a bound (elliptic) Keplerian orbit by dt seconds.
pub fn propagate_elliptic(
    el: OrbitalElements,
    mu: f64, // CRITICAL: This must be the MU of the PARENT body
    dt: f64, 
) -> PoliastroResult<OrbitalElements> {
    // 1. true anomaly -> mean anomaly at t0
    let m0 = true_to_mean_anomaly(el.nu, el.e)?;

    // 2. mean motion n = sqrt(mu / a^3)
    let n = (mu / (el.a.powi(3))).sqrt();

    // 3. advance mean anomaly
    let m1 = (m0 + n * dt).rem_euclid(2.0 * PI);

    // 4. mean anomaly -> new true anomaly
    // Note: Assuming regular precision tolerance (1e-6) usually handled inside astrora or use defaults
    let nu1 = mean_to_true_anomaly(m1, el.e, None, None)?;

    // 5. same geometry, updated anomaly
    Ok(OrbitalElements {
        a: el.a,
        e: el.e,
        i: el.i,
        raan: el.raan,
        argp: el.argp,
        nu: nu1,
    })
}

#[derive(Component, Clone, Debug)]
pub struct Body {
    pub name: String,
    /// Name of the body this orbits around. None for the Sun.
    pub parent_name: Option<String>,
    /// Parent body entity (set during setup, None for Sun)
    pub parent_entity: Option<Entity>,
    /// The standard gravitational parameter (mu = G * M) of THIS body [m^3/s^2].
    /// Used when calculating orbits of children (moons).
    pub std_grav_param: f64,
    /// Mean radius in meters.
    pub radius: f64,
    /// Display color (approximate realistic appearance).
    pub color: Color,
    pub orbital_elements: OrbitalElements,
}

#[derive(Resource, Clone, Debug)]
pub struct PlanetaryElements {
    #[allow(dead_code)]
    pub bodies: HashMap<String, Body>,
}

impl PlanetaryElements {
    #[rustfmt::skip]
    pub fn get_planetary_elements() -> HashMap<String, Body> {
        let mut bodies = HashMap::new();

        // Helper macro: Name, Parent, color, mu, radius, a, e, i, raan, argp, nu
        macro_rules! add_body {
            ($name:expr, $parent:expr, $color:expr, $mu:expr, $rad:expr,
             $a:expr, $e:expr, $i:expr, $raan:expr, $argp:expr, $nu:expr) => {
                bodies.insert(
                    $name.to_string(),
                    Body {
                        name: $name.to_string(),
                        parent_name: $parent.map(|s: &str| s.to_string()),
                        parent_entity: None, // Set during setup
                        std_grav_param: $mu,
                        radius: $rad,
                        color: $color,
                        orbital_elements: OrbitalElements {
                            a: $a,
                            e: $e,
                            i: $i,
                            raan: $raan,
                            argp: $argp,
                            nu: $nu,
                        },
                    },
                );
            };
        }

        // --- THE SUN ---
        add_body!("Sun", None, Color::srgb(1.0, 0.95, 0.4),
            MU_SUN, 6.9634e8, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);

        // --- PLANETS (Heliocentric) ---
        // Mercury - gray, cratered
        add_body!("Mercury", Some("Sun"), Color::srgb(0.55, 0.55, 0.55),
            2.2032e13, 2.4397e6, 5.790905e10, 0.205630, 0.122258, 0.843547, 0.508309, 3.08075);
        // Venus - pale yellow/tan (clouds)
        add_body!("Venus", Some("Sun"), Color::srgb(0.9, 0.85, 0.7),
            3.24859e14, 6.0518e6, 1.08208e11, 0.006772, 0.059248, 1.3383, 0.9579, 0.8797);
        // Earth - blue
        add_body!("Earth", Some("Sun"), Color::srgb(0.2, 0.5, 0.9),
            3.986004418e14, 6.371e6, 1.49598e11, 0.016708, 0.00005, 0.0, 1.7967, 6.2383);
        // Mars - rusty red
        add_body!("Mars", Some("Sun"), Color::srgb(0.8, 0.4, 0.25),
            4.2828e13, 3.3895e6, 2.27939e11, 0.09340, 0.03229, 0.8653, 4.9997, 0.3404);
        // Jupiter - orange/tan bands
        add_body!("Jupiter", Some("Sun"), Color::srgb(0.85, 0.75, 0.55),
            1.266865e17, 6.9911e7, 7.7857e11, 0.04890, 0.02276, 1.7550, 4.7786, 0.3567);
        // Saturn - golden/yellow
        add_body!("Saturn", Some("Sun"), Color::srgb(0.9, 0.8, 0.5),
            3.793118e16, 5.8232e7, 1.43353e12, 0.05650, 0.04336, 1.9847, 5.9048, 5.4880);
        // Uranus - pale cyan
        add_body!("Uranus", Some("Sun"), Color::srgb(0.6, 0.85, 0.9),
            5.793939e15, 2.5362e7, 2.87246e12, 0.04638, 0.01343, 1.2955, 1.6969, 2.5205);
        // Neptune - deep blue
        add_body!("Neptune", Some("Sun"), Color::srgb(0.3, 0.45, 0.9),
            6.836529e15, 2.4622e7, 4.49506e12, 0.00945, 0.03087, 2.2989, 4.7477, 4.4774);

        // --- ASTEROIDS (Heliocentric) ---
        // Ceres - gray
        add_body!("Ceres", Some("Sun"), Color::srgb(0.55, 0.55, 0.55),
            6.26e10, 4.73e5, 4.137e11, 0.0760, 0.1850, 1.401, 1.284, 1.993);
        // Vesta - gray/tan
        add_body!("Vesta", Some("Sun"), Color::srgb(0.6, 0.58, 0.55),
            1.72e10, 2.62e5, 3.532e11, 0.0891, 0.1246, 1.811, 2.622, 0.698);

        // --- MOONS (Planetocentric) ---
        // Moon - light gray
        add_body!("Moon", Some("Earth"), Color::srgb(0.75, 0.75, 0.75),
            4.9048e12, 1.737e6, 3.844e8, 0.0549, 0.0898, 2.181, 3.490, 2.356);
        // Phobos - dark gray
        add_body!("Phobos", Some("Mars"), Color::srgb(0.4, 0.38, 0.35),
            7.1e5, 1.12e4, 9.376e6, 0.0151, 0.018, 0.0, 0.0, 0.0);
        // Io - yellow/orange (volcanic)
        add_body!("Io", Some("Jupiter"), Color::srgb(0.95, 0.85, 0.4),
            5.959e12, 1.821e6, 4.217e8, 0.0041, 0.0006, 0.0, 0.0, 1.469);
        // Europa - icy white/cream
        add_body!("Europa", Some("Jupiter"), Color::srgb(0.9, 0.88, 0.82),
            3.202e12, 1.560e6, 6.710e8, 0.0094, 0.0082, 0.0, 0.0, 1.815);
        // Ganymede - gray/brown
        add_body!("Ganymede", Some("Jupiter"), Color::srgb(0.6, 0.55, 0.5),
            9.887e12, 2.634e6, 1.070e9, 0.0011, 0.0031, 0.0, 0.0, 2.164);
        // Callisto - dark gray
        add_body!("Callisto", Some("Jupiter"), Color::srgb(0.45, 0.42, 0.4),
            7.179e12, 2.410e6, 1.882e9, 0.0074, 0.0034, 0.0, 0.0, 2.513);
        // Titan - orange haze
        add_body!("Titan", Some("Saturn"), Color::srgb(0.9, 0.65, 0.35),
            8.978e12, 2.574e6, 1.221e9, 0.0288, 0.006, 0.0, 0.0, 0.0);

        bodies
    }
}