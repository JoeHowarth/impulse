use astrora_core::{
    PoliastroResult,
    core::{
        anomaly::{mean_to_true_anomaly, true_to_mean_anomaly},
        elements::OrbitalElements,
    },
};
use bevy::{platform::collections::HashMap, prelude::*};

pub const MU_SUN: f64 = 1.32712440018e20;

/// Advance a bound (elliptic) Keplerian orbit by dt seconds.
pub fn propagate_elliptic(
    el: OrbitalElements,
    mu: f64, // gravitational parameter of central body [m^3/s^2]
    dt: f64, // time step [s] - can be negative
) -> PoliastroResult<OrbitalElements> {
    // 1. true anomaly -> mean anomaly at t0
    let M0 = true_to_mean_anomaly(el.nu, el.e)?;

    // 2. mean motion n = sqrt(mu / a^3)
    let n = (mu / (el.a * el.a * el.a)).sqrt();

    // 3. advance mean anomaly
    let M1 = (M0 + n * dt).rem_euclid(2.0 * std::f64::consts::PI);

    // 4. mean anomaly -> new true anomaly
    let nu1 = mean_to_true_anomaly(M1, el.e, None, None)?;

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
    pub orbital_elements: OrbitalElements,
    pub radius: f64,
}

#[derive(Resource, Clone, Debug)]
pub struct PlanetaryElements {
    pub bodies: HashMap<String, Body>,
}

impl PlanetaryElements {
    pub fn get_planetary_elements() -> HashMap<String, Body> {
        HashMap::from([
            (
                "Mercury".to_string(),
                Body {
                    name: "Mercury".to_string(),
                    orbital_elements: OrbitalElements {
                        a: 5.790934e10,
                        e: 0.20564000,
                        i: 0.122277767394723,
                        raan: 0.843692160414059,
                        argp: 0.508239878180749,
                        nu: 3.080352286349359,
                    },
                    radius: 0.0,
                },
            ),
            (
                "Venus".to_string(),
                Body {
                    name: "Venus".to_string(),
                    orbital_elements: OrbitalElements {
                        a: 1.082041e11,
                        e: 0.00676000,
                        i: 0.059306287982767,
                        raan: 1.338143937504052,
                        argp: 0.961676417848876,
                        nu: 0.886774803963541,
                    },
                    radius: 0.0,
                },
            ),
            (
                "Earth".to_string(),
                Body {
                    name: "Earth".to_string(),
                    orbital_elements: OrbitalElements {
                        a: 1.495979e11,
                        e: 0.01673000,
                        i: 0.000000000000000,
                        raan: 0.000000000000000,
                        argp: 1.796467399077764,
                        nu: 6.238783421468145,
                    },
                    radius: 0.0,
                },
            ),
            (
                "Mars".to_string(),
                Body {
                    name: "Mars".to_string(),
                    orbital_elements: OrbitalElements {
                        a: 2.279423e11,
                        e: 0.09337000,
                        i: 0.032323497746935,
                        raan: 0.867603171166381,
                        argp: 4.998099378936161,
                        nu: 0.407151718007528,
                    },
                    radius: 0.0,
                },
            ),
            (
                "Jupiter".to_string(),
                Body {
                    name: "Jupiter".to_string(),
                    orbital_elements: OrbitalElements {
                        a: 7.782829e11,
                        e: 0.04854000,
                        i: 0.022671826983406,
                        raan: 1.750390706825113,
                        argp: 4.781853084614064,
                        nu: 0.385411781617430,
                    },
                    radius: 0.0,
                },
            ),
            (
                "Saturn".to_string(),
                Body {
                    name: "Saturn".to_string(),
                    orbital_elements: OrbitalElements {
                        a: 1.427388e12,
                        e: 0.05551000,
                        i: 0.043528511544739,
                        raan: 1.983392161966356,
                        argp: 5.920505888615164,
                        nu: 5.457177281331235,
                    },
                    radius: 0.0,
                },
            ),
            (
                "Uranus".to_string(),
                Body {
                    name: "Uranus".to_string(),
                    orbital_elements: OrbitalElements {
                        a: 2.870484e12,
                        e: 0.04686000,
                        i: 0.013491395117916,
                        raan: 1.290845514775006,
                        argp: 1.718625714438817,
                        nu: 2.529765523437681,
                    },
                    radius: 0.0,
                },
            ),
            (
                "Neptune".to_string(),
                Body {
                    name: "Neptune".to_string(),
                    orbital_elements: OrbitalElements {
                        a: 4.498408e12,
                        e: 0.00895000,
                        i: 0.030892327760300,
                        raan: 2.300169421203327,
                        argp: 4.797735580807212,
                        nu: 4.477485531393506,
                    },
                    radius: 0.0,
                },
            ),
        ])
    }
}
