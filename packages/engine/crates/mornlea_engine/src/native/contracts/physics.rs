use super::{CollisionGrid, KernelError};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsState {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub on_ground: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsControls {
    pub move_x: i8,
    pub move_z: i8,
    pub jump: bool,
    pub yaw_sin: f32,
    pub yaw_cos: f32,
    pub body_in_fluid: bool,
    pub sprinting: bool,
    pub sneaking: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsTuning {
    pub fixed_delta_seconds: f32,
    pub step_height: f32,
    pub walk_speed: f32,
    pub ground_acceleration: f32,
    pub ground_deceleration: f32,
    pub air_acceleration: f32,
    pub jump_speed: f32,
    pub gravity: f32,
    pub terminal_fall_speed: f32,
    pub fluid_gravity: f32,
    pub fluid_sink_speed: f32,
    pub fluid_ascend_speed: f32,
    pub fluid_horizontal_drag: f32,
    pub sprint_speed_multiplier: f32,
    pub sneak_speed_multiplier: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepBounds {
    pub minimum: [f32; 3],
    pub maximum: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsRequest<'a> {
    pub state: PhysicsState,
    pub controls: PhysicsControls,
    pub tuning: PhysicsTuning,
    pub sweep: SweepBounds,
    pub grid: CollisionGrid<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsResult {
    pub state: PhysicsState,
    pub clipped: [bool; 3],
    pub used_step: bool,
    pub hit_unknown: bool,
}

pub trait PhysicsOp {
    fn step(&self, request: &PhysicsRequest<'_>) -> Result<PhysicsResult, KernelError>;
}
