use std::fs;

use astrora_core::core::elements::OrbitalElements;
use bevy::{platform::collections::HashMap, prelude::*};
use bevy_pancam::{PanCam, PanCamPlugin};
use bevy_polyline::PolylinePlugin;
use big_space::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn main() {
    let app = App::new()
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(PanCamPlugin)
        .add_plugins(PolylinePlugin)
        .add_plugins(BigSpaceDefaultPlugins)
        .add_systems(Startup, setup_system)
        .add_systems(Update, update_system)
        .run();
}

fn setup_system(mut commands: Commands) {
    commands.spawn((Camera2d, PanCam::default()));
    load_orbital_data(&mut commands);
}

fn load_orbital_data(commands: &mut Commands) {
    let orbital_data = fs::read_to_string("assets/orbital-data.json").unwrap();
    let orbital_data: HashMap<String, Value> = serde_json::from_str(&orbital_data).unwrap();
    let bodies = orbital_data.get("bodies").unwrap();
    let bodies = bodies.as_object().unwrap();

    let mut bodies_res = HashMap::new();
    for (body_name, body_data) in bodies {
        if body_name == "sun" {
            continue;
        }
        let kind = body_data.get("kind").unwrap().as_str().unwrap().to_string();
        let central_body = body_data
            .get("central_body")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        let mu = body_data.get("mu").unwrap().as_f64().unwrap();
        let radius_m = body_data.get("radius_m").unwrap().as_f64().unwrap();
        let orbit = body_data.get("orbit").unwrap();
        let orbit_serde: OrbitalElementsSerde = serde_json::from_value(orbit.clone()).unwrap();
        let orbit = OrbitalElements::new(
            orbit_serde.a_m,
            orbit_serde.e,
            orbit_serde.i_rad,
            orbit_serde.raan_rad,
            orbit_serde.arg_periapsis_rad,
            orbit_serde.mean_anomaly_rad,
        );
        bodies_res.insert(
            body_name.to_string(),
            BodyData {
                kind,
                central_body,
                mu,
                radius_m,
                orbit,
                epoch: orbit_serde.epoch,
                frame: orbit_serde.frame,
            },
        );
    }
    commands.insert_resource(Bodies(bodies_res));
}

#[derive(Debug, Resource)]
struct Bodies(pub HashMap<String, BodyData>);

#[derive(Debug, Component)]
struct BodyData {
    kind: String,
    central_body: String,
    mu: f64,
    radius_m: f64,
    orbit: OrbitalElements,
    epoch: String,
    frame: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OrbitalElementsSerde {
    a_m: f64,
    e: f64,
    i_rad: f64,
    raan_rad: f64,
    arg_periapsis_rad: f64,
    mean_anomaly_rad: f64,
    epoch: String,
    frame: Option<String>,
}

fn update_system(mut commands: Commands) {}
