use mornlea_engine::native::collision::resolve_collision;
use mornlea_engine::native::contracts::collision::{
    CollisionCell, CollisionGrid, CollisionRequest,
};

#[test]
fn parity_test() {
    let cells = [CollisionCell::default(); 4];
    let grid = CollisionGrid::try_new([0, -1, 0], [1, 4, 1], &cells).unwrap();

    let req = CollisionRequest {
        position: [0.5, 0.5, 0.5],
        displacement: [0.0, -0.5, 0.0],
        began_grounded: false,
        step_height: 0.6,
        grid,
    };

    let res = resolve_collision(&req).unwrap();
    assert_eq!(res.position[1].to_bits(), 0.0f32.to_bits());
    assert!(res.hit_unknown);
}
