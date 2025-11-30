//! Transfer lookup table (LUT) for precomputed Lambert solutions.
//!
//! The LUT stores delta-v values keyed by (source, target, ν_source_bucket, ν_target_bucket, tof_idx).
//! This allows instant lookup of transfer costs without solving Lambert's problem at runtime.
//!
//! At startup, the system:
//! 1. Attempts to load the LUT from disk
//! 2. Validates it against current configuration (bodies, buckets, TOFs)
//! 3. If invalid or missing, regenerates it (may take 10-15s)
//! 4. Saves the regenerated LUT to disk for next time

use std::collections::HashMap;
use std::f64::consts::PI;
use std::path::Path;
use std::time::Instant;

use astrora_core::core::elements::{coe_to_rv, OrbitalElements};
use astrora_core::maneuvers::{Lambert, TransferKind};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::orbital_data::{Body, MU_SUN};

// ============================================================================
// Configuration
// ============================================================================

/// Number of anomaly buckets (10° per bucket)
pub const ANOMALY_BUCKETS: usize = 36;
const BUCKET_SIZE_RAD: f64 = 2.0 * PI / ANOMALY_BUCKETS as f64;

/// Path to the LUT file
const LUT_PATH: &str = "assets/transfer_lut.bin";

/// LUT format version - increment when changing structure
const LUT_VERSION: u32 = 1;

// TOF candidates by distance category
const TOF_INNER: &[i32] = &[60, 80, 100, 120, 150, 180, 200, 220, 250, 280, 300, 350, 400, 450, 500];
const TOF_BELT: &[i32] = &[150, 200, 250, 300, 350, 400, 450, 500, 600, 700, 800];
const TOF_JUPITER: &[i32] = &[400, 500, 600, 700, 800, 900, 1000, 1100, 1200, 1400, 1600];
const TOF_SATURN: &[i32] = &[1000, 1200, 1400, 1600, 1800, 2000, 2200, 2400, 2600, 2800, 3000];
const TOF_URANUS: &[i32] = &[1500, 2000, 2500, 3000, 3500, 4000, 4500, 5000, 5500, 6000];
const TOF_NEPTUNE: &[i32] = &[3000, 4000, 5000, 6000, 7000, 8000, 9000, 10000, 11000, 12000];

// ============================================================================
// Types
// ============================================================================

/// A single LUT entry storing transfer delta-v
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferEntry {
    /// Total delta-v in m/s
    pub total_dv: f64,
    /// Departure delta-v magnitude in m/s
    pub departure_dv: f64,
    /// Arrival delta-v magnitude in m/s
    pub arrival_dv: f64,
}

/// The complete transfer LUT
#[derive(Clone, Debug, Serialize, Deserialize, Resource)]
pub struct TransferLut {
    /// Format version for validation
    pub version: u32,
    /// Number of anomaly buckets (same for source and target)
    pub anomaly_buckets: usize,
    /// TOF candidates per body pair: "src_name,tgt_name" -> Vec<tof_days>
    pub tof_candidates: HashMap<String, Vec<i32>>,
    /// Body names in order (index = body ID used in entries)
    pub body_names: Vec<String>,
    /// Map from "source_idx,target_idx,ν_src_bucket,ν_tgt_bucket,tof_idx" to entry
    pub entries: HashMap<String, TransferEntry>,
}

impl Default for TransferLut {
    fn default() -> Self {
        Self {
            version: LUT_VERSION,
            anomaly_buckets: ANOMALY_BUCKETS,
            tof_candidates: HashMap::new(),
            body_names: Vec::new(),
            entries: HashMap::new(),
        }
    }
}

impl TransferLut {
    /// Create a new empty LUT with the given body names
    pub fn new(body_names: Vec<String>) -> Self {
        Self {
            version: LUT_VERSION,
            anomaly_buckets: ANOMALY_BUCKETS,
            tof_candidates: HashMap::new(),
            body_names,
            entries: HashMap::new(),
        }
    }

    /// Get the body index for a given name
    pub fn body_index(&self, name: &str) -> Option<usize> {
        self.body_names.iter().position(|n| n == name)
    }

    /// Make a key for the entries map
    fn make_entry_key(
        source_idx: usize,
        target_idx: usize,
        nu_src_bucket: usize,
        nu_tgt_bucket: usize,
        tof_idx: usize,
    ) -> String {
        format!(
            "{},{},{},{},{}",
            source_idx, target_idx, nu_src_bucket, nu_tgt_bucket, tof_idx
        )
    }

    /// Make a key for the tof_candidates map
    fn make_pair_key(source_name: &str, target_name: &str) -> String {
        format!("{},{}", source_name, target_name)
    }

