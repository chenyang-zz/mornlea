pub(crate) const RAYCAST_INPUT_BYTES: usize = 40;
pub(crate) const RAYCAST_CURSOR_BYTES: usize = 64;
pub(crate) const RAYCAST_RECORD_BYTES: usize = 20;
pub(crate) const RAYCAST_RECORD_CAPACITY: usize = 64;
pub(crate) const RAYCAST_OUTPUT_BYTES: usize = RAYCAST_RECORD_BYTES * RAYCAST_RECORD_CAPACITY;

pub(crate) struct RaycastBatch {
    pub(crate) cursor: [u8; RAYCAST_CURSOR_BYTES],
    pub(crate) output: [u8; RAYCAST_OUTPUT_BYTES],
    pub(crate) count: usize,
    pub(crate) done: bool,
}

type Vector = [f32; 3];

pub(crate) struct RaycastInput {
    pub(crate) origin: Vector,
    pub(crate) direction: Vector,
    pub(crate) maximum: f32,
}

pub(crate) struct RaycastCursor {
    pub(crate) state: u8,
    pub(crate) cell: [i32; 3],
    pub(crate) step: [i32; 3],
    pub(crate) delta: Vector,
    pub(crate) maximum: Vector,
}

impl RaycastInput {
    fn decode(bytes: &[u8]) -> Self {
        Self {
            origin: [read_f32(bytes, 8), read_f32(bytes, 12), read_f32(bytes, 16)],
            direction: [
                read_f32(bytes, 20),
                read_f32(bytes, 24),
                read_f32(bytes, 28),
            ],
            maximum: read_f32(bytes, 32),
        }
    }
}

impl RaycastCursor {
    fn decode(bytes: &[u8]) -> Self {
        Self {
            state: bytes[8],
            cell: [
                read_i32(bytes, 12),
                read_i32(bytes, 16),
                read_i32(bytes, 20),
            ],
            step: [
                read_i32(bytes, 24),
                read_i32(bytes, 28),
                read_i32(bytes, 32),
            ],
            delta: [
                read_f32(bytes, 36),
                read_f32(bytes, 40),
                read_f32(bytes, 44),
            ],
            maximum: [
                read_f32(bytes, 48),
                read_f32(bytes, 52),
                read_f32(bytes, 56),
            ],
        }
    }

    pub(crate) fn start(input: &RaycastInput) -> Self {
        let mut cursor = Self {
            state: 1,
            cell: input.origin.map(floor_to_i32),
            step: [0; 3],
            delta: [0.0; 3],
            maximum: [0.0; 3],
        };
        for axis in 0..3 {
            let component = input.direction[axis];
            if component > 0.0 {
                cursor.step[axis] = 1;
                cursor.delta[axis] = 1.0 / component;
                let boundary = cursor.cell[axis].wrapping_add(1) as f32;
                cursor.maximum[axis] = (boundary - input.origin[axis]) / component;
            } else if component < 0.0 {
                cursor.step[axis] = -1;
                cursor.delta[axis] = -1.0 / component;
                let boundary = cursor.cell[axis] as f32;
                cursor.maximum[axis] = (boundary - input.origin[axis]) / component;
            } else {
                cursor.delta[axis] = f32::INFINITY;
                cursor.maximum[axis] = f32::INFINITY;
            }
        }
        cursor
    }

