use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;

#[test]
fn test_numerical_migration() {
    let ray = Ray {
        origin: [0.5, 0.5, 0.5],
        direction: [1.0, 0.0, 0.0],
        maximum: 70.0,
    };
    let mut cursor = RayCursor::try_new(ray).unwrap();
    let provider = NativeRaycast;

    let batch1 = provider.next_batch(&mut cursor).unwrap();
    let batch2 = provider.next_batch(&mut cursor).unwrap();

    assert_eq!(batch1.records().len(), 64);
    assert_eq!(batch2.records().len(), 7);

    // Exact parity check for specific records
    let r0 = batch1.records()[0];
    assert_eq!(r0.cell, [0, 0, 0]);
    assert_eq!(r0.distance.to_bits(), 0.0_f32.to_bits());

    let r1 = batch1.records()[1];
    assert_eq!(r1.cell, [1, 0, 0]);
    assert_eq!(r1.distance.to_bits(), 0.5_f32.to_bits());

    let r63 = batch1.records()[63];
    assert_eq!(r63.cell, [63, 0, 0]);
    assert_eq!(r63.distance.to_bits(), 62.5_f32.to_bits());

    let r64 = batch2.records()[0];
    assert_eq!(r64.cell, [64, 0, 0]);
    assert_eq!(r64.distance.to_bits(), 63.5_f32.to_bits());

    let r70 = batch2.records()[6];
    assert_eq!(r70.cell, [70, 0, 0]);
    assert_eq!(r70.distance.to_bits(), 69.5_f32.to_bits());

    assert!(batch2.is_done());
}
