const HEADER_BYTES: usize = 64;
const CELL_BYTES: usize = 196;
const PLAYER_WIDTH: f32 = 0.6;
const PLAYER_HEIGHT: f32 = 1.8;
const EPSILON: f32 = 1e-5;
const GROUND_PROBE: f32 = 1e-4;

pub(crate) const COLLISION_STEP_HEIGHT_OFFSET: usize = 36;

type Vector = [f32; 3];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Bounds {
    pub minimum: Vector,
    pub maximum: Vector,
}

#[derive(Clone, Copy, Debug, Default)]
struct MoveResult {
    position: Vector,
    clipped: [bool; 3],
    on_ground: bool,
    hit_unknown: bool,
}

struct CollisionInput<'a> {
    bytes: &'a [u8],
    position: Vector,
    displacement: Vector,
    began_grounded: bool,
    step_height: f32,
    origin: [i32; 3],
    dimensions: [u32; 3],
}

pub(crate) trait CollisionCells {
    /// Native views admit each actual read prism before traversal. Legacy byte
    /// callers keep their established admission and panic-to-status boundary.
    fn scan_is_admitted(&self, _minimum: [f32; 3], _maximum: [f32; 3]) -> bool {
        true
    }

    fn loaded(&self, position: [i32; 3]) -> bool;
    fn count(&self, position: [i32; 3]) -> usize;
    fn bounds(&self, position: [i32; 3], index: usize) -> Bounds;
    fn unknown_bounds(&self, position: [i32; 3]) -> Bounds {
        block_bounds(
            position,
            Bounds {
                minimum: [0.0; 3],
                maximum: [1.0; 3],
            },
        )
    }
}

pub(crate) struct CollisionInputData<'a, C: CollisionCells> {
    pub(crate) cells: &'a C,
    pub(crate) position: Vector,
    pub(crate) displacement: Vector,
    pub(crate) began_grounded: bool,
    pub(crate) step_height: f32,
}

pub(crate) struct FinalResult {
    pub position: Vector,
    pub clipped: [bool; 3],
    pub on_ground: bool,
    pub used_step: bool,
    pub hit_unknown: bool,
}
#[derive(Clone, Copy)]
struct Cell<'a> {
    bytes: &'a [u8],
    position: [i32; 3],
}

impl<'a> CollisionInput<'a> {
    fn decode(bytes: &'a [u8]) -> Self {
        let position = [read_f32(bytes, 8), read_f32(bytes, 12), read_f32(bytes, 16)];
        let displacement = [
            read_f32(bytes, 20),
            read_f32(bytes, 24),
            read_f32(bytes, 28),
        ];
        let began_grounded = bytes[32] == 1;
        let step_height = read_f32(bytes, COLLISION_STEP_HEIGHT_OFFSET);
        let origin = [
            read_i32(bytes, 40),
            read_i32(bytes, 44),
            read_i32(bytes, 48),
        ];
        let dimensions = [
            read_u32(bytes, 52),
            read_u32(bytes, 56),
            read_u32(bytes, 60),
        ];
        Self::from_parts(
            position,
            displacement,
            began_grounded,
            step_height,
            origin,
            dimensions,
            &bytes[HEADER_BYTES..],
        )
    }

    fn from_parts(
        position: Vector,
        displacement: Vector,
        began_grounded: bool,
        step_height: f32,
        origin: [i32; 3],
        dimensions: [u32; 3],
        cells: &'a [u8],
    ) -> Self {
        Self {
            bytes: cells,
            position,
            displacement,
            began_grounded,
            step_height,
            origin,
            dimensions,
        }
    }

    fn cell(&self, position: [i32; 3]) -> Cell<'a> {
        let x = i64::from(position[0]) - i64::from(self.origin[0]);
        let y = i64::from(position[1]) - i64::from(self.origin[1]);
        let z = i64::from(position[2]) - i64::from(self.origin[2]);
        debug_assert!(x >= 0 && x < i64::from(self.dimensions[0]));
        debug_assert!(y >= 0 && y < i64::from(self.dimensions[1]));
        debug_assert!(z >= 0 && z < i64::from(self.dimensions[2]));
        let index = ((y as usize * self.dimensions[0] as usize) + x as usize)
            * self.dimensions[2] as usize
            + z as usize;
        let offset = index * CELL_BYTES;
        Cell {
            bytes: &self.bytes[offset..offset + CELL_BYTES],
            position,
        }
    }
}

