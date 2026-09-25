//! physics step ABI 的输入解析、校验、积分与输出编码。
//!
//! 输入布局与偏移以 `docs/superpowers/specs/2026-08-15-rust-engine-physics-step-design.md`
//! 第 4 节为准：header（magic MGP1 + layout 版本）+ 每 cell 196 字节；header 现为 v4、160 字节。

/// StepInput header 长度。v1 128，v2 160（浸没标志+水中 tunable），v3
/// 复用保留区追加疾跑位与倍率（129 位 + 148..152 multiplier），v4 在 130 置
/// 潜行位、152..156 置潜行倍率，总长保持 160（32 整数倍），仍是同一
/// engine ABI v5 内的 header 扩展。
pub(crate) const STEP_HEADER_BYTES: usize = 160;

/// StepInput header 的布局版本。v3 → v4 追加潜行位与潜行倍率。
const STEP_LAYOUT_VERSION: u32 = 4;
pub(crate) const STEP_OUTPUT_BYTES: usize = 32;
pub(crate) const STEP_MAX_CELLS: usize = 4096;

const CELL_BYTES: usize = 196;

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated range"),
    )
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated range"),
    )
}

fn read_f32(bytes: &[u8], offset: usize) -> f32 {
    f32::from_bits(read_u32(bytes, offset))
}

pub(crate) struct StepInput<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) position: [f32; 3],
    pub(crate) velocity: [f32; 3],
    pub(crate) on_ground: bool,
    pub(crate) jump: bool,
    pub(crate) move_x: i8,
    pub(crate) move_z: i8,
    pub(crate) yaw_sin: f32,
    pub(crate) yaw_cos: f32,
    pub(crate) fixed_delta_seconds: f32,
    pub(crate) step_height: f32,
    pub(crate) walk_speed: f32,
    pub(crate) ground_acceleration: f32,
    pub(crate) ground_deceleration: f32,
    pub(crate) air_acceleration: f32,
    pub(crate) jump_speed: f32,
    pub(crate) gravity: f32,
    pub(crate) terminal_fall_speed: f32,
    /// 身体浸没标志。流体没有碰撞盒，prism 里区分不出水与空气，因此由 Go
    /// 调用方从各自的方块镜像算好后随 header 传入（design D4）。
    pub(crate) body_in_fluid: bool,
    /// 疾跑位（地面+前移+非浸没时提升目标速度，潜行同置时让位）。
    pub(crate) sprinting: bool,
    /// 潜行位（地面+非浸没时降低目标速度，优先于疾跑）。
    pub(crate) sneaking: bool,
    /// 水中重力，替换 gravity。
    pub(crate) fluid_gravity: f32,
    /// 水中垂直终端下沉速度，替换 terminal_fall_speed。
    pub(crate) fluid_sink_speed: f32,
    /// 水中持续上浮速度：Jump 在水中不是冲量，而是每 tick 直接赋值。
    pub(crate) fluid_ascend_speed: f32,
    /// 水中水平阻力系数，每 tick 乘在水平速度上。
    pub(crate) fluid_horizontal_drag: f32,
    /// 疾跑倍率（默认 1.3）。
    pub(crate) sprint_speed_multiplier: f32,
    /// 潜行倍率（默认 0.3）。
    pub(crate) sneak_speed_multiplier: f32,
    pub(crate) sweep_min: [f32; 3],
    pub(crate) sweep_max: [f32; 3],
    pub(crate) origin: [i32; 3],
    pub(crate) dimensions: [u32; 3],
}

impl<'a> StepInput<'a> {
    pub(crate) fn decode(bytes: &'a [u8]) -> Self {
        let mut tunables = [0.0f32; 8];
        for (index, slot) in tunables.iter_mut().enumerate() {
            *slot = read_f32(bytes, 48 + index * 4);
        }
        let mut sweep_min = [0.0f32; 3];
        let mut sweep_max = [0.0f32; 3];
        for axis in 0..3 {
            sweep_min[axis] = read_f32(bytes, 80 + axis * 8);
            sweep_max[axis] = read_f32(bytes, 84 + axis * 8);
        }
        let mut origin = [0i32; 3];
        let mut dimensions = [0u32; 3];
        for axis in 0..3 {
            origin[axis] = read_i32(bytes, 104 + axis * 4);
            dimensions[axis] = read_u32(bytes, 116 + axis * 4);
        }
        Self {
            bytes: &bytes[STEP_HEADER_BYTES..],
            position: [read_f32(bytes, 8), read_f32(bytes, 12), read_f32(bytes, 16)],
            velocity: [
                read_f32(bytes, 20),
                read_f32(bytes, 24),
                read_f32(bytes, 28),
            ],
            on_ground: bytes[32] == 1,
            jump: bytes[33] == 1,
            move_x: bytes[34] as i8,
            move_z: bytes[35] as i8,
            yaw_sin: read_f32(bytes, 36),
            yaw_cos: read_f32(bytes, 40),
            fixed_delta_seconds: read_f32(bytes, 44),
            step_height: tunables[0],
            walk_speed: tunables[1],
            ground_acceleration: tunables[2],
            ground_deceleration: tunables[3],
            air_acceleration: tunables[4],
            jump_speed: tunables[5],
            gravity: tunables[6],
            terminal_fall_speed: tunables[7],
            body_in_fluid: bytes[128] == 1,
            sprinting: bytes[129] == 1,
            sneaking: bytes[130] == 1,
            fluid_gravity: read_f32(bytes, 132),
            fluid_sink_speed: read_f32(bytes, 136),
            fluid_ascend_speed: read_f32(bytes, 140),
            fluid_horizontal_drag: read_f32(bytes, 144),
            sprint_speed_multiplier: read_f32(bytes, 148),
            sneak_speed_multiplier: read_f32(bytes, 152),
            sweep_min,
            sweep_max,
            origin,
            dimensions,
        }
    }
}

