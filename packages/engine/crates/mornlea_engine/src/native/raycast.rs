use crate::native::contracts::KernelError;
use crate::native::contracts::raycast::{RayBatch, RayCursor, RayFace, RayRecord, RaycastOp};
use crate::raycast::{RaycastCursor, RaycastInput, advance_raycast};

/// Stateless provider; the caller owns complete DDA continuation and a call
/// publishes its locally advanced cursor only after the full batch succeeds.
pub struct NativeRaycast;

impl RaycastOp for NativeRaycast {
    fn next_batch(&self, cursor: &mut RayCursor) -> Result<RayBatch, KernelError> {
        let mut local = cursor.clone();
        let fresh = local.continuation.is_none();
        let mut inner = local.continuation.take().unwrap_or_else(|| {
            RaycastCursor::start(&RaycastInput {
                origin: local.ray.origin,
                direction: local.ray.direction,
                maximum: local.ray.maximum,
            })
        });
        let mut records = [RayRecord {
            cell: [0; 3],
            face: RayFace::Origin,
            distance: 0.0,
        }; 64];
        let mut slot = 0;
        let mut invalid_face = false;
        let count = advance_raycast(
            &mut inner,
            local.ray.maximum,
            fresh,
            |cell, face, distance| {
                let face = match face {
                    255 => RayFace::Origin,
                    0 => RayFace::NegX,
                    1 => RayFace::PosX,
                    2 => RayFace::NegY,
                    3 => RayFace::PosY,
                    4 => RayFace::NegZ,
                    5 => RayFace::PosZ,
                    _ => {
                        invalid_face = true;
                        RayFace::Origin
                    }
                };
                records[slot] = RayRecord {
                    cell,
                    face,
                    distance,
                };
                slot += 1;
            },
        );
        // A core invariant failure cannot publish a fabricated face or advance
        // the caller's cursor; raw-byte validation remains in the ABI adapter.
        if invalid_face {
            return Err(KernelError::OutputInvariant);
        }
        let done = inner.state == 2;
        local.continuation = Some(inner);
        let batch = RayBatch::from_parts(records, count, done)?;
        *cursor = local;
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::contracts::raycast::Ray;

    #[test]
    fn raycast_late_batches_have_bounded_advance_work() {
        let mut cursor = RayCursor::try_new(Ray {
            origin: [0.5; 3],
            direction: [1.0, 0.0, 0.0],
            maximum: 100_000.0,
        })
        .unwrap();
        crate::raycast::work::take();
        for batch_index in 0..1024 {
            let batch = NativeRaycast.next_batch(&mut cursor).unwrap();
            let (starts, advances) = crate::raycast::work::take();
            assert_eq!(
                starts,
                usize::from(batch_index == 0),
                "continuation must retain its DDA state"
            );
            assert!(
                advances <= 64,
                "one batch performed {advances} advances at history batch {batch_index}"
            );
            assert_eq!(batch.records().len(), 64);
            assert_eq!(batch.records()[63].cell, [63 + batch_index * 64, 0, 0]);
            assert!(!batch.is_done());
        }
    }
}