impl<'a> CollisionCells for CollisionInput<'a> {
    fn loaded(&self, position: [i32; 3]) -> bool {
        self.cell(position).loaded()
    }
    fn count(&self, position: [i32; 3]) -> usize {
        self.cell(position).count()
    }
    fn bounds(&self, position: [i32; 3], index: usize) -> Bounds {
        self.cell(position).bounds(index)
    }
    fn unknown_bounds(&self, position: [i32; 3]) -> Bounds {
        self.cell(position).unknown_bounds()
    }
}

impl Cell<'_> {
    fn loaded(&self) -> bool {
        self.bytes[0] == 1
    }

    fn count(&self) -> usize {
        self.bytes[1] as usize
    }

    fn bounds(&self, index: usize) -> Bounds {
        let offset = 4 + index * 24;
        let local = Bounds {
            minimum: [
                read_f32(self.bytes, offset),
                read_f32(self.bytes, offset + 4),
                read_f32(self.bytes, offset + 8),
            ],
            maximum: [
                read_f32(self.bytes, offset + 12),
                read_f32(self.bytes, offset + 16),
                read_f32(self.bytes, offset + 20),
            ],
        };
        block_bounds(self.position, local)
    }

    fn unknown_bounds(&self) -> Bounds {
        block_bounds(
            self.position,
            Bounds {
                minimum: [0.0; 3],
                maximum: [1.0; 3],
            },
        )
    }
}

pub(crate) fn resolve_collision(bytes: &[u8]) -> [u8; 16] {
    let input = CollisionInput::decode(bytes);
    resolve_collision_input(input)
}

// 供 Task 5 的 step.rs 零拷贝复用 cells。
pub(crate) fn resolve_collision_parts(
    position: Vector,
    displacement: Vector,
    began_grounded: bool,
    step_height: f32,
    origin: [i32; 3],
    dimensions: [u32; 3],
    cells: &[u8],
) -> [u8; 16] {
    let input = CollisionInput::from_parts(
        position,
        displacement,
        began_grounded,
        step_height,
        origin,
        dimensions,
        cells,
    );
    resolve_collision_input(input)
}

fn resolve_collision_input(input: CollisionInput<'_>) -> [u8; 16] {
    let data = CollisionInputData {
        cells: &input,
        position: input.position,
        displacement: input.displacement,
        began_grounded: input.began_grounded,
        step_height: input.step_height,
    };
    let result = resolve_move_and_step(&data);
    encode_result(
        MoveResult {
            position: result.position,
            clipped: result.clipped,
            on_ground: result.on_ground,
            hit_unknown: result.hit_unknown,
        },
        result.used_step,
    )
}

pub(crate) fn resolve_move_and_step<C: CollisionCells>(
    input: &CollisionInputData<'_, C>,
) -> FinalResult {
    let mut ordinary = resolve_move(input);
    let mut used_step = false;
    if (ordinary.clipped[0] || ordinary.clipped[2])
        && (input.began_grounded || ordinary.on_ground)
        && (input.displacement[0] != 0.0 || input.displacement[2] != 0.0)
    {
        let (stepped, accepted) = resolve_step_move(input);
        if accepted
            && horizontal_distance_squared(input.position, stepped.position)
                > horizontal_distance_squared(input.position, ordinary.position)
        {
            ordinary = stepped;
            used_step = true;
        }
    }
    FinalResult {
        position: ordinary.position,
        clipped: ordinary.clipped,
        on_ground: ordinary.on_ground,
        used_step,
        hit_unknown: ordinary.hit_unknown,
    }
}

fn resolve_move<C: CollisionCells>(input: &CollisionInputData<'_, C>) -> MoveResult {
    let mut result = MoveResult {
        position: input.position,
        ..MoveResult::default()
    };
    for axis in [1, 0, 2] {
        let (moved, clipped, hit_unknown) =
            clip_axis(input, result.position, axis, input.displacement[axis]);
        result.position[axis] += moved;
        result.clipped[axis] = clipped;
        result.hit_unknown |= hit_unknown;
        if axis == 1 && clipped && input.displacement[axis] < 0.0 {
            result.on_ground = true;
        }
    }

    let (_, supported, hit_unknown) = clip_axis(input, result.position, 1, -GROUND_PROBE);
    result.on_ground = supported;
    result.hit_unknown |= hit_unknown;
    result
}