    fn encode(&self) -> [u8; RAYCAST_CURSOR_BYTES] {
        let mut bytes = [0_u8; RAYCAST_CURSOR_BYTES];
        bytes[0..4].copy_from_slice(b"MRC1");
        bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
        bytes[8] = self.state;
        for (axis, value) in self.cell.into_iter().enumerate() {
            bytes[12 + axis * 4..16 + axis * 4].copy_from_slice(&value.to_le_bytes());
        }
        for (axis, value) in self.step.into_iter().enumerate() {
            bytes[24 + axis * 4..28 + axis * 4].copy_from_slice(&value.to_le_bytes());
        }
        for (axis, value) in self.delta.into_iter().enumerate() {
            bytes[36 + axis * 4..40 + axis * 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        for (axis, value) in self.maximum.into_iter().enumerate() {
            bytes[48 + axis * 4..52 + axis * 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        bytes
    }
}

pub(crate) fn raycast_cursor_overflow_is_valid(
    input_bytes: &[u8],
    axis: usize,
    maximum: f32,
) -> bool {
    let input = RaycastInput::decode(input_bytes);
    let initial = RaycastCursor::start(&input);
    maximum == f32::NEG_INFINITY && initial.maximum[axis] == f32::NEG_INFINITY
        || maximum.is_nan()
            && initial.maximum[axis] == f32::NEG_INFINITY
            && initial.delta[axis] == f32::INFINITY
}

pub(crate) fn raycast_batch(input_bytes: &[u8], cursor_bytes: &[u8]) -> RaycastBatch {
    let input = RaycastInput::decode(input_bytes);
    let fresh = cursor_bytes[8] == 0;
    let mut cursor = if fresh {
        RaycastCursor::start(&input)
    } else {
        RaycastCursor::decode(cursor_bytes)
    };
    let mut output = [0_u8; RAYCAST_OUTPUT_BYTES];
    let mut count = 0;

    if cursor.state == 2 {
        return RaycastBatch {
            cursor: cursor.encode(),
            output,
            count,
            done: true,
        };
    }
    if fresh {
        write_record(&mut output, count, cursor.cell, 0xff, 0.0);
        count += 1;
    }
    while count < RAYCAST_RECORD_CAPACITY {
        let mut axis = 0;
        if cursor.maximum[1] < cursor.maximum[axis] {
            axis = 1;
        }
        if cursor.maximum[2] < cursor.maximum[axis] {
            axis = 2;
        }
        let distance = cursor.maximum[axis];
        if distance > input.maximum {
            cursor.state = 2;
            break;
        }
        cursor.cell[axis] = cursor.cell[axis].wrapping_add(cursor.step[axis]);
        cursor.maximum[axis] += cursor.delta[axis];
        write_record(
            &mut output,
            count,
            cursor.cell,
            entry_face(axis, cursor.step[axis]),
            distance,
        );
        count += 1;
    }
    RaycastBatch {
        cursor: cursor.encode(),
        output,
        count,
        done: cursor.state == 2,
    }
}

fn write_record(
    output: &mut [u8; RAYCAST_OUTPUT_BYTES],
    index: usize,
    block: [i32; 3],
    face: u8,
    distance: f32,
) {
    let offset = index * RAYCAST_RECORD_BYTES;
    for (axis, value) in block.into_iter().enumerate() {
        output[offset + axis * 4..offset + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    output[offset + 12] = face;
    output[offset + 16..offset + 20].copy_from_slice(&distance.to_bits().to_le_bytes());
}

fn entry_face(axis: usize, step: i32) -> u8 {
    if step > 0 {
        [0, 2, 4][axis]
    } else {
        [1, 3, 5][axis]
    }
}

fn floor_to_i32(value: f32) -> i32 {
    let floored = value.floor();
    if !(-2_147_483_648.0..2_147_483_648.0).contains(&floored) {
        i32::MIN
    } else {
        floored as i32
    }
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
    use super::{
        RAYCAST_CURSOR_BYTES, RAYCAST_INPUT_BYTES, RAYCAST_OUTPUT_BYTES, RAYCAST_RECORD_BYTES,
        RAYCAST_RECORD_CAPACITY, raycast_batch,
    };

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    fn read_i32(bytes: &[u8], offset: usize) -> i32 {
        i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn raycast_layout_v1_is_stable() {
        assert_eq!(RAYCAST_INPUT_BYTES, 40);
        assert_eq!(RAYCAST_CURSOR_BYTES, 64);
        assert_eq!(RAYCAST_RECORD_BYTES, 20);
        assert_eq!(RAYCAST_RECORD_CAPACITY, 64);
        assert_eq!(RAYCAST_OUTPUT_BYTES, 1280);

        let input = [
            0x4d, 0x47, 0x52, 0x31, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa0, 0x3f, 0x00, 0x00,
            0x20, 0xc0, 0x00, 0x00, 0x70, 0x40, 0x00, 0x00, 0x80, 0xbe, 0x00, 0x00, 0x00, 0x3f,
            0x00, 0x00, 0x40, 0xbf, 0x00, 0x00, 0xd0, 0x40, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(&input[0..4], b"MGR1");
        assert_eq!(read_u32(&input, 4), 1);
        assert_eq!(read_u32(&input, 8), 1.25_f32.to_bits());
        assert_eq!(read_u32(&input, 12), (-2.5_f32).to_bits());
        assert_eq!(read_u32(&input, 16), 3.75_f32.to_bits());
        assert_eq!(read_u32(&input, 20), (-0.25_f32).to_bits());
        assert_eq!(read_u32(&input, 24), 0.5_f32.to_bits());
        assert_eq!(read_u32(&input, 28), (-0.75_f32).to_bits());
        assert_eq!(read_u32(&input, 32), 6.5_f32.to_bits());
        assert_eq!(&input[36..40], &[0; 4]);

        let mut cursor = [0_u8; RAYCAST_CURSOR_BYTES];
        cursor[0..4].copy_from_slice(b"MRC1");
        cursor[4..8].copy_from_slice(&1_u32.to_le_bytes());
        cursor[8] = 1;
        for (index, value) in [-7_i32, 8, -9].into_iter().enumerate() {
            cursor[12 + index * 4..16 + index * 4].copy_from_slice(&value.to_le_bytes());
        }
        for (index, value) in [-1_i32, 0, 1].into_iter().enumerate() {
            cursor[24 + index * 4..28 + index * 4].copy_from_slice(&value.to_le_bytes());
        }
        for (index, value) in [4.0_f32, f32::INFINITY, 2.0].into_iter().enumerate() {
            cursor[36 + index * 4..40 + index * 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        for (index, value) in [-0.0_f32, f32::INFINITY, 1.25].into_iter().enumerate() {
            cursor[48 + index * 4..52 + index * 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        assert_eq!(&cursor[0..4], b"MRC1");
        assert_eq!(read_u32(&cursor, 4), 1);
        assert_eq!(&cursor[8..12], &[1, 0, 0, 0]);
        assert_eq!(
            [
                read_i32(&cursor, 12),
                read_i32(&cursor, 16),
                read_i32(&cursor, 20)
            ],
            [-7, 8, -9]
        );
        assert_eq!(
            [
                read_i32(&cursor, 24),
                read_i32(&cursor, 28),
                read_i32(&cursor, 32)
            ],
            [-1, 0, 1]
        );
        assert_eq!(read_u32(&cursor, 36), 4.0_f32.to_bits());
        assert_eq!(read_u32(&cursor, 40), f32::INFINITY.to_bits());
        assert_eq!(read_u32(&cursor, 44), 2.0_f32.to_bits());
        assert_eq!(read_u32(&cursor, 48), (-0.0_f32).to_bits());
        assert_eq!(read_u32(&cursor, 52), f32::INFINITY.to_bits());
        assert_eq!(read_u32(&cursor, 56), 1.25_f32.to_bits());
        assert_eq!(&cursor[60..64], &[0; 4]);

        let record = [
            0xf9, 0xff, 0xff, 0xff, 0x08, 0x00, 0x00, 0x00, 0xf7, 0xff, 0xff, 0xff, 0x03, 0x00,
            0x00, 0x00, 0x00, 0x00, 0xa0, 0x3f,
        ];
        assert_eq!(read_i32(&record, 0), -7);
        assert_eq!(read_i32(&record, 4), 8);
        assert_eq!(read_i32(&record, 8), -9);
        assert_eq!(&record[12..16], &[3, 0, 0, 0]);
        assert_eq!(read_u32(&record, 16), 1.25_f32.to_bits());
    }

    #[test]
    fn raycast_emits_origin_with_negative_floor() {
        let input = test_input([-0.25, -1.75, -2.5], [-1.0, 0.0, 0.0], 0.1);
        let cursor = fresh_cursor();

        let result = raycast_batch(&input, &cursor);

        assert_eq!(result.count, 1);
        assert!(result.done);
        assert_eq!(read_i32(&result.output, 0), -1);
        assert_eq!(read_i32(&result.output, 4), -2);
        assert_eq!(read_i32(&result.output, 8), -3);
        assert_eq!(result.output[12], 0xff);
        assert_eq!(read_u32(&result.output, 16), 0.0_f32.to_bits());
    }

    #[test]
    fn raycast_uses_strict_xyz_tie_priority() {
        let component = 1.0_f32 / 3.0_f32.sqrt();
        let input = test_input([0.5; 3], [component; 3], 1.0);
        let cursor = fresh_cursor();

        let result = raycast_batch(&input, &cursor);

        assert_eq!(result.count, 4);
        assert!(result.done);
        assert_eq!(record_block(&result.output, 0), [0, 0, 0]);
        assert_eq!(record_block(&result.output, 1), [1, 0, 0]);
        assert_eq!(record_block(&result.output, 2), [1, 1, 0]);
        assert_eq!(record_block(&result.output, 3), [1, 1, 1]);
        assert_eq!(
            [result.output[32], result.output[52], result.output[72]],
            [0, 2, 4]
        );
    }

    #[test]
    fn raycast_includes_exact_endpoint_and_rejects_next_float() {
        let cursor = fresh_cursor();
        let exact = raycast_batch(&test_input([0.0, 0.5, 0.5], [1.0, 0.0, 0.0], 6.0), &cursor);
        assert_eq!(exact.count, 7);
        assert!(exact.done);
        assert_eq!(record_block(&exact.output, 6), [6, 0, 0]);
        assert_eq!(
            read_u32(&exact.output, 6 * RAYCAST_RECORD_BYTES + 16),
            6.0_f32.to_bits()
        );

        let next = f32::from_bits(6.0_f32.to_bits() + 1);
        let outside = raycast_batch(
            &test_input([6.0 - next, 0.5, 0.5], [1.0, 0.0, 0.0], 6.0),
            &cursor,
        );
        assert!(outside.done);
        assert_ne!(record_block(&outside.output, outside.count - 1), [6, 0, 0]);
    }

    #[test]
    fn raycast_continues_after_sixty_four_records() {
        let input = test_input([0.5; 3], [1.0, 0.0, 0.0], 70.0);
        let first = raycast_batch(&input, &fresh_cursor());
        assert_eq!(first.count, 64);
        assert!(!first.done);
        assert_eq!(record_block(&first.output, 63), [63, 0, 0]);

        let second = raycast_batch(&input, &first.cursor);
        assert_eq!(second.count, 7);
        assert!(second.done);
        assert_eq!(record_block(&second.output, 6), [70, 0, 0]);
    }

    #[test]
    fn raycast_wraps_i32_cell_advance() {
        let input = test_input([i32::MIN as f32, 0.5, 0.5], [-1.0, 0.0, 0.0], 1.0);
        let result = raycast_batch(&input, &fresh_cursor());

        assert_eq!(record_block(&result.output, 0), [i32::MIN, 0, 0]);
        assert_eq!(record_block(&result.output, 1), [i32::MAX, 0, 0]);
        assert_eq!(
            read_u32(&result.output, RAYCAST_RECORD_BYTES + 16),
            (-0.0_f32).to_bits()
        );
    }

    fn test_input(
        origin: [f32; 3],
        direction: [f32; 3],
        maximum: f32,
    ) -> [u8; RAYCAST_INPUT_BYTES] {
        let mut input = [0_u8; RAYCAST_INPUT_BYTES];
        input[0..4].copy_from_slice(b"MGR1");
        input[4..8].copy_from_slice(&1_u32.to_le_bytes());
        for (index, value) in origin.into_iter().enumerate() {
            input[8 + index * 4..12 + index * 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        for (index, value) in direction.into_iter().enumerate() {
            input[20 + index * 4..24 + index * 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        input[32..36].copy_from_slice(&maximum.to_bits().to_le_bytes());
        input
    }

    fn fresh_cursor() -> [u8; RAYCAST_CURSOR_BYTES] {
        let mut cursor = [0_u8; RAYCAST_CURSOR_BYTES];
        cursor[0..4].copy_from_slice(b"MRC1");
        cursor[4..8].copy_from_slice(&1_u32.to_le_bytes());
        cursor
    }

    fn record_block(output: &[u8], index: usize) -> [i32; 3] {
        let offset = index * RAYCAST_RECORD_BYTES;
        [
            read_i32(output, offset),
            read_i32(output, offset + 4),
            read_i32(output, offset + 8),
        ]
    }
}