pub(crate) fn step_input_is_valid(bytes: &[u8]) -> bool {
    if bytes.len() < STEP_HEADER_BYTES
        || &bytes[0..4] != b"MGP1"
        || read_u32(bytes, 4) != STEP_LAYOUT_VERSION
        || bytes[32] > 1
        || bytes[33] > 1
        || bytes[128] > 1
        || bytes[129] > 1
        || bytes[130] > 1
        // v4 保留区：131 与 156..160 必须为 0，129/130 为疾跑/潜行位、
        // 148..156 为两档倍率已在别处校验。
        || bytes[131] != 0
        || bytes[156..STEP_HEADER_BYTES].iter().any(|&byte| byte != 0)
        || !(-1..=1).contains(&(bytes[34] as i8))
        || !(-1..=1).contains(&(bytes[35] as i8))
    {
        return false;
    }
    // position/velocity（8..32）、yaw_sin/yaw_cos/dt（36/40/44）、tunables 与 sweep bounds（48..104）与水中/疾跑/潜行倍率必须全部有限
    for offset in (8..32)
        .step_by(4)
        .chain((36..=44).step_by(4))
        .chain((48..104).step_by(4))
        .chain((132..156).step_by(4))
    {
        if !read_f32(bytes, offset).is_finite() {
            return false;
        }
    }
    for axis in 0..3 {
        if read_f32(bytes, 80 + axis * 8) > read_f32(bytes, 84 + axis * 8) {
            return false;
        }
    }
    let mut cell_count: usize = 1;
    for axis in 0..3 {
        let dimension = read_u32(bytes, 116 + axis * 4);
        let origin = read_i32(bytes, 104 + axis * 4);
        if dimension == 0 || origin.checked_add((dimension - 1) as i32).is_none() {
            return false;
        }
        let Some(next) = cell_count.checked_mul(dimension as usize) else {
            return false;
        };
        cell_count = next;
    }
    if cell_count > STEP_MAX_CELLS {
        return false;
    }
    let Some(expected_length) = STEP_HEADER_BYTES.checked_add(cell_count * CELL_BYTES) else {
        return false;
    };
    if expected_length != bytes.len() {
        return false;
    }
    for cell in bytes[STEP_HEADER_BYTES..].chunks_exact(CELL_BYTES) {
        if cell[0] > 1 || cell[1] > 8 || cell[2] != 0 || cell[3] != 0 {
            return false;
        }
        for box_index in 0..cell[1] as usize {
            let box_offset = 4 + box_index * 24;
            for component in 0..6 {
                if !read_f32(cell, box_offset + component * 4).is_finite() {
                    return false;
                }
            }
        }
    }
    true
}

type Vector = [f32; 3];

// Match Go's `stepVectorLength` across platforms: round the inner and outer
// fused sums to f32, take the square root in f64, then round back to f32.
// Explicit fusion prevents the sweep envelope from depending on Go's target
// architecture or its optimization of `mgl32.Vec3.Len`.
fn vec3_len(v: Vector) -> f32 {
    let sum = v[2].mul_add(v[2], v[0].mul_add(v[0], v[1] * v[1]));
    ((sum as f64).sqrt()) as f32
}

// Match Go's fixed-step normalization: divide once, then multiply each component.
fn vec3_normalize(v: Vector) -> Vector {
    let l = 1.0f32 / vec3_len(v);
    [v[0] * l, v[1] * l, v[2] * l]
}

fn vec3_scale(v: Vector, c: f32) -> Vector {
    [v[0] * c, v[1] * c, v[2] * c]
}

// 与 Go moveToward 逐位一致：delta = target−current；len <= max → target；
// 否则 current + delta*(max/len)。
fn move_toward(current: Vector, target: Vector, maximum_delta: f32) -> Vector {
    let delta = [
        target[0] - current[0],
        target[1] - current[1],
        target[2] - current[2],
    ];
    let length = vec3_len(delta);
    if length <= maximum_delta {
        return target;
    }
    let scale = maximum_delta / length;
    [
        current[0] + delta[0] * scale,
        current[1] + delta[1] * scale,
        current[2] + delta[2] * scale,
    ]
}

// 与 Go movementTarget 逐位一致（三角已由 Go 算好传入）：
// right.Mul(f32(MoveX)).Add(forward.Mul(f32(MoveZ)))，Normalize().Mul(walkSpeed)。
fn movement_target(move_x: i8, move_z: i8, walk_speed: f32, yaw_sin: f32, yaw_cos: f32) -> Vector {
    let forward = [-yaw_sin, 0.0, -yaw_cos];
    let right = [yaw_cos, 0.0, -yaw_sin];
    let intent = [
        right[0] * move_x as f32 + forward[0] * move_z as f32,
        right[1] * move_x as f32 + forward[1] * move_z as f32,
        right[2] * move_x as f32 + forward[2] * move_z as f32,
    ];
    if vec3_len(intent) == 0.0 {
        return [0.0; 3];
    }
    vec3_scale(vec3_normalize(intent), walk_speed)
}

// integrate 返回（积分后 velocity，displacement）。运算顺序逐条镜像 Go 旧 Step 实现。
pub(crate) struct IntegrationParams {
    pub(crate) velocity: [f32; 3],
    pub(crate) on_ground: bool,
    pub(crate) jump: bool,
    pub(crate) move_x: i8,
    pub(crate) move_z: i8,
    pub(crate) yaw_sin: f32,
    pub(crate) yaw_cos: f32,
    pub(crate) fixed_delta_seconds: f32,
    pub(crate) walk_speed: f32,
    pub(crate) ground_acceleration: f32,
    pub(crate) ground_deceleration: f32,
    pub(crate) air_acceleration: f32,
    pub(crate) jump_speed: f32,
    pub(crate) gravity: f32,
    pub(crate) terminal_fall_speed: f32,
    pub(crate) body_in_fluid: bool,
    pub(crate) sprinting: bool,
    pub(crate) sneaking: bool,
    pub(crate) fluid_gravity: f32,
    pub(crate) fluid_sink_speed: f32,
    pub(crate) fluid_ascend_speed: f32,
    pub(crate) fluid_horizontal_drag: f32,
    pub(crate) sprint_speed_multiplier: f32,
    pub(crate) sneak_speed_multiplier: f32,
}

