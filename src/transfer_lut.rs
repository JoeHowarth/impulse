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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use astrora_core::core::elements::{coe_to_rv, OrbitalElements};
use bevy::prelude::*;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::orbital_data::{propagate_elliptic, Body, MU_SUN};
use crate::transfer::{TransferSolution, compute_transfer};

// ============================================================================
// Configuration
// ============================================================================

/// Number of anomaly buckets (360 / ANOMALY_BUCKETS = bucket size in degrees)
pub const ANOMALY_BUCKETS: usize = 72;
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
    /// Map from "source_idx,target_idx,ν_src_bucket,ν_tgt_bucket,tof_idx" to full solution
    pub entries: HashMap<String, TransferSolution>,

    // Runtime mappings (not serialized)
    /// Entity -> body index in LUT
    #[serde(skip)]
    pub entity_to_idx: HashMap<Entity, usize>,
    /// Body name -> Entity
    #[serde(skip)]
    pub name_to_entity: HashMap<String, Entity>,
}

impl Default for TransferLut {
    fn default() -> Self {
        Self {
            version: LUT_VERSION,
            anomaly_buckets: ANOMALY_BUCKETS,
            tof_candidates: HashMap::new(),
            body_names: Vec::new(),
            entries: HashMap::new(),
            entity_to_idx: HashMap::new(),
            name_to_entity: HashMap::new(),
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
            entity_to_idx: HashMap::new(),
            name_to_entity: HashMap::new(),
        }
    }

    /// Build entity mappings after loading. Call this after deserializing.
    pub fn build_entity_mappings(&mut self, bodies: &Query<(Entity, &Body)>) {
        self.entity_to_idx.clear();
        self.name_to_entity.clear();

        for (entity, body) in bodies.iter() {
            if let Some(idx) = self.body_names.iter().position(|n| n == &body.name) {
                self.entity_to_idx.insert(entity, idx);
                self.name_to_entity.insert(body.name.clone(), entity);
            }
        }

        info!(
            "Built entity mappings: {} bodies mapped",
            self.entity_to_idx.len()
        );
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
        solution: TransferSolution,
    ) {
        let key = Self::make_entry_key(source_idx, target_idx, nu_src_bucket, nu_tgt_bucket, tof_idx);
        self.entries.insert(key, solution);
    }

    /// Set TOF candidates for a body pair
    pub fn set_tof_candidates(&mut self, source_name: &str, target_name: &str, tofs: Vec<i32>) {
        let key = Self::make_pair_key(source_name, target_name);
        self.tof_candidates.insert(key, tofs);
    }

    /// Get TOF candidates for a body pair by name
    pub fn get_tof_candidates(&self, source_name: &str, target_name: &str) -> Option<&Vec<i32>> {
        let key = Self::make_pair_key(source_name, target_name);
        self.tof_candidates.get(&key)
    }

    /// Get TOF candidates for a body pair by entity
    pub fn get_tof_candidates_by_entity(&self, source: Entity, target: Entity) -> Option<&Vec<i32>> {
        let source_idx = *self.entity_to_idx.get(&source)?;
        let target_idx = *self.entity_to_idx.get(&target)?;
        let source_name = &self.body_names[source_idx];
        let target_name = &self.body_names[target_idx];
        self.get_tof_candidates(source_name, target_name)
    }

    /// Find the best transfer in a day range (lowest delta-v)
    ///
    /// Returns (departure_day, tof_days, solution) for the best transfer found.
    pub fn find_best_transfer(
        &self,
        source: Entity,
        target: Entity,
        source_elements: &OrbitalElements,
        target_elements: &OrbitalElements,
        start_day: i32,
        end_day: i32,
    ) -> Option<(i32, i32, TransferSolution)> {
        let source_idx = *self.entity_to_idx.get(&source)?;
        let target_idx = *self.entity_to_idx.get(&target)?;
        let tof_candidates = self.get_tof_candidates_by_entity(source, target)?;

        let mut best: Option<(i32, i32, TransferSolution)> = None;

        for day in start_day..=end_day {
            let nu_src = true_anomaly_at_day(source_elements, day);
            let nu_src_bucket = anomaly_to_bucket(nu_src);

            for (tof_idx, &tof_days) in tof_candidates.iter().enumerate() {
                let arrival_day = day + tof_days;
                let nu_tgt = true_anomaly_at_day(target_elements, arrival_day);
                let nu_tgt_bucket = anomaly_to_bucket(nu_tgt);

                let key = Self::make_entry_key(source_idx, target_idx, nu_src_bucket, nu_tgt_bucket, tof_idx);
                if let Some(solution) = self.entries.get(&key) {
                    let dominated = best.as_ref().map_or(false, |(_, _, b)| solution.total_dv >= b.total_dv);
                    if !dominated {
                        best = Some((day, tof_days, solution.clone()));
                    }
                }
            }
        }

        best
    }