    /// Insert an entry
    pub fn insert(
        &mut self,
        source_idx: usize,
        target_idx: usize,
        nu_src_bucket: usize,
        nu_tgt_bucket: usize,
        tof_idx: usize,
        entry: TransferEntry,
    ) {
        let key = Self::make_entry_key(source_idx, target_idx, nu_src_bucket, nu_tgt_bucket, tof_idx);
        self.entries.insert(key, entry);
    }

    /// Set TOF candidates for a body pair
    pub fn set_tof_candidates(&mut self, source_name: &str, target_name: &str, tofs: Vec<i32>) {
        let key = Self::make_pair_key(source_name, target_name);
        self.tof_candidates.insert(key, tofs);
    }

    /// Get TOF candidates for a body pair
    pub fn get_tof_candidates(&self, source_name: &str, target_name: &str) -> Option<&Vec<i32>> {
        let key = Self::make_pair_key(source_name, target_name);
        self.tof_candidates.get(&key)
    }

    /// Look up a transfer entry
    pub fn get(
        &self,
        source_name: &str,
        target_name: &str,
        nu_src_bucket: usize,
        nu_tgt_bucket: usize,
        tof_idx: usize,
    ) -> Option<&TransferEntry> {
        let source_idx = self.body_index(source_name)?;
        let target_idx = self.body_index(target_name)?;
        let key = Self::make_entry_key(source_idx, target_idx, nu_src_bucket, nu_tgt_bucket, tof_idx);
        self.entries.get(&key)
    }

    /// Validate that this LUT matches the given body configuration
    pub fn validate(&self, expected_bodies: &[String]) -> bool {
        if self.version != LUT_VERSION {
            info!("LUT version mismatch: {} != {}", self.version, LUT_VERSION);
            return false;
        }
        if self.anomaly_buckets != ANOMALY_BUCKETS {
            info!("LUT bucket count mismatch: {} != {}", self.anomaly_buckets, ANOMALY_BUCKETS);
            return false;
        }
        if self.body_names != expected_bodies {
            info!("LUT body list mismatch");
            return false;
        }
        if self.entries.is_empty() {
            info!("LUT has no entries");
            return false;
        }
        true
    }

    /// Load LUT from disk
    pub fn load_from_disk() -> Option<Self> {
        let path = Path::new(LUT_PATH);
        if !path.exists() {
            info!("LUT file not found at {}", LUT_PATH);
            return None;
        }

        let data = std::fs::read(path).ok()?;
        match bincode::deserialize(&data) {
            Ok(lut) => Some(lut),
            Err(e) => {
                warn!("Failed to deserialize LUT: {}", e);
                None
            }
        }
    }

    /// Save LUT to disk
    pub fn save_to_disk(&self) -> Result<(), String> {
        // Ensure directory exists
        if let Some(parent) = Path::new(LUT_PATH).parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let data = bincode::serialize(self).map_err(|e| e.to_string())?;
        std::fs::write(LUT_PATH, data).map_err(|e| e.to_string())?;

        let size_mb = std::fs::metadata(LUT_PATH)
            .map(|m| m.len() as f64 / 1_000_000.0)
            .unwrap_or(0.0);
        info!("Saved LUT to {} ({:.1} MB)", LUT_PATH, size_mb);

        Ok(())
    }
}

// ============================================================================
// Body Classification
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyCategory {
    Inner,
    Belt,
    Jupiter,
    Saturn,
    Uranus,
    Neptune,
}

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