fn clip_axis<C: CollisionCells>(
    input: &CollisionInputData<'_, C>,
    feet_position: Vector,
    axis: usize,
    requested: f32,
) -> (f32, bool, bool) {
    if requested == 0.0 {
        return (0.0, false, false);
    }

    let player = player_bounds(feet_position);
    let mut minimum = player.minimum;
    let mut maximum = player.maximum;
    if requested < 0.0 {
        minimum[axis] += requested;
    } else {
        maximum[axis] += requested;
    }

    let minimum = minimum.map(|value| value - EPSILON);
    let maximum = maximum.map(|value| value + EPSILON);
    if !input.cells.scan_is_admitted(minimum, maximum) {
        // A rejected native scan must not read cells. Its wrapper discards this
        // blocked placeholder and returns the recorded typed admission error.
        return (0.0, true, false);
    }
    let (minimum_x, maximum_x) = block_range(minimum[0], maximum[0]);
    let (minimum_y, maximum_y) = block_range(minimum[1], maximum[1]);
    let (minimum_z, maximum_z) = block_range(minimum[2], maximum[2]);
    let mut moved = requested;
    let mut was_clipped = false;
    let mut hit_unknown = false;
    for y in i64::from(minimum_y)..=i64::from(maximum_y) {
        for x in i64::from(minimum_x)..=i64::from(maximum_x) {
            for z in i64::from(minimum_z)..=i64::from(maximum_z) {
                let pos = [x as i32, y as i32, z as i32];
                if !input.cells.loaded(pos) {
                    let (candidate, blocks) = clip_against(
                        feet_position,
                        player,
                        axis,
                        moved,
                        input.cells.unknown_bounds(pos),
                    );
                    if blocks {
                        hit_unknown = true;
                        moved = candidate;
                        was_clipped = true;
                    }
                    continue;
                }
                for index in 0..input.cells.count(pos) {
                    let (candidate, blocks) = clip_against(
                        feet_position,
                        player,
                        axis,
                        moved,
                        input.cells.bounds(pos, index),
                    );
                    if blocks {
                        moved = candidate;
                        was_clipped = true;
                    }
                }
            }
        }
    }
    (moved, was_clipped, hit_unknown)
}

fn block_bounds(position: [i32; 3], local: Bounds) -> Bounds {
    let offset = [position[0] as f32, position[1] as f32, position[2] as f32];
    Bounds {
        minimum: [
            local.minimum[0] + offset[0],
            local.minimum[1] + offset[1],
            local.minimum[2] + offset[2],
        ],
        maximum: [
            local.maximum[0] + offset[0],
            local.maximum[1] + offset[1],
            local.maximum[2] + offset[2],
        ],
    }
}

fn clip_against(
    position: Vector,
    player: Bounds,
    axis: usize,
    requested: f32,
    collider: Bounds,
) -> (f32, bool) {
    if !overlaps_other_axes(player, collider, axis)
        || !endpoint_touches_collider(position, collider, axis, requested)
    {
        return (requested, false);
    }

    if requested > 0.0 {
        let distance = collider.minimum[axis] - player.maximum[axis];
        if distance >= -EPSILON && distance <= requested + EPSILON {
            let candidate = distance.min(requested);
            return (
                safe_collision_distance(position, collider, axis, requested, candidate),
                true,
            );
        }
        return (requested, false);
    }

    let distance = collider.maximum[axis] - player.minimum[axis];
    if distance <= EPSILON && distance >= requested - EPSILON {
        let candidate = distance.max(requested);
        return (
            safe_collision_distance(position, collider, axis, requested, candidate),
            true,
        );
    }
    (requested, false)
}

fn endpoint_touches_collider(
    mut position: Vector,
    collider: Bounds,
    axis: usize,
    requested: f32,
) -> bool {
    position[axis] += requested;
    let player = player_bounds(position);
    if requested > 0.0 {
        player.maximum[axis] >= collider.minimum[axis]
    } else {
        player.minimum[axis] <= collider.maximum[axis]
    }
}

fn safe_collision_distance(
    position: Vector,
    collider: Bounds,
    axis: usize,
    requested: f32,
    mut distance: f32,
) -> f32 {
    loop {
        let mut final_position = position;
        final_position[axis] += distance;
        let final_bounds = player_bounds(final_position);
        if requested > 0.0 {
            if final_bounds.maximum[axis] <= collider.minimum[axis] {
                return distance;
            }
            distance = next_after(distance, f32::NEG_INFINITY);
            continue;
        }
        if final_bounds.minimum[axis] >= collider.maximum[axis] {
            return distance;
        }
        distance = next_after(distance, f32::INFINITY);
    }
}

