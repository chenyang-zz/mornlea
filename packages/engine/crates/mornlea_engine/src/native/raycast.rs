use crate::native::contracts::raycast::{RayBatch, RayCursor, RayFace, RayRecord, RaycastOp};
use crate::native::contracts::KernelError;
use crate::raycast::{RaycastCursor, RaycastInput};

pub struct NativeRaycast;

impl RaycastOp for NativeRaycast {
    fn next_batch(&self, cursor: &mut RayCursor) -> Result<RayBatch, KernelError> {
        let mut local_cursor = cursor.clone();

        if local_cursor.is_done() {
            let batch = RayBatch::from_parts(
                [RayRecord { cell: [0; 3], face: RayFace::Origin, distance: 0.0 }; 64],
                0,
                true
            )?;
            *cursor = local_cursor;
            return Ok(batch);
        }

        let input = RaycastInput {
            origin: local_cursor.ray().origin,
            direction: local_cursor.ray().direction,
            maximum: local_cursor.ray().maximum,
        };

        let mut inner = RaycastCursor::start(&input);
        
        if local_cursor.initialized {
            while inner.cell != local_cursor.current_cell {
                let mut axis = 0;
                if inner.maximum[1] < inner.maximum[axis] { axis = 1; }
                if inner.maximum[2] < inner.maximum[axis] { axis = 2; }
                
                inner.cell[axis] = inner.cell[axis].wrapping_add(inner.step[axis]);
                inner.maximum[axis] += inner.delta[axis];
            }
        }

        let mut records = [RayRecord { cell: [0; 3], face: RayFace::Origin, distance: 0.0 }; 64];
        let mut count = 0;

        if !local_cursor.initialized {
            records[count] = RayRecord {
                cell: inner.cell,
                face: RayFace::Origin,
                distance: 0.0,
            };
            count += 1;
            local_cursor.initialized = true;
        }

        while count < 64 {
            let mut axis = 0;
            if inner.maximum[1] < inner.maximum[axis] { axis = 1; }
            if inner.maximum[2] < inner.maximum[axis] { axis = 2; }
            
            let distance = inner.maximum[axis];
            if distance > input.maximum {
                local_cursor.done = true;
                break;
            }
            
            inner.cell[axis] = inner.cell[axis].wrapping_add(inner.step[axis]);
            inner.maximum[axis] += inner.delta[axis];
            
            let face = match (axis, inner.step[axis] > 0) {
                (0, true) => RayFace::NegX,
                (0, false) => RayFace::PosX,
                (1, true) => RayFace::NegY,
                (1, false) => RayFace::PosY,
                (2, true) => RayFace::NegZ,
                (2, false) => RayFace::PosZ,
                _ => unreachable!(),
            };

            records[count] = RayRecord {
                cell: inner.cell,
                face,
                distance,
            };
            count += 1;
        }

        local_cursor.current_cell = inner.cell;
        if count > 0 {
            local_cursor.distance = records[count - 1].distance;
        }

        let batch = RayBatch::from_parts(records, count, local_cursor.done)?;
        *cursor = local_cursor;

        Ok(batch)
    }
}