pub(crate) fn integrate(input: &IntegrationParams) -> (Vector, Vector) {
    let dt = input.fixed_delta_seconds;
    let mut velocity = input.velocity;
    // 潜行优先于疾跑：同置时只减速不加速，与 Go stepSweepBounds 同序。
    let sneaking_effective = input.sneaking && input.on_ground && !input.body_in_fluid;
    let effective_walk_speed = if sneaking_effective {
        input.walk_speed * input.sneak_speed_multiplier
    } else if input.sprinting && input.move_z > 0 && input.on_ground && !input.body_in_fluid {
        input.walk_speed * input.sprint_speed_multiplier
    } else {
        input.walk_speed
    };
    let target = movement_target(
        input.move_x,
        input.move_z,
        effective_walk_speed,
        input.yaw_sin,
        input.yaw_cos,
    );
    let mut horizontal = [velocity[0], 0.0, velocity[2]];
    if input.on_ground {
        if vec3_len(target) == 0.0 {
            horizontal = move_toward(horizontal, [0.0; 3], input.ground_deceleration * dt);
        } else {
            horizontal = move_toward(horizontal, target, input.ground_acceleration * dt);
        }
    } else {
        horizontal = move_toward(horizontal, target, input.air_acceleration * dt);
        if vec3_len(horizontal) > input.walk_speed {
            horizontal = vec3_scale(vec3_normalize(horizontal), input.walk_speed);
        }
    }
    // 水中水平阻力乘在加速之后：每 tick 都把这一步的结果按系数缩一次，稳态因此
    // 收敛到 accel*dt*k/(1−k)（或加速已够到 walk_speed 时的 walk_speed*k），
    // 恒低于陆地行走速度。放在加速之前只会被同一步的 move_toward 拉回去，
    // 水陆速度将无差别。
    if input.body_in_fluid {
        horizontal = vec3_scale(horizontal, input.fluid_horizontal_drag);
    }
    velocity[0] = horizontal[0];
    velocity[2] = horizontal[2];
    // 垂直分支的优先级：水中上浮 > 地面起跳 > 重力。
    //
    // 为什么水中的 Jump 是持续上浮而不是跳跃冲量：冲量语义是「离地瞬间给一次
    // 初速度，之后交给重力」，它依赖 on_ground 这个只在触地那一 tick 成立的
    // 边沿。玩家浮在水中时 on_ground 恒为假，冲量分支永远不会再触发，按住上升
    // 键将毫无作用；即便强行去掉 on_ground 条件，冲量也会被随后的水中重力立刻
    // 吃掉，表现为一次抖动而不是上升。直接每 tick 把垂直速度赋成 fluid_ascend_speed
    // 才能给出「按住就一直升，松开就开始沉」的可控上浮，并保证一定能浮出水面。
    if input.body_in_fluid && input.jump {
        velocity[1] = input.fluid_ascend_speed;
    } else if input.on_ground && input.jump {
        velocity[1] = input.jump_speed;
    } else {
        let (gravity, terminal) = if input.body_in_fluid {
            (input.fluid_gravity, input.fluid_sink_speed)
        } else {
            (input.gravity, input.terminal_fall_speed)
        };
        // 与 Go 内建 max 逐位一致：f32::max 符号零语义为 +0 胜出。
        velocity[1] = (velocity[1] - gravity * dt).max(-terminal);
    }
    let displacement = [velocity[0] * dt, velocity[1] * dt, velocity[2] * dt];
    (velocity, displacement)
}

#[derive(Debug)]
pub(crate) enum StepError {
    DisplacementOutOfBounds,
}

fn write_f32_output(output: &mut [u8], offset: usize, value: f32) {
    output[offset..offset + 4].copy_from_slice(&value.to_bits().to_le_bytes());
}