fn overlaps_other_axes(left: Bounds, right: Bounds, axis: usize) -> bool {
    for other in 0..3 {
        if other == axis {
            continue;
        }
        if left.minimum[other] >= right.maximum[other]
            || left.maximum[other] <= right.minimum[other]
        {
            return false;
        }
    }
    true
}

fn bounds_are_collision_free<C: CollisionCells>(
    input: &CollisionInputData<'_, C>,
    position: Vector,
) -> (bool, bool) {
    let player = player_bounds(position);
    if !input.cells.scan_is_admitted(player.minimum, player.maximum) {
        return (false, false);
    }
    let (minimum_x, maximum_x) = block_range(player.minimum[0], player.maximum[0]);
    let (minimum_y, maximum_y) = block_range(player.minimum[1], player.maximum[1]);
    let (minimum_z, maximum_z) = block_range(player.minimum[2], player.maximum[2]);
    for y in i64::from(minimum_y)..=i64::from(maximum_y) {
        for x in i64::from(minimum_x)..=i64::from(maximum_x) {
            for z in i64::from(minimum_z)..=i64::from(maximum_z) {
                let pos = [x as i32, y as i32, z as i32];
                if !input.cells.loaded(pos) {
                    return (false, true);
                }
                for index in 0..input.cells.count(pos) {
                    if bounds_overlap(player, input.cells.bounds(pos, index)) {
                        return (false, false);
                    }
                }
            }
        }
    }
    (true, false)
}

fn bounds_overlap(left: Bounds, right: Bounds) -> bool {
    left.minimum[0] < right.maximum[0]
        && left.maximum[0] > right.minimum[0]
        && left.minimum[1] < right.maximum[1]
        && left.maximum[1] > right.minimum[1]
        && left.minimum[2] < right.maximum[2]
        && left.maximum[2] > right.minimum[2]
}

fn block_range(minimum: f32, maximum: f32) -> (i32, i32) {
    (minimum.floor() as i32, maximum.floor() as i32)
}

fn horizontal_distance_squared(from: Vector, to: Vector) -> f32 {
    let delta_x = to[0] - from[0];
    let delta_z = to[2] - from[2];
    delta_x * delta_x + delta_z * delta_z
}

fn resolve_step_move<C: CollisionCells>(input: &CollisionInputData<'_, C>) -> (MoveResult, bool) {
    let mut result = MoveResult {
        position: input.position,
        ..MoveResult::default()
    };
    let (rise, rise_clipped, rise_unknown) =
        clip_axis(input, result.position, 1, input.step_height);
    result.position[1] += rise;
    result.clipped[1] = rise_clipped;
    result.hit_unknown = rise_unknown;
    if result.hit_unknown {
        return (result, false);
    }

    for axis in [0, 2] {
        let (moved, clipped, hit_unknown) =
            clip_axis(input, result.position, axis, input.displacement[axis]);
        result.position[axis] += moved;
        result.clipped[axis] = clipped;
        result.hit_unknown |= hit_unknown;
    }
    if result.hit_unknown {
        return (result, false);
    }

    let down = -(rise + 0_f32.max(-input.displacement[1]));
    let (moved, clipped, hit_unknown) = clip_axis(input, result.position, 1, down);
    result.position[1] += moved;
    result.clipped[1] |= clipped;
    result.hit_unknown |= hit_unknown;
    if result.hit_unknown {
        return (result, false);
    }

    let (_, on_ground, hit_unknown) = clip_axis(input, result.position, 1, -GROUND_PROBE);
    result.on_ground = on_ground;
    result.hit_unknown |= hit_unknown;
    if result.hit_unknown || !result.on_ground {
        return (result, false);
    }
    let (clear, hit_unknown) = bounds_are_collision_free(input, result.position);
    if !clear || hit_unknown {
        result.hit_unknown |= hit_unknown;
        return (result, false);
    }
    (result, true)
}

fn player_bounds(position: Vector) -> Bounds {
    let half_width = PLAYER_WIDTH / 2.0;
    Bounds {
        minimum: [
            position[0] - half_width,
            position[1],
            position[2] - half_width,
        ],
        maximum: [
            position[0] + half_width,
            position[1] + PLAYER_HEIGHT,
            position[2] + half_width,
        ],
    }
}

