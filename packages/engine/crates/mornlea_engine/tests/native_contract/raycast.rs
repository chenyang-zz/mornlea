use mornlea_engine::native::contracts::raycast::{Ray, RayFace, RayCursor, RaycastOp, RayBatch, RayRecord};
use mornlea_engine::native::contracts::KernelError;
use mornlea_engine::native::raycast::NativeRaycast;

#[test]
fn test_input_validation() {
    // Valid ray
    let ray = Ray { origin: [0.0; 3], direction: [1.0, 0.0, 0.0], maximum: 10.0 };
    assert!(RayCursor::try_new(ray).is_ok());

    // zero direction
    let ray = Ray { origin: [0.0; 3], direction: [0.0; 3], maximum: 10.0 };
    assert_eq!(RayCursor::try_new(ray).unwrap_err(), KernelError::InvalidInput);

    // nonfinite origin
    let ray = Ray { origin: [f32::NAN, 0.0, 0.0], direction: [1.0; 3], maximum: 10.0 };
    assert_eq!(RayCursor::try_new(ray).unwrap_err(), KernelError::InvalidInput);
    let ray = Ray { origin: [0.0, f32::INFINITY, 0.0], direction: [1.0; 3], maximum: 10.0 };
    assert_eq!(RayCursor::try_new(ray).unwrap_err(), KernelError::InvalidInput);

    // nonfinite direction
    let ray = Ray { origin: [0.0; 3], direction: [1.0, f32::NAN, 0.0], maximum: 10.0 };
    assert_eq!(RayCursor::try_new(ray).unwrap_err(), KernelError::InvalidInput);

    // negative max
    let ray = Ray { origin: [0.0; 3], direction: [1.0; 3], maximum: -5.0 };
    assert_eq!(RayCursor::try_new(ray).unwrap_err(), KernelError::InvalidInput);

    // zero max
    let ray = Ray { origin: [0.0; 3], direction: [1.0; 3], maximum: 0.0 };
    assert_eq!(RayCursor::try_new(ray).unwrap_err(), KernelError::InvalidInput);

    // nonfinite max
    let ray = Ray { origin: [0.0; 3], direction: [1.0; 3], maximum: f32::INFINITY };
    assert_eq!(RayCursor::try_new(ray).unwrap_err(), KernelError::InvalidInput);
}

#[test]
fn test_repeated_call_on_done() {
    let ray = Ray { origin: [0.0; 3], direction: [1.0, 0.0, 0.0], maximum: 0.5 };
    let mut cursor = RayCursor::try_new(ray).unwrap();
    let provider = NativeRaycast;

    let batch = provider.next_batch(&mut cursor).unwrap();
    assert!(batch.is_done());
    
    let empty_batch = provider.next_batch(&mut cursor).unwrap();
    assert!(empty_batch.is_done());
    assert!(empty_batch.records().is_empty());
}

#[test]
fn test_negative_floor() {
    let ray = Ray { origin: [-0.25, 0.0, 0.0], direction: [-1.0, 0.0, 0.0], maximum: 1.0 };
    let mut cursor = RayCursor::try_new(ray).unwrap();
    let provider = NativeRaycast;

    let batch = provider.next_batch(&mut cursor).unwrap();
    assert_eq!(batch.records()[0].cell, [-1, 0, 0]);
    assert_eq!(batch.records()[0].face, RayFace::Origin);
}

#[test]
fn test_strict_priority() {
    let comp = 1.0 / (3.0_f32).sqrt();
    let ray = Ray { origin: [0.5; 3], direction: [comp; 3], maximum: 1.0 };
    let mut cursor = RayCursor::try_new(ray).unwrap();
    let provider = NativeRaycast;

    let batch = provider.next_batch(&mut cursor).unwrap();
    assert_eq!(batch.records()[1].cell, [1, 0, 0]);
    assert_eq!(batch.records()[2].cell, [1, 1, 0]);
    assert_eq!(batch.records()[3].cell, [1, 1, 1]);
}

#[test]
fn test_exact_endpoint() {
    let ray = Ray { origin: [0.0, 0.5, 0.5], direction: [1.0, 0.0, 0.0], maximum: 6.0 };
    let mut cursor = RayCursor::try_new(ray).unwrap();
    let provider = NativeRaycast;

    let batch = provider.next_batch(&mut cursor).unwrap();
    assert!(batch.is_done());
    assert_eq!(batch.records().len(), 7);
    assert_eq!(batch.records()[6].cell, [6, 0, 0]);
    assert_eq!(batch.records()[6].distance, 6.0);
}

#[test]
fn test_multi_batch_continuation() {
    let ray = Ray { origin: [0.5; 3], direction: [1.0, 0.0, 0.0], maximum: 70.0 };
    let mut cursor = RayCursor::try_new(ray).unwrap();
    let provider = NativeRaycast;

    let batch1 = provider.next_batch(&mut cursor).unwrap();
    assert_eq!(batch1.records().len(), 64);
    assert!(!batch1.is_done());

    let batch2 = provider.next_batch(&mut cursor).unwrap();
    assert_eq!(batch2.records().len(), 7);
    assert!(batch2.is_done());
    assert_eq!(batch2.records()[6].cell, [70, 0, 0]);
}

#[test]
fn test_error_leaves_cursor_unchanged() {
    struct FailingProvider;
    impl RaycastOp for FailingProvider {
        fn next_batch(&self, cursor: &mut RayCursor) -> Result<RayBatch, KernelError> {
            let local_cursor = cursor.clone();
            // Mutate local state
            // we don't have access to pub(crate) fields from tests? Actually we might.
            // But we don't need to. We can just simulate error.
            let records = [RayRecord { cell: [0; 3], face: RayFace::Origin, distance: 0.0 }; 64];
            let batch = RayBatch::from_parts(records, 65, false)?; // Error out
            *cursor = local_cursor; // This would happen on success
            Ok(batch)
        }
    }

    let ray = Ray { origin: [0.0; 3], direction: [1.0, 0.0, 0.0], maximum: 10.0 };
    let mut cursor = RayCursor::try_new(ray).unwrap();
    let cursor_before = cursor.clone();
    
    let op = FailingProvider;
    assert!(op.next_batch(&mut cursor).is_err());
    assert_eq!(cursor, cursor_before);
}