/// Get appropriate TOF candidates for a transfer between two bodies
pub fn get_tof_candidates_for_pair(source: &str, target: &str) -> &'static [i32] {
    let src_cat = categorize_body(source);
    let tgt_cat = categorize_body(target);

    let max_cat = match (src_cat, tgt_cat) {
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

// ============================================================================
// Generation
// ============================================================================

/// Get orbital state (position, velocity) for a body at a given true anomaly
fn state_at_anomaly(elements: &OrbitalElements, nu: f64) -> (astrora_core::core::Vector3, astrora_core::core::Vector3) {
    let elem_at_nu = OrbitalElements {
        a: elements.a,
        e: elements.e,
        i: elements.i,
        raan: elements.raan,
        argp: elements.argp,
        nu,
    };
    coe_to_rv(&elem_at_nu, MU_SUN)
}

/// Get the center true anomaly for a bucket index
fn bucket_center_anomaly(bucket: usize) -> f64 {
    (bucket as f64 + 0.5) * BUCKET_SIZE_RAD
}

/// Compute transfer delta-v between two states
fn compute_transfer_dv(
    r1: astrora_core::core::Vector3,
    v1: astrora_core::core::Vector3,
    r2: astrora_core::core::Vector3,
    v2: astrora_core::core::Vector3,
    tof_seconds: f64,
) -> Option<TransferEntry> {
    let mut best: Option<TransferEntry> = None;

    for kind in [TransferKind::ShortWay, TransferKind::LongWay] {
        let Ok(lambert) = Lambert::solve(r1, r2, tof_seconds, MU_SUN, kind, 0) else {
            continue;
        };

        let dep_dv = (lambert.v1 - v1).norm();
        let arr_dv = (v2 - lambert.v2).norm();
        let total_dv = dep_dv + arr_dv;

        // Filter out unreasonable solutions (> 50 km/s total)
        if total_dv > 50_000.0 {
            continue;
        }

        match &best {
            None => {
                best = Some(TransferEntry {
                    total_dv,
                    departure_dv: dep_dv,
                    arrival_dv: arr_dv,
                });
            }
            Some(b) if total_dv < b.total_dv => {
                best = Some(TransferEntry {
                    total_dv,
                    departure_dv: dep_dv,
                    arrival_dv: arr_dv,
                });
            }
            _ => {}
        }
    }

    best
}

/// Heliocentric body data for LUT generation
struct BodyDef {
    name: String,
    orbital_elements: OrbitalElements,
}

/// Check if a body is heliocentric (orbits the Sun directly)
fn is_heliocentric(body: &Body) -> bool {
    body.parent_name.as_deref() == Some("Sun")
}

/// Generate the transfer LUT from scratch
pub fn generate_lut(bodies: &Query<&Body>) -> TransferLut {
    info!("Generating transfer LUT (this may take 10-15 seconds)...");
    let start = Instant::now();

    // Collect heliocentric bodies
    let body_defs: Vec<BodyDef> = bodies
        .iter()
        .filter(|b| is_heliocentric(b))
        .map(|b| BodyDef {
            name: b.name.clone(),
            orbital_elements: b.orbital_elements,
        })
        .collect();

    let body_names: Vec<String> = body_defs.iter().map(|b| b.name.clone()).collect();
    let mut lut = TransferLut::new(body_names);

    let mut computed = 0;
    let mut failed = 0;

    for (src_idx, source) in body_defs.iter().enumerate() {
        for (tgt_idx, target) in body_defs.iter().enumerate() {
            if src_idx == tgt_idx {
                continue;
            }

            let tof_candidates = get_tof_candidates_for_pair(&source.name, &target.name);
            lut.set_tof_candidates(&source.name, &target.name, tof_candidates.to_vec());

            for nu_src_bucket in 0..ANOMALY_BUCKETS {
                let nu_src = bucket_center_anomaly(nu_src_bucket);
                let (r1, v1) = state_at_anomaly(&source.orbital_elements, nu_src);

                for nu_tgt_bucket in 0..ANOMALY_BUCKETS {
                    let nu_tgt = bucket_center_anomaly(nu_tgt_bucket);
                    let (r2, v2) = state_at_anomaly(&target.orbital_elements, nu_tgt);

                    for (tof_idx, &tof_days) in tof_candidates.iter().enumerate() {
                        let tof_seconds = tof_days as f64 * 86400.0;

                        if let Some(entry) = compute_transfer_dv(r1, v1, r2, v2, tof_seconds) {
                            lut.insert(src_idx, tgt_idx, nu_src_bucket, nu_tgt_bucket, tof_idx, entry);
                            computed += 1;
                        } else {
                            failed += 1;
                        }
                    }
                }
            }
        }
    }

    let elapsed = start.elapsed();
    info!(
        "Generated LUT: {} entries ({} failed) in {:.1}s",
        computed, failed, elapsed.as_secs_f64()
    );

    lut
}

// ============================================================================
// Bevy Systems
// ============================================================================

/// Startup system that loads or generates the transfer LUT
pub fn init_transfer_lut(mut commands: Commands, bodies: Query<&Body>) {
    // Collect expected body names (heliocentric only)
    let expected_bodies: Vec<String> = bodies
        .iter()
        .filter(|b| is_heliocentric(b))
        .map(|b| b.name.clone())
        .collect();

    // Try to load from disk
    if let Some(lut) = TransferLut::load_from_disk() {
        if lut.validate(&expected_bodies) {
            info!("Loaded valid LUT from disk ({} entries)", lut.entries.len());
            commands.insert_resource(lut);
            return;
        }
        info!("LUT validation failed, regenerating...");
    }

    // Generate new LUT
    let lut = generate_lut(&bodies);

    // Save to disk for next time
    if let Err(e) = lut.save_to_disk() {
        warn!("Failed to save LUT: {}", e);
    }

    commands.insert_resource(lut);
}