    /// Get a specific transfer solution (for committed legs)
    pub fn get_transfer(
        &self,
        source: Entity,
        target: Entity,
        source_elements: &OrbitalElements,
        target_elements: &OrbitalElements,
        departure_day: i32,
        tof_days: i32,
    ) -> Option<TransferSolution> {
        let source_idx = *self.entity_to_idx.get(&source)?;
        let target_idx = *self.entity_to_idx.get(&target)?;
        let tof_candidates = self.get_tof_candidates_by_entity(source, target)?;

        // Find the TOF index
        let tof_idx = tof_candidates.iter().position(|&t| t == tof_days)?;

        let nu_src = true_anomaly_at_day(source_elements, departure_day);
        let nu_src_bucket = anomaly_to_bucket(nu_src);

        let arrival_day = departure_day + tof_days;
        let nu_tgt = true_anomaly_at_day(target_elements, arrival_day);
        let nu_tgt_bucket = anomaly_to_bucket(nu_tgt);

        let key = Self::make_entry_key(source_idx, target_idx, nu_src_bucket, nu_tgt_bucket, tof_idx);
        self.entries.get(&key).cloned()
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
// Anomaly Helpers
// ============================================================================

/// Compute the true anomaly of a body at a given day
pub fn true_anomaly_at_day(elements: &OrbitalElements, day: i32) -> f64 {
    let dt = day as f64 * 86400.0; // Convert days to seconds
    match propagate_elliptic(*elements, MU_SUN, dt) {
        Ok(propagated) => propagated.nu,
        Err(_) => elements.nu, // Fallback to initial anomaly
    }
}

/// Convert true anomaly (radians) to bucket index (0 to ANOMALY_BUCKETS-1)
pub fn anomaly_to_bucket(nu: f64) -> usize {
    // Normalize to [0, 2π)
    let nu_normalized = nu.rem_euclid(2.0 * PI);
    // Convert to bucket index
    let bucket = (nu_normalized / BUCKET_SIZE_RAD).floor() as usize;
    bucket.min(ANOMALY_BUCKETS - 1)
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

/// Compute full transfer solution between two states
fn compute_transfer_solution(
    r1: astrora_core::core::Vector3,
    v1: astrora_core::core::Vector3,
    r2: astrora_core::core::Vector3,
    v2: astrora_core::core::Vector3,
    tof_seconds: f64,
) -> Option<TransferSolution> {
    // compute_transfer already picks the best of ShortWay/LongWay
    let solution = compute_transfer(r1, v1, r2, v2, tof_seconds, MU_SUN).ok()?;

    // Filter out unreasonable solutions (> 50 km/s total)
    if solution.total_dv > 50_000.0 {
        return None;
    }

    Some(solution)
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

// ============================================================================
// Bevy Systems
// ============================================================================

/// Startup system that loads or generates the transfer LUT
pub fn init_transfer_lut(mut commands: Commands, bodies: Query<(Entity, &Body)>) {
    // Collect expected body names (heliocentric only)
    let expected_bodies: Vec<String> = bodies
        .iter()
        .filter(|(_, b)| is_heliocentric(b))
        .map(|(_, b)| b.name.clone())
        .collect();

    // Try to load from disk, generate if needed
    let mut lut = match TransferLut::load_from_disk() {
        Some(loaded) if loaded.validate(&expected_bodies) => {
            info!("Loaded valid LUT from disk ({} entries)", loaded.entries.len());
            loaded
        }
        Some(_) => {
            info!("LUT validation failed, regenerating...");
            generate_lut_from_query(&bodies)
        }
        None => generate_lut_from_query(&bodies),
    };

    // Build entity mappings (must be done after load since entities aren't serialized)
    lut.build_entity_mappings(&bodies);

    if lut.entity_to_idx.is_empty() {
        warn!("No entity mappings built - LUT may not work correctly");
    }

    commands.insert_resource(lut);
}

/// Generate LUT from a query (used by init_transfer_lut)
fn generate_lut_from_query(bodies: &Query<(Entity, &Body)>) -> TransferLut {
    info!("Generating transfer LUT (using {} threads)...", rayon::current_num_threads());
    let start = Instant::now();

    // Collect heliocentric bodies
    let body_defs: Vec<BodyDef> = bodies
        .iter()
        .filter(|(_, b)| is_heliocentric(b))
        .map(|(_, b)| BodyDef {
            name: b.name.clone(),
            orbital_elements: b.orbital_elements,
        })
        .collect();

    let body_names: Vec<String> = body_defs.iter().map(|b| b.name.clone()).collect();
    let mut lut = TransferLut::new(body_names);

    // Pre-populate TOF candidates (not parallelized - fast enough)
    for (src_idx, source) in body_defs.iter().enumerate() {
        for (tgt_idx, target) in body_defs.iter().enumerate() {
            if src_idx == tgt_idx {
                continue;
            }
            let tof_candidates = get_tof_candidates_for_pair(&source.name, &target.name);
            lut.set_tof_candidates(&source.name, &target.name, tof_candidates.to_vec());
        }
    }

    // Build work items: all (src, tgt, nu_src, nu_tgt, tof) combinations
    let mut work_items: Vec<(usize, usize, usize, usize, usize, f64)> = Vec::new();
    for (src_idx, source) in body_defs.iter().enumerate() {
        for (tgt_idx, target) in body_defs.iter().enumerate() {
            if src_idx == tgt_idx {
                continue;
            }
            let tof_candidates = get_tof_candidates_for_pair(&source.name, &target.name);
            for nu_src_bucket in 0..ANOMALY_BUCKETS {
                for nu_tgt_bucket in 0..ANOMALY_BUCKETS {
                    for (tof_idx, &tof_days) in tof_candidates.iter().enumerate() {
                        work_items.push((
                            src_idx,
                            tgt_idx,
                            nu_src_bucket,
                            nu_tgt_bucket,
                            tof_idx,
                            tof_days as f64 * 86400.0,
                        ));
                    }
                }
            }
        }
    }

    let total_items = work_items.len();
    let computed = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);

    // Compute transfers in parallel
    let results: Vec<_> = work_items
        .par_iter()
        .filter_map(|&(src_idx, tgt_idx, nu_src_bucket, nu_tgt_bucket, tof_idx, tof_seconds)| {
            let source = &body_defs[src_idx];
            let target = &body_defs[tgt_idx];

            let nu_src = bucket_center_anomaly(nu_src_bucket);
            let (r1, v1) = state_at_anomaly(&source.orbital_elements, nu_src);

            let nu_tgt = bucket_center_anomaly(nu_tgt_bucket);
            let (r2, v2) = state_at_anomaly(&target.orbital_elements, nu_tgt);

            match compute_transfer_solution(r1, v1, r2, v2, tof_seconds) {
                Some(solution) => {
                    computed.fetch_add(1, Ordering::Relaxed);
                    Some((src_idx, tgt_idx, nu_src_bucket, nu_tgt_bucket, tof_idx, solution))
                }
                None => {
                    failed.fetch_add(1, Ordering::Relaxed);
                    None
                }
            }
        })
        .collect();

    // Insert results into LUT (single-threaded, but fast)
    for (src_idx, tgt_idx, nu_src_bucket, nu_tgt_bucket, tof_idx, solution) in results {
        lut.insert(src_idx, tgt_idx, nu_src_bucket, nu_tgt_bucket, tof_idx, solution);
    }

    let elapsed = start.elapsed();
    let computed_count = computed.load(Ordering::Relaxed);
    let failed_count = failed.load(Ordering::Relaxed);
    info!(
        "Generated LUT: {} entries ({} failed) in {:.1}s ({} work items)",
        computed_count, failed_count, elapsed.as_secs_f64(), total_items
    );

    // Save to disk
    if let Err(e) = lut.save_to_disk() {
        warn!("Failed to save LUT: {}", e);
    }

    lut
}
