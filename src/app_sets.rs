use bevy::prelude::*;

#[derive(SystemSet, Debug, Clone, Eq, PartialEq, Hash)]
pub enum AppSet {
    Input,
    Simulation,
    Ui,
    Render,
}