fn next_after(value: f32, toward: f32) -> f32 {
    if value == toward {
        return toward;
    }
    if value == 0.0 {
        return if toward > 0.0 {
            f32::from_bits(1)
        } else {
            f32::from_bits(0x8000_0001)
        };
    }
    let bits = value.to_bits();
    if (toward > value) == (value > 0.0) {
        f32::from_bits(bits + 1)
    } else {
        f32::from_bits(bits - 1)
    }
}

fn encode_result(result: MoveResult, used_step: bool) -> [u8; 16] {
    let mut output = [0_u8; 16];
    for (axis, value) in result.position.into_iter().enumerate() {
        output[axis * 4..axis * 4 + 4].copy_from_slice(&value.to_bits().to_le_bytes());
    }
    for (axis, clipped) in result.clipped.into_iter().enumerate() {
        if clipped {
            output[12] |= 1 << axis;
        }
    }
    output[13] = u8::from(result.on_ground);
    output[14] = u8::from(used_step);
    output[15] = u8::from(result.hit_unknown);
    output
}

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

#[cfg(test)]
mod tests {
    use super::{CELL_BYTES, HEADER_BYTES, resolve_collision};

    const TEST_CELLS: usize = 64;

    struct TestInput {
        bytes: [u8; HEADER_BYTES + TEST_CELLS * CELL_BYTES],
        length: usize,
        origin: [i32; 3],
        dimensions: [u32; 3],
    }

    impl TestInput {
        fn new(
            position: [f32; 3],
            displacement: [f32; 3],
            began_grounded: bool,
            step_height: f32,
            origin: [i32; 3],
            dimensions: [u32; 3],
        ) -> Self {
            let cells = dimensions
                .into_iter()
                .map(|value| value as usize)
                .product::<usize>();
            assert!(cells <= TEST_CELLS);
            let mut input = Self {
                bytes: [0; HEADER_BYTES + TEST_CELLS * CELL_BYTES],
                length: HEADER_BYTES + cells * CELL_BYTES,
                origin,
                dimensions,
            };
            input.bytes[0..4].copy_from_slice(b"MGC1");
            input.bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
            for (index, value) in position.into_iter().enumerate() {
                input.put_f32(8 + index * 4, value);
            }
            for (index, value) in displacement.into_iter().enumerate() {
                input.put_f32(20 + index * 4, value);
            }
            input.bytes[32] = u8::from(began_grounded);
            input.put_f32(36, step_height);
            for (index, value) in origin.into_iter().enumerate() {
                input.bytes[40 + index * 4..44 + index * 4].copy_from_slice(&value.to_le_bytes());
            }
            for (index, value) in dimensions.into_iter().enumerate() {
                input.bytes[52 + index * 4..56 + index * 4].copy_from_slice(&value.to_le_bytes());
            }
            for cell in input.bytes[HEADER_BYTES..input.length].chunks_exact_mut(CELL_BYTES) {
                cell[0] = 1;
            }
            input
        }

        fn set_full_cube(&mut self, position: [i32; 3], loaded: bool) {
            let offset = self.cell_offset(position);
            self.bytes[offset] = u8::from(loaded);
            self.bytes[offset + 1] = u8::from(loaded);
            if loaded {
                for (index, value) in [0.0_f32, 0.0, 0.0, 1.0, 1.0, 1.0].into_iter().enumerate() {
                    self.put_f32(offset + 4 + index * 4, value);
                }
            }
        }

        fn set_half_block(&mut self, position: [i32; 3]) {
            let offset = self.cell_offset(position);
            self.bytes[offset] = 1;
            self.bytes[offset + 1] = 1;
            for (index, value) in [0.0_f32, 0.0, 0.0, 1.0, 0.5, 1.0].into_iter().enumerate() {
                self.put_f32(offset + 4 + index * 4, value);
            }
        }

        fn set_unknown(&mut self, position: [i32; 3]) {
            let offset = self.cell_offset(position);
            self.bytes[offset] = 0;
            self.bytes[offset + 1] = 0;
        }

        fn as_slice(&self) -> &[u8] {
            &self.bytes[..self.length]
        }

