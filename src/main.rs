#![allow(unused_imports)]
use std::{cell::LazyCell, fs};

use astrora_core::core::{
    Vector3,
    elements::{OrbitalElements, coe_to_rv},
    integrators_static::position,
};
use bevy::{math::Vec3, platform::collections::HashMap, prelude::*};
use bevy_pancam::{PanCam, PanCamPlugin};
use bevy_vector_shapes::prelude::*;
use big_space::prelude::*;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::orbital_data::{Body, MU_SUN, PlanetaryElements, propagate_elliptic};

pub mod orbital_data;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(PanCamPlugin)
        .add_plugins(ShapePlugin::default())
        .add_plugins(BigSpaceDefaultPlugins)
        .add_systems(Startup, setup_system)
        .add_systems(Update, update_system)
        .run();
}

const ORBIT_SEGMENTS: usize = 1000;
const AU: LazyCell<f64> = LazyCell::new(|| {
    PlanetaryElements::get_planetary_elements()
        .get("Earth")
        .unwrap()
        .orbital_elements
        .a as f64
});

fn setup_system(mut commands: Commands, mut shapes: ShapeCommands) {
    // spawn a 3D orthographic camera looking at the XY plane
    commands.spawn((
        Camera3d::default(),
        Projection::from(OrthographicProjection {
            far: 10_000.0,
            near: -10_000.0,
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 0.0, 1000.0).looking_at(Vec3::ZERO, Vec3::Y),
        PanCam::default(),
    ));

    // spawn the bodies
    let planetary_elements = PlanetaryElements::get_planetary_elements();

    commands.insert_resource(PlanetaryElements {
        bodies: planetary_elements.clone(),
    });

    // Configure retained shape settings for orbit lines
    shapes.set_color(Color::srgb(0., 1., 1.));
    shapes.thickness = 1.5;

    for (_, body) in planetary_elements {
        let orbital_elements = body.orbital_elements;
        let period = orbital_elements.period(MU_SUN).unwrap();
        // calculate the number of segments based on the period and the number of segments
        let dt = period / ORBIT_SEGMENTS as f64;
        let mut vertices = Vec::with_capacity(ORBIT_SEGMENTS);
        for i in 0..=ORBIT_SEGMENTS {
            let t = i as f64 * dt;
            let orbital_elements: OrbitalElements =
                propagate_elliptic(orbital_elements, MU_SUN, t).unwrap();
            let (r, _v): (Vector3, Vector3) = coe_to_rv(&orbital_elements, MU_SUN);
            let r = r * 100. / *AU;
            vertices.push(Vec2::new(r.x as f32, r.y as f32));
        }
        // Retained: spawn one line per segment, once
        let mut last = None;
        for v in vertices.into_iter().map(|v| Vec3::new(v.x, v.y, -1.0)) {
            if let Some(prev) = last {
                shapes.line(prev, v);
            }
            last = Some(v);
        }

        // Spawn body component for position updates
        commands.spawn(body);
    }
}

fn update_system(query: Query<(&Body,)>, time: Res<Time>, mut painter: ShapePainter) {
    let _t = time.elapsed_secs_f64();
    let days = _t * 60. * 60. * 24.; // convert to days
    let days_per_step = days * 10.;

    for (body,) in query.iter() {
        let curr_elems: OrbitalElements =
            propagate_elliptic(body.orbital_elements, MU_SUN, days_per_step).unwrap();

        let (r_body, _v): (Vector3, Vector3) = coe_to_rv(&curr_elems, MU_SUN);
        let r_body = r_body * 100. / *AU;
        let vec3 = Vec3::new(r_body.x as f32, r_body.y as f32, 0.);

        painter.translate(vec3);
        painter.set_color(Color::xyz(1., 0., 0.));
        painter.circle(10.);
        painter.reset();
    }
}