// physics_step 一次完成积分 + 碰撞解析 + 速度裁剪。
// 积分位移必须落在输入 sweep bounds 内，否则返回 DisplacementOutOfBounds（调用方映射 StatusInput）。
pub(crate) fn physics_step(bytes: &[u8]) -> Result<[u8; STEP_OUTPUT_BYTES], StepError> {
    let input = StepInput::decode(bytes);
    let params = IntegrationParams {
        velocity: input.velocity,
        on_ground: input.on_ground,
        jump: input.jump,
        move_x: input.move_x,
        move_z: input.move_z,
        yaw_sin: input.yaw_sin,
        yaw_cos: input.yaw_cos,
        fixed_delta_seconds: input.fixed_delta_seconds,
        walk_speed: input.walk_speed,
        ground_acceleration: input.ground_acceleration,
        ground_deceleration: input.ground_deceleration,
        air_acceleration: input.air_acceleration,
        jump_speed: input.jump_speed,
        gravity: input.gravity,
        terminal_fall_speed: input.terminal_fall_speed,
        body_in_fluid: input.body_in_fluid,
        sprinting: input.sprinting,
        sneaking: input.sneaking,
        fluid_gravity: input.fluid_gravity,
        fluid_sink_speed: input.fluid_sink_speed,
        fluid_ascend_speed: input.fluid_ascend_speed,
        fluid_horizontal_drag: input.fluid_horizontal_drag,
        sprint_speed_multiplier: input.sprint_speed_multiplier,
        sneak_speed_multiplier: input.sneak_speed_multiplier,
    };
    let (velocity, displacement) = integrate(&params);
    // Go mirrors this integrator's fused vector length when encoding sweep bounds.
    // Keep the one-ULP allowance for residual rounding, but reject wider drift;
    // the prism's collision margin is a separate geometric safety bound.
    for ((&minimum, &maximum), &offset) in input
        .sweep_min
        .iter()
        .zip(&input.sweep_max)
        .zip(&displacement)
    {
        if !(minimum.next_down() <= offset && offset <= maximum.next_up()) {
            return Err(StepError::DisplacementOutOfBounds);
        }
    }
    let result = crate::collision::resolve_collision_parts(
        input.position,
        displacement,
        input.on_ground,
        input.step_height,
        input.origin,
        input.dimensions,
        input.bytes,
    );
    // result 布局沿用 collision ABI：position(0..12)、clipped mask(12)、on_ground(13)、used_step(14)、hit_unknown(15)
    let clipped = result[12];
    let mut velocity = velocity;
    for (axis, component) in velocity.iter_mut().enumerate() {
        if clipped & (1 << axis) != 0 {
            *component = 0.0;
        }
    }
    let mut output = [0u8; STEP_OUTPUT_BYTES];
    for axis in 0..3 {
        let result_component: [u8; 4] = result[axis * 4..axis * 4 + 4]
            .try_into()
            .expect("collision result 4 bytes");
        write_f32_output(&mut output, axis * 4, f32::from_le_bytes(result_component));
        write_f32_output(&mut output, 12 + axis * 4, velocity[axis]);
    }
    output[24] = clipped;
    output[25] = result[13];
    output[26] = result[14];
    output[27] = result[15];
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{
        STEP_HEADER_BYTES, STEP_LAYOUT_VERSION, STEP_OUTPUT_BYTES, StepError, StepInput, integrate,
        physics_step, IntegrationParams, read_f32, step_input_is_valid, vec3_len,
    };

    const CELL_BYTES: usize = 196;

    fn write_f32(bytes: &mut [u8], offset: usize, value: f32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_bits().to_le_bytes());
    }

    fn valid_step_bytes() -> Vec<u8> {
        let mut bytes = vec![0u8; STEP_HEADER_BYTES + CELL_BYTES];
        bytes[0..4].copy_from_slice(b"MGP1");
        bytes[4..8].copy_from_slice(&STEP_LAYOUT_VERSION.to_le_bytes());
        write_f32(&mut bytes, 8, 0.5); // position x
        write_f32(&mut bytes, 12, 1.0); // position y
        write_f32(&mut bytes, 16, 0.5); // position z
        write_f32(&mut bytes, 20, 0.0); // velocity x
        write_f32(&mut bytes, 24, -1.6); // velocity y
        write_f32(&mut bytes, 28, 0.0); // velocity z
        bytes[32] = 1; // on_ground
        bytes[34] = 1; // move_x
        write_f32(&mut bytes, 36, 0.0); // yaw_sin
        write_f32(&mut bytes, 40, 1.0); // yaw_cos
        write_f32(&mut bytes, 44, 0.05); // fixed_delta_seconds
        for (index, value) in [0.6f32, 4.3, 40.0, 50.0, 8.0, 8.4, 32.0, 78.4]
            .iter()
            .enumerate()
        {
            write_f32(&mut bytes, 48 + index * 4, *value);
        }
        write_f32(&mut bytes, 80, 0.0); // dx_min
        write_f32(&mut bytes, 84, 4.3 * 0.05); // dx_max
        write_f32(&mut bytes, 88, -1.6 * 0.05); // dy_min
        write_f32(&mut bytes, 92, 0.05); // dy_max
        write_f32(&mut bytes, 96, 0.0); // dz_min
        write_f32(&mut bytes, 100, 0.0); // dz_max
        for index in 0..3 {
            bytes[104 + index * 4..108 + index * 4].copy_from_slice(&0u32.to_le_bytes()); // origin
            bytes[116 + index * 4..120 + index * 4].copy_from_slice(&1u32.to_le_bytes()); // dimensions
        }
        // v2 新增区：默认给一组「水中」参数，浸没标志本身仍为 0（空气）。
        for (index, value) in [6.4f32, 3.0, 4.0, 0.8].iter().enumerate() {
            write_f32(&mut bytes, 132 + index * 4, *value);
        }
        bytes[STEP_HEADER_BYTES] = 1; // cell loaded
        bytes
    }

    #[test]
    fn accepts_valid_input() {
        assert!(step_input_is_valid(&valid_step_bytes()));
    }

    #[test]
    fn vec3_len_matches_go_fma() {
        // 该 delta 是 Task 8 差分语料中触发的真实输入：Go arm64 会把平方和
        // 收缩为 FMA 得到 0x40829268，而严格 IEEE 是 0x40829267。
        let delta = [f32::from_bits(0x3f048e4c), 0.0, f32::from_bits(0x4081842c)];
        assert_eq!(vec3_len(delta).to_bits(), 0x40829268);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = valid_step_bytes();
        bytes[0] = b'X';
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_bad_layout() {
        let mut bytes = valid_step_bytes();
        bytes[4] = 0;
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_move_out_of_range() {
        let mut bytes = valid_step_bytes();
        bytes[34] = 2; // move_x = 2
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_non_finite_tunable() {
        let mut bytes = valid_step_bytes();
        write_f32(&mut bytes, 48, f32::NAN);
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_non_finite_sweep_bounds() {
        let mut bytes = valid_step_bytes();
        write_f32(&mut bytes, 84, f32::INFINITY);
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_inverted_sweep_bounds() {
        let mut bytes = valid_step_bytes();
        write_f32(&mut bytes, 84, -1.0); // dx_max < dx_min
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_wrong_length() {
        let mut bytes = valid_step_bytes();
        bytes.pop();
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_too_many_cells() {
        let mut bytes = valid_step_bytes();
        // dimensions 33×8×16 = 4224 > 4096
        for (index, dimension) in [33u32, 8, 16].iter().enumerate() {
            bytes[116 + index * 4..120 + index * 4].copy_from_slice(&dimension.to_le_bytes());
        }
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_invalid_cell() {
        let mut bytes = valid_step_bytes();
        bytes[STEP_HEADER_BYTES + 1] = 9; // cell count > 8
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_on_ground_out_of_range() {
        let mut bytes = valid_step_bytes();
        bytes[32] = 2; // on_ground = 2
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_jump_out_of_range() {
        let mut bytes = valid_step_bytes();
        bytes[33] = 2; // jump = 2
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_move_z_out_of_range() {
        let mut bytes = valid_step_bytes();
        bytes[35] = 2; // move_z = 2
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_non_finite_position() {
        let mut bytes = valid_step_bytes();
        write_f32(&mut bytes, 8, f32::NAN); // position x
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_non_finite_velocity() {
        let mut bytes = valid_step_bytes();
        write_f32(&mut bytes, 20, f32::INFINITY); // velocity x
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_non_finite_yaw() {
        let mut bytes = valid_step_bytes();
        write_f32(&mut bytes, 36, f32::NAN); // yaw_sin
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_non_finite_delta() {
        let mut bytes = valid_step_bytes();
        write_f32(&mut bytes, 44, f32::INFINITY); // fixed_delta_seconds
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_zero_dimension() {
        let mut bytes = valid_step_bytes();
        bytes[116..120].copy_from_slice(&0u32.to_le_bytes()); // dimensions[0] = 0
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_origin_overflow() {
        let mut bytes = valid_step_bytes();
        bytes[104..108].copy_from_slice(&i32::MAX.to_le_bytes()); // origin[0] = i32::MAX
        bytes[116..120].copy_from_slice(&2u32.to_le_bytes()); // dimensions[0] = 2
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_unloaded_cell() {
        let mut bytes = valid_step_bytes();
        bytes[STEP_HEADER_BYTES] = 2; // cell loaded = 2
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_nonzero_cell_reserved() {
        let mut bytes = valid_step_bytes();
        bytes[STEP_HEADER_BYTES + 2] = 1; // cell reserved byte != 0
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_non_finite_box() {
        let mut bytes = valid_step_bytes();
        bytes[STEP_HEADER_BYTES + 1] = 1; // cell box count = 1
        write_f32(&mut bytes, STEP_HEADER_BYTES + 4, f32::NAN); // box[0] component 0
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn decodes_fields() {
        let bytes = valid_step_bytes();
        let input = StepInput::decode(&bytes);
        assert_eq!(input.position, [0.5, 1.0, 0.5]);
        assert_eq!(input.velocity, [0.0, -1.6, 0.0]);
        assert!(input.on_ground);
        assert!(!input.jump);
        assert_eq!(input.move_x, 1);
        assert_eq!(input.move_z, 0);
        assert_eq!(input.yaw_sin, 0.0);
        assert_eq!(input.yaw_cos, 1.0);
        assert_eq!(input.fixed_delta_seconds, 0.05);
        assert_eq!(input.step_height, 0.6);
        assert_eq!(input.dimensions, [1, 1, 1]);
    }

    #[test]
    fn diagonal_input_accelerates_without_boost() {
        let mut bytes = valid_step_bytes();
        bytes[35] = 1; // move_z = 1，真正的对角输入
        let input = StepInput::decode(Box::leak(bytes.into_boxed_slice()));
        let params = IntegrationParams {
        velocity: input.velocity,
        on_ground: input.on_ground,
        jump: input.jump,
        move_x: input.move_x,
        move_z: input.move_z,
        yaw_sin: input.yaw_sin,
        yaw_cos: input.yaw_cos,
        fixed_delta_seconds: input.fixed_delta_seconds,
        walk_speed: input.walk_speed,
        ground_acceleration: input.ground_acceleration,
        ground_deceleration: input.ground_deceleration,
        air_acceleration: input.air_acceleration,
        jump_speed: input.jump_speed,
        gravity: input.gravity,
        terminal_fall_speed: input.terminal_fall_speed,
        body_in_fluid: input.body_in_fluid,
        sprinting: input.sprinting,
        sneaking: input.sneaking,
        fluid_gravity: input.fluid_gravity,
        fluid_sink_speed: input.fluid_sink_speed,
        fluid_ascend_speed: input.fluid_ascend_speed,
        fluid_horizontal_drag: input.fluid_horizontal_drag,
        sprint_speed_multiplier: input.sprint_speed_multiplier,
        sneak_speed_multiplier: input.sneak_speed_multiplier,
    };
    let (velocity, _) = integrate(&params);
        let horizontal = (velocity[0] * velocity[0] + velocity[2] * velocity[2]).sqrt();
        assert!((horizontal - 2.0).abs() < 1e-5);
    }

    #[test]
    fn sneaking_slows_to_sneak_multiplier() {
        // 站立纯前移 + 潜行位：目标速度恰为 walk*0.3 且单步可达，
        // 积分后 z 速度与 -(4.3*0.3) 逐位一致。
        let mut bytes = valid_step_bytes();
        bytes[34] = 0; // move_x = 0，纯前移
        bytes[35] = 1; // move_z = 1
        bytes[130] = 1; // sneaking
        write_f32(&mut bytes, 152, 0.3); // sneak_speed_multiplier
        let input = StepInput::decode(Box::leak(bytes.into_boxed_slice()));
        let params = IntegrationParams {
        velocity: input.velocity,
        on_ground: input.on_ground,
        jump: input.jump,
        move_x: input.move_x,
        move_z: input.move_z,
        yaw_sin: input.yaw_sin,
        yaw_cos: input.yaw_cos,
        fixed_delta_seconds: input.fixed_delta_seconds,
        walk_speed: input.walk_speed,
        ground_acceleration: input.ground_acceleration,
        ground_deceleration: input.ground_deceleration,
        air_acceleration: input.air_acceleration,
        jump_speed: input.jump_speed,
        gravity: input.gravity,
        terminal_fall_speed: input.terminal_fall_speed,
        body_in_fluid: input.body_in_fluid,
        sprinting: input.sprinting,
        sneaking: input.sneaking,
        fluid_gravity: input.fluid_gravity,
        fluid_sink_speed: input.fluid_sink_speed,
        fluid_ascend_speed: input.fluid_ascend_speed,
        fluid_horizontal_drag: input.fluid_horizontal_drag,
        sprint_speed_multiplier: input.sprint_speed_multiplier,
        sneak_speed_multiplier: input.sneak_speed_multiplier,
    };
    let (velocity, _) = integrate(&params);
        assert_eq!(velocity[2].to_bits(), (-(4.3f32 * 0.3f32)).to_bits());
    }

    #[test]
    fn sneaking_takes_priority_over_sprinting() {
        // 潜行 + 疾跑同置：只减速不加速，结果与纯潜行逐位一致。
        let mut bytes = valid_step_bytes();
        bytes[34] = 0; // move_x = 0，纯前移
        bytes[35] = 1; // move_z = 1
        bytes[129] = 1; // sprinting
        bytes[130] = 1; // sneaking
        write_f32(&mut bytes, 148, 1.3); // sprint_speed_multiplier
        write_f32(&mut bytes, 152, 0.3); // sneak_speed_multiplier
        let input = StepInput::decode(Box::leak(bytes.into_boxed_slice()));
        let params = IntegrationParams {
        velocity: input.velocity,
        on_ground: input.on_ground,
        jump: input.jump,
        move_x: input.move_x,
        move_z: input.move_z,
        yaw_sin: input.yaw_sin,
        yaw_cos: input.yaw_cos,
        fixed_delta_seconds: input.fixed_delta_seconds,
        walk_speed: input.walk_speed,
        ground_acceleration: input.ground_acceleration,
        ground_deceleration: input.ground_deceleration,
        air_acceleration: input.air_acceleration,
        jump_speed: input.jump_speed,
        gravity: input.gravity,
        terminal_fall_speed: input.terminal_fall_speed,
        body_in_fluid: input.body_in_fluid,
        sprinting: input.sprinting,
        sneaking: input.sneaking,
        fluid_gravity: input.fluid_gravity,
        fluid_sink_speed: input.fluid_sink_speed,
        fluid_ascend_speed: input.fluid_ascend_speed,
        fluid_horizontal_drag: input.fluid_horizontal_drag,
        sprint_speed_multiplier: input.sprint_speed_multiplier,
        sneak_speed_multiplier: input.sneak_speed_multiplier,
    };
    let (velocity, _) = integrate(&params);
        assert_eq!(velocity[2].to_bits(), (-(4.3f32 * 0.3f32)).to_bits());
    }

    #[test]
    fn accepts_sneak_bit_and_multiplier() {
        let mut bytes = valid_step_bytes();
        bytes[130] = 1; // sneaking
        write_f32(&mut bytes, 152, 0.3); // sneak_speed_multiplier
        assert!(step_input_is_valid(&bytes));
    }

    #[test]
    fn rejects_nonzero_sneak_tail_reserved() {
        let mut bytes = valid_step_bytes();
        bytes[131] = 1; // v4 保留字节必须为 0
        assert!(!step_input_is_valid(&bytes));
    }

    #[test]
    fn jump_uses_jump_speed() {
        let mut bytes = valid_step_bytes();
        bytes[33] = 1; // jump
        let input = StepInput::decode(Box::leak(bytes.into_boxed_slice()));
        let params = IntegrationParams {
        velocity: input.velocity,
        on_ground: input.on_ground,
        jump: input.jump,
        move_x: input.move_x,
        move_z: input.move_z,
        yaw_sin: input.yaw_sin,
        yaw_cos: input.yaw_cos,
        fixed_delta_seconds: input.fixed_delta_seconds,
        walk_speed: input.walk_speed,
        ground_acceleration: input.ground_acceleration,
        ground_deceleration: input.ground_deceleration,
        air_acceleration: input.air_acceleration,
        jump_speed: input.jump_speed,
        gravity: input.gravity,
        terminal_fall_speed: input.terminal_fall_speed,
        body_in_fluid: input.body_in_fluid,
        sprinting: input.sprinting,
        sneaking: input.sneaking,
        fluid_gravity: input.fluid_gravity,
        fluid_sink_speed: input.fluid_sink_speed,
        fluid_ascend_speed: input.fluid_ascend_speed,
        fluid_horizontal_drag: input.fluid_horizontal_drag,
        sprint_speed_multiplier: input.sprint_speed_multiplier,
        sneak_speed_multiplier: input.sneak_speed_multiplier,
    };
    let (velocity, _) = integrate(&params);
        assert_eq!(velocity[1].to_bits(), 8.4f32.to_bits());
    }

    #[test]
    fn gravity_clamps_to_terminal() {
        let mut bytes = valid_step_bytes();
        bytes[32] = 0; // 空中
        write_f32(&mut bytes, 24, -78.0);
        let input = StepInput::decode(Box::leak(bytes.into_boxed_slice()));
        let params = IntegrationParams {
        velocity: input.velocity,
        on_ground: input.on_ground,
        jump: input.jump,
        move_x: input.move_x,
        move_z: input.move_z,
        yaw_sin: input.yaw_sin,
        yaw_cos: input.yaw_cos,
        fixed_delta_seconds: input.fixed_delta_seconds,
        walk_speed: input.walk_speed,
        ground_acceleration: input.ground_acceleration,
        ground_deceleration: input.ground_deceleration,
        air_acceleration: input.air_acceleration,
        jump_speed: input.jump_speed,
        gravity: input.gravity,
        terminal_fall_speed: input.terminal_fall_speed,
        body_in_fluid: input.body_in_fluid,
        sprinting: input.sprinting,
        sneaking: input.sneaking,
        fluid_gravity: input.fluid_gravity,
        fluid_sink_speed: input.fluid_sink_speed,
        fluid_ascend_speed: input.fluid_ascend_speed,
        fluid_horizontal_drag: input.fluid_horizontal_drag,
        sprint_speed_multiplier: input.sprint_speed_multiplier,
        sneak_speed_multiplier: input.sneak_speed_multiplier,
    };
    let (velocity, _) = integrate(&params);
        assert_eq!(velocity[1].to_bits(), (-78.4f32).to_bits());
    }

    #[test]
    fn zero_input_on_ground_decelerates() {
        let mut bytes = valid_step_bytes();
        bytes[34] = 0; // move_x = 0
        write_f32(&mut bytes, 20, 10.0); // velocity x = 10
        let input = StepInput::decode(Box::leak(bytes.into_boxed_slice()));
        let params = IntegrationParams {
        velocity: input.velocity,
        on_ground: input.on_ground,
        jump: input.jump,
        move_x: input.move_x,
        move_z: input.move_z,
        yaw_sin: input.yaw_sin,
        yaw_cos: input.yaw_cos,
        fixed_delta_seconds: input.fixed_delta_seconds,
        walk_speed: input.walk_speed,
        ground_acceleration: input.ground_acceleration,
        ground_deceleration: input.ground_deceleration,
        air_acceleration: input.air_acceleration,
        jump_speed: input.jump_speed,
        gravity: input.gravity,
        terminal_fall_speed: input.terminal_fall_speed,
        body_in_fluid: input.body_in_fluid,
        sprinting: input.sprinting,
        sneaking: input.sneaking,
        fluid_gravity: input.fluid_gravity,
        fluid_sink_speed: input.fluid_sink_speed,
        fluid_ascend_speed: input.fluid_ascend_speed,
        fluid_horizontal_drag: input.fluid_horizontal_drag,
        sprint_speed_multiplier: input.sprint_speed_multiplier,
        sneak_speed_multiplier: input.sneak_speed_multiplier,
    };
    let (velocity, _) = integrate(&params);
        assert_eq!(velocity[0].to_bits(), 7.5f32.to_bits()); // 10 − 50*0.05
    }

    // empty_prism_bytes 构造"空中 + 空世界"夹具：position {0.5,1,0.5}、velocity 0、
    // on_ground=0、move_x=0、sweep dy=[-0.08,0.05]、prism {1,4,1} 全空 cell。
    // 积分结果：velocity {0,-1.6,0}，displacement {0,-0.08,0}，位置落到 y=0.92。
    fn empty_prism_bytes() -> Vec<u8> {
        let dimensions = [1u32, 4, 1];
        let cell_count = (dimensions[0] * dimensions[1] * dimensions[2]) as usize;
        let mut bytes = vec![0u8; STEP_HEADER_BYTES + cell_count * CELL_BYTES];
        bytes[0..4].copy_from_slice(b"MGP1");
        bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        write_f32(&mut bytes, 8, 0.5);
        write_f32(&mut bytes, 12, 1.0);
        write_f32(&mut bytes, 16, 0.5);
        // velocity 保持 0
        bytes[32] = 0; // 空中
        // move_x = 0
        write_f32(&mut bytes, 36, 0.0); // yaw_sin
        write_f32(&mut bytes, 40, 1.0); // yaw_cos
        write_f32(&mut bytes, 44, 0.05); // dt
        for (index, value) in [0.6f32, 4.3, 40.0, 50.0, 8.0, 8.4, 32.0, 78.4]
            .iter()
            .enumerate()
        {
            write_f32(&mut bytes, 48 + index * 4, *value);
        }
        write_f32(&mut bytes, 80, 0.0); // dx_min
        write_f32(&mut bytes, 84, 0.0); // dx_max
        write_f32(&mut bytes, 88, -1.6 * 0.05); // dy_min（与积分 dy 逐位一致）
        write_f32(&mut bytes, 92, 0.05); // dy_max
        write_f32(&mut bytes, 96, 0.0); // dz_min
        write_f32(&mut bytes, 100, 0.0); // dz_max
        for index in 0..3 {
            bytes[104 + index * 4..108 + index * 4].copy_from_slice(&0u32.to_le_bytes());
            bytes[116 + index * 4..120 + index * 4]
                .copy_from_slice(&dimensions[index].to_le_bytes());
        }
        for cell in bytes[STEP_HEADER_BYTES..].chunks_exact_mut(CELL_BYTES) {
            cell[0] = 1; // loaded、count 0
        }
        bytes
    }

    #[test]
    fn physics_step_rejects_displacement_outside_sweep_bounds() {
        // 基础夹具 on_ground=1、velocity y=-1.6、sweep dy=[-0.08,0.05]，
        // 积分 dy = -0.16 越界 → DisplacementOutOfBounds。
        let input_bytes = Box::leak(valid_step_bytes().into_boxed_slice());
        assert!(matches!(
            physics_step(input_bytes),
            Err(StepError::DisplacementOutOfBounds)
        ));
    }

    #[test]
    fn physics_step_allows_one_ulp_outside_sweep_bounds() {
        // Accept exactly one ULP outside the supplied envelope as the frozen
        // ABI contract; explicit Go/Rust FMA parity does not widen this guard.
        let mut bytes = empty_prism_bytes();
        let displacement = {
            let input = StepInput::decode(&bytes);
            {
let params = IntegrationParams {
        velocity: input.velocity,
        on_ground: input.on_ground,
        jump: input.jump,
        move_x: input.move_x,
        move_z: input.move_z,
        yaw_sin: input.yaw_sin,
        yaw_cos: input.yaw_cos,
        fixed_delta_seconds: input.fixed_delta_seconds,
        walk_speed: input.walk_speed,
        ground_acceleration: input.ground_acceleration,
        ground_deceleration: input.ground_deceleration,
        air_acceleration: input.air_acceleration,
        jump_speed: input.jump_speed,
        gravity: input.gravity,
        terminal_fall_speed: input.terminal_fall_speed,
        body_in_fluid: input.body_in_fluid,
        sprinting: input.sprinting,
        sneaking: input.sneaking,
        fluid_gravity: input.fluid_gravity,
        fluid_sink_speed: input.fluid_sink_speed,
        fluid_ascend_speed: input.fluid_ascend_speed,
        fluid_horizontal_drag: input.fluid_horizontal_drag,
        sprint_speed_multiplier: input.sprint_speed_multiplier,
        sneak_speed_multiplier: input.sneak_speed_multiplier,
    };
    integrate(&params).1
}
        };
        write_f32(&mut bytes, 88, displacement[1].next_down()); // dy_min 取更小确定值
        write_f32(&mut bytes, 92, displacement[1].next_down()); // dy_max = 位移 − 1 ulp
        let input_bytes = Box::leak(bytes.into_boxed_slice());
        assert!(physics_step(input_bytes).is_ok());
    }

    #[test]
    fn physics_step_rejects_two_ulp_outside_sweep_bounds() {
        // 位移为 sweep_max.next_up().next_up()（界外 2 ulp）→ DisplacementOutOfBounds。
        let mut bytes = empty_prism_bytes();
        let displacement = {
            let input = StepInput::decode(&bytes);
            {
let params = IntegrationParams {
        velocity: input.velocity,
        on_ground: input.on_ground,
        jump: input.jump,
        move_x: input.move_x,
        move_z: input.move_z,
        yaw_sin: input.yaw_sin,
        yaw_cos: input.yaw_cos,
        fixed_delta_seconds: input.fixed_delta_seconds,
        walk_speed: input.walk_speed,
        ground_acceleration: input.ground_acceleration,
        ground_deceleration: input.ground_deceleration,
        air_acceleration: input.air_acceleration,
        jump_speed: input.jump_speed,
        gravity: input.gravity,
        terminal_fall_speed: input.terminal_fall_speed,
        body_in_fluid: input.body_in_fluid,
        sprinting: input.sprinting,
        sneaking: input.sneaking,
        fluid_gravity: input.fluid_gravity,
        fluid_sink_speed: input.fluid_sink_speed,
        fluid_ascend_speed: input.fluid_ascend_speed,
        fluid_horizontal_drag: input.fluid_horizontal_drag,
        sprint_speed_multiplier: input.sprint_speed_multiplier,
        sneak_speed_multiplier: input.sneak_speed_multiplier,
    };
    integrate(&params).1
}
        };
        write_f32(&mut bytes, 88, displacement[1].next_down()); // dy_min 取更小确定值
        write_f32(&mut bytes, 92, displacement[1].next_down().next_down()); // dy_max = 位移 − 2 ulp
        let input_bytes = Box::leak(bytes.into_boxed_slice());
        assert!(matches!(
            physics_step(input_bytes),
            Err(StepError::DisplacementOutOfBounds)
        ));
    }

    #[test]
    fn physics_step_encodes_output_layout() {
        let bytes = Box::leak(empty_prism_bytes().into_boxed_slice());
        let output = physics_step(bytes).expect("valid input");
        assert_eq!(output.len(), STEP_OUTPUT_BYTES);
        assert_eq!(read_f32(&output, 0), 0.5); // position x
        assert_eq!(read_f32(&output, 4), 0.92); // position y = 1 − 0.08
        assert_eq!(read_f32(&output, 8), 0.5); // position z
        assert_eq!(read_f32(&output, 12), 0.0); // velocity x
        assert_eq!(read_f32(&output, 16), -1.6); // velocity y（重力 −1.6）
        assert_eq!(read_f32(&output, 20), 0.0); // velocity z
        assert_eq!(output[24], 0); // clipped mask
        assert_eq!(output[25], 0); // on_ground（0.92 处无支撑）
        assert_eq!(output[26], 0); // used_step
        assert_eq!(output[27], 0); // hit_unknown
        assert_eq!(&output[28..32], &[0, 0, 0, 0]); // reserved
    }

    #[test]
    fn physics_step_lands_on_floor_and_clips_velocity() {
        let mut bytes = empty_prism_bytes();
        // 地板：cell (0,0,0)（prism Y/X/Z 顺序的第一个 cell）全立方
        bytes[STEP_HEADER_BYTES + 1] = 1;
        let box_offset = STEP_HEADER_BYTES + 4;
        write_f32(&mut bytes, box_offset, 0.0);
        write_f32(&mut bytes, box_offset + 4, 0.0);
        write_f32(&mut bytes, box_offset + 8, 0.0);
        write_f32(&mut bytes, box_offset + 12, 1.0);
        write_f32(&mut bytes, box_offset + 16, 1.0);
        write_f32(&mut bytes, box_offset + 20, 1.0);
        let input_bytes = Box::leak(bytes.into_boxed_slice());
        let output = physics_step(input_bytes).expect("valid input");
        assert_eq!(read_f32(&output, 4), 1.0); // position y 落回 1.0
        assert_eq!(read_f32(&output, 16), 0.0); // velocity y 被裁剪清零
        assert_eq!(output[25], 1); // on_ground
    }
}
