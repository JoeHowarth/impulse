//! Standalone tool to generate/regenerate the transfer LUT.
//!
//! This is useful for:
//! - Pre-generating the LUT before distributing the game
//! - Regenerating after changing TOF candidates or body configurations
//! - Testing LUT generation without starting the full game
//!
//! Usage: cargo run --bin gen_transfer_lut --release [--json]

use std::collections::HashMap;
use std::f64::consts::PI;
use std::io::Write;
use std::time::Instant;

use astrora_core::core::elements::{coe_to_rv, OrbitalElements};
use astrora_core::maneuvers::{Lambert, TransferKind};
use serde::{Deserialize, Serialize};

// Mirror the types from transfer_lut.rs (can't import due to bevy dependency)
const MU_SUN: f64 = 1.327_124_400_18e20;
const ANOMALY_BUCKETS: usize = 36;
const BUCKET_SIZE_RAD: f64 = 2.0 * PI / ANOMALY_BUCKETS as f64;
const LUT_VERSION: u32 = 1;

const TOF_INNER: &[i32] = &[60, 80, 100, 120, 150, 180, 200, 220, 250, 280, 300, 350, 400, 450, 500];
const TOF_BELT: &[i32] = &[150, 200, 250, 300, 350, 400, 450, 500, 600, 700, 800];
const TOF_JUPITER: &[i32] = &[400, 500, 600, 700, 800, 900, 1000, 1100, 1200, 1400, 1600];
const TOF_SATURN: &[i32] = &[1000, 1200, 1400, 1600, 1800, 2000, 2200, 2400, 2600, 2800, 3000];
const TOF_URANUS: &[i32] = &[1500, 2000, 2500, 3000, 3500, 4000, 4500, 5000, 5500, 6000];
const TOF_NEPTUNE: &[i32] = &[3000, 4000, 5000, 6000, 7000, 8000, 9000, 10000, 11000, 12000];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferEntry {
    pub total_dv: f64,
    pub departure_dv: f64,
    pub arrival_dv: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferLut {
    pub version: u32,
    pub anomaly_buckets: usize,
    pub tof_candidates: HashMap<String, Vec<i32>>,
    pub body_names: Vec<String>,
    pub entries: HashMap<String, TransferEntry>,
}

impl TransferLut {
    fn new(body_names: Vec<String>) -> Self {
        Self {
            version: LUT_VERSION,
            anomaly_buckets: ANOMALY_BUCKETS,
            tof_candidates: HashMap::new(),
            body_names,
            entries: HashMap::new(),
        }
    }

    fn make_entry_key(src: usize, tgt: usize, nu_src: usize, nu_tgt: usize, tof: usize) -> String {
        format!("{},{},{},{},{}", src, tgt, nu_src, nu_tgt, tof)
    }

    fn make_pair_key(source: &str, target: &str) -> String {
        format!("{},{}", source, target)
    }

    fn insert(&mut self, src: usize, tgt: usize, nu_src: usize, nu_tgt: usize, tof: usize, entry: TransferEntry) {
        self.entries.insert(Self::make_entry_key(src, tgt, nu_src, nu_tgt, tof), entry);
    }

    fn set_tof_candidates(&mut self, source: &str, target: &str, tofs: Vec<i32>) {
        self.tof_candidates.insert(Self::make_pair_key(source, target), tofs);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyCategory { Inner, Belt, Jupiter, Saturn, Uranus, Neptune }

fn categorize_body(name: &str) -> BodyCategory {
    match name {
        "Mercury" | "Venus" | "Earth" | "Mars" => BodyCategory::Inner,
        "Ceres" | "Vesta" => BodyCategory::Belt,
        "Jupiter" => BodyCategory::Jupiter,
        "Saturn" => BodyCategory::Saturn,
        "Uranus" => BodyCategory::Uranus,
        "Neptune" => BodyCategory::Neptune,
        _ => BodyCategory::Inner,
    }
}

fn get_tof_candidates(source: &str, target: &str) -> &'static [i32] {
    let max_cat = match (categorize_body(source), categorize_body(target)) {
        (BodyCategory::Neptune, _) | (_, BodyCategory::Neptune) => BodyCategory::Neptune,
        (BodyCategory::Uranus, _) | (_, BodyCategory::Uranus) => BodyCategory::Uranus,
        (BodyCategory::Saturn, _) | (_, BodyCategory::Saturn) => BodyCategory::Saturn,
        (BodyCategory::Jupiter, _) | (_, BodyCategory::Jupiter) => BodyCategory::Jupiter,
        (BodyCategory::Belt, _) | (_, BodyCategory::Belt) => BodyCategory::Belt,
        _ => BodyCategory::Inner,
    };
    match max_cat {
        BodyCategory::Inner => TOF_INNER,
        BodyCategory::Belt => TOF_BELT,
        BodyCategory::Jupiter => TOF_JUPITER,
        BodyCategory::Saturn => TOF_SATURN,
        BodyCategory::Uranus => TOF_URANUS,
        BodyCategory::Neptune => TOF_NEPTUNE,
    }
}

struct BodyDef { name: String, orbital_elements: OrbitalElements }

fn state_at_anomaly(elements: &OrbitalElements, nu: f64) -> (astrora_core::core::Vector3, astrora_core::core::Vector3) {
    coe_to_rv(&OrbitalElements { nu, ..*elements }, MU_SUN)
}

fn bucket_center_anomaly(bucket: usize) -> f64 {
    (bucket as f64 + 0.5) * BUCKET_SIZE_RAD
}

fn compute_transfer_dv(
    r1: astrora_core::core::Vector3, v1: astrora_core::core::Vector3,
    r2: astrora_core::core::Vector3, v2: astrora_core::core::Vector3,
    tof_seconds: f64,
) -> Option<TransferEntry> {
    let mut best: Option<TransferEntry> = None;
    for kind in [TransferKind::ShortWay, TransferKind::LongWay] {
        let Ok(lambert) = Lambert::solve(r1, r2, tof_seconds, MU_SUN, kind, 0) else { continue };
        let dep_dv = (lambert.v1 - v1).norm();
        let arr_dv = (v2 - lambert.v2).norm();
        let total_dv = dep_dv + arr_dv;
        if total_dv > 50_000.0 { continue; }
        if best.as_ref().map_or(true, |b| total_dv < b.total_dv) {
            best = Some(TransferEntry { total_dv, departure_dv: dep_dv, arrival_dv: arr_dv });
        }
    }
    best
}

fn get_heliocentric_bodies() -> Vec<BodyDef> {
    vec![
        BodyDef { name: "Mercury".into(), orbital_elements: OrbitalElements { a: 5.790905e10, e: 0.205630, i: 0.122258, raan: 0.843547, argp: 0.508309, nu: 0.0 }},
        BodyDef { name: "Venus".into(), orbital_elements: OrbitalElements { a: 1.08208e11, e: 0.006772, i: 0.059248, raan: 1.3383, argp: 0.9579, nu: 0.0 }},
        BodyDef { name: "Earth".into(), orbital_elements: OrbitalElements { a: 1.49598e11, e: 0.016708, i: 0.00005, raan: 0.0, argp: 1.7967, nu: 0.0 }},
        BodyDef { name: "Mars".into(), orbital_elements: OrbitalElements { a: 2.27939e11, e: 0.09340, i: 0.03229, raan: 0.8653, argp: 4.9997, nu: 0.0 }},
        BodyDef { name: "Ceres".into(), orbital_elements: OrbitalElements { a: 4.137e11, e: 0.0760, i: 0.1850, raan: 1.401, argp: 1.284, nu: 0.0 }},
        BodyDef { name: "Vesta".into(), orbital_elements: OrbitalElements { a: 3.532e11, e: 0.0891, i: 0.1246, raan: 1.811, argp: 2.622, nu: 0.0 }},
        BodyDef { name: "Jupiter".into(), orbital_elements: OrbitalElements { a: 7.7857e11, e: 0.04890, i: 0.02276, raan: 1.7550, argp: 4.7786, nu: 0.0 }},
        BodyDef { name: "Saturn".into(), orbital_elements: OrbitalElements { a: 1.43353e12, e: 0.05650, i: 0.04336, raan: 1.9847, argp: 5.9048, nu: 0.0 }},
        BodyDef { name: "Uranus".into(), orbital_elements: OrbitalElements { a: 2.87246e12, e: 0.04638, i: 0.01343, raan: 1.2955, argp: 1.6969, nu: 0.0 }},
        BodyDef { name: "Neptune".into(), orbital_elements: OrbitalElements { a: 4.49506e12, e: 0.00945, i: 0.03087, raan: 2.2989, argp: 4.7477, nu: 0.0 }},
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let use_json = args.iter().any(|a| a == "--json");
    let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");

    let bodies = get_heliocentric_bodies();
    let body_names: Vec<String> = bodies.iter().map(|b| b.name.clone()).collect();

    println!("Generating transfer LUT for {} bodies", bodies.len());
    println!("  Anomaly buckets: {} ({}° each)", ANOMALY_BUCKETS, 360.0 / ANOMALY_BUCKETS as f64);
    println!("  {} body pairs to compute", bodies.len() * (bodies.len() - 1));

    let mut lut = TransferLut::new(body_names);
    let start = Instant::now();
    let mut computed = 0;
    let mut failed = 0;

    for (src_idx, source) in bodies.iter().enumerate() {
        for (tgt_idx, target) in bodies.iter().enumerate() {
            if src_idx == tgt_idx { continue; }

            let tof_candidates = get_tof_candidates(&source.name, &target.name);
            lut.set_tof_candidates(&source.name, &target.name, tof_candidates.to_vec());

            let pair_start = Instant::now();
            let mut pair_computed = 0;

            for nu_src_bucket in 0..ANOMALY_BUCKETS {
                let nu_src = bucket_center_anomaly(nu_src_bucket);
                let (r1, v1) = state_at_anomaly(&source.orbital_elements, nu_src);

                for nu_tgt_bucket in 0..ANOMALY_BUCKETS {
                    let nu_tgt = bucket_center_anomaly(nu_tgt_bucket);
                    let (r2, v2) = state_at_anomaly(&target.orbital_elements, nu_tgt);

                    for (tof_idx, &tof_days) in tof_candidates.iter().enumerate() {
                        if let Some(entry) = compute_transfer_dv(r1, v1, r2, v2, tof_days as f64 * 86400.0) {
                            lut.insert(src_idx, tgt_idx, nu_src_bucket, nu_tgt_bucket, tof_idx, entry);
                            pair_computed += 1;
                            computed += 1;
                        } else {
                            failed += 1;
                        }
                    }
                }
            }

            if verbose {
                println!("  {} -> {}: {} entries ({:.1}s)", source.name, target.name, pair_computed, pair_start.elapsed().as_secs_f64());
            } else {
                print!(".");
                std::io::stdout().flush().unwrap();
            }
        }
    }

    if !verbose { println!(); }

    println!("Computed {} entries ({} failed) in {:.1}s", computed, failed, start.elapsed().as_secs_f64());

    let output_path = if use_json { "assets/transfer_lut.json" } else { "assets/transfer_lut.bin" };
    std::fs::create_dir_all("assets").expect("Failed to create assets directory");

    if use_json {
        std::fs::write(output_path, serde_json::to_string_pretty(&lut).unwrap()).unwrap();
    } else {
        std::fs::write(output_path, bincode::serialize(&lut).unwrap()).unwrap();
    }

    let size_mb = std::fs::metadata(output_path).map(|m| m.len() as f64 / 1_000_000.0).unwrap_or(0.0);
    println!("Wrote {} ({:.1} MB)", output_path, size_mb);
}