        fn cell_offset(&self, position: [i32; 3]) -> usize {
            let x = (position[0] - self.origin[0]) as usize;
            let y = (position[1] - self.origin[1]) as usize;
            let z = (position[2] - self.origin[2]) as usize;
            HEADER_BYTES
                + ((y * self.dimensions[0] as usize + x) * self.dimensions[2] as usize + z)
                    * CELL_BYTES
        }

        fn put_f32(&mut self, offset: usize, value: f32) {
            self.bytes[offset..offset + 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
    }

    struct RejectingScan {
        calls: std::cell::Cell<usize>,
    }

    impl super::CollisionCells for RejectingScan {
        fn scan_is_admitted(&self, _minimum: [f32; 3], _maximum: [f32; 3]) -> bool {
            self.calls.set(self.calls.get() + 1);
            false
        }
        fn loaded(&self, _position: [i32; 3]) -> bool {
            panic!("rejected scan read a cell")
        }
        fn count(&self, _position: [i32; 3]) -> usize {
            panic!("rejected scan read a box count")
        }
        fn bounds(&self, _position: [i32; 3], _index: usize) -> super::Bounds {
            panic!("rejected scan read bounds")
        }
    }

    #[test]
    fn collision_rejected_scans_never_enter_cell_loops() {
        let cells = RejectingScan {
            calls: std::cell::Cell::new(0),
        };
        let input = super::CollisionInputData {
            cells: &cells,
            position: [0.5, 1.0, 0.5],
            displacement: [1.0, -1.0, 1.0],
            began_grounded: true,
            step_height: 0.6,
        };
        let _ = super::resolve_move_and_step(&input);
        assert!(
            cells.calls.get() > 0 && cells.calls.get() <= 10,
            "solver scan work remains fixed after rejection"
        );
        assert_eq!(
            super::bounds_are_collision_free(&input, input.position),
            (false, false)
        );
    }

    #[test]
    fn collision_resolves_in_y_x_z_order() {
        let mut input = TestInput::new(
            [0.5, 1.2, 0.5],
            [0.5, -0.5, 0.5],
            true,
            0.6,
            [0, 0, 0],
            [2, 4, 2],
        );
        input.set_full_cube([0, 0, 0], true);
        input.set_full_cube([1, 1, 0], true);
        input.set_full_cube([0, 1, 1], true);

        let output = resolve_collision(input.as_slice());

        assert_eq!(
            u32::from_le_bytes(output[0..4].try_into().unwrap()),
            0.7_f32.to_bits()
        );
        assert_eq!(
            u32::from_le_bytes(output[4..8].try_into().unwrap()),
            1.0_f32.to_bits()
        );
        assert_eq!(
            u32::from_le_bytes(output[8..12].try_into().unwrap()),
            0.7_f32.to_bits()
        );
        assert_eq!(&output[12..16], &[7, 1, 0, 0]);
    }

    #[test]
    fn collision_treats_unknown_as_closed() {
        let mut input = TestInput::new(
            [0.5, 1.0, 0.5],
            [0.5, 0.0, 0.0],
            true,
            0.6,
            [0, 0, 0],
            [2, 4, 1],
        );
        input.set_unknown([1, 1, 0]);

        let output = resolve_collision(input.as_slice());

        assert_eq!(
            u32::from_le_bytes(output[0..4].try_into().unwrap()),
            0.7_f32.to_bits()
        );
        assert_eq!(output[12] & 1, 1);
        assert_eq!(output[15], 1);
    }

    #[test]
    fn rejected_step_unknown_does_not_reach_selected_result() {
        let mut input = TestInput::new(
            [0.5, 1.0, 0.5],
            [0.5, 0.0, 0.0],
            true,
            0.6,
            [0, 0, 0],
            [2, 4, 1],
        );
        input.set_full_cube([0, 0, 0], true);
        input.set_full_cube([1, 0, 0], true);
        input.set_half_block([1, 1, 0]);
        input.set_unknown([0, 3, 0]);

        let output = resolve_collision(input.as_slice());

        assert_eq!(output[14], 0);
        assert_eq!(output[15], 0);
    }

    #[test]
    fn next_after_matches_ieee_neighbor_bits() {
        assert_eq!(
            super::next_after(1.0, f32::NEG_INFINITY).to_bits(),
            0x3f7f_ffff
        );
        assert_eq!(super::next_after(1.0, f32::INFINITY).to_bits(), 0x3f80_0001);
        assert_eq!(
            super::next_after(0.0, f32::NEG_INFINITY).to_bits(),
            0x8000_0001
        );
    }
}
