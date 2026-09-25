use mornlea_engine::native::collision::{NativeCollision, resolve_collision};
use mornlea_engine::native::contracts::KernelError;
use mornlea_engine::native::contracts::collision::{
    Aabb, CollisionCell, CollisionGrid, CollisionOp, CollisionRequest,
};

#[test]
fn collision_floor_wall_unknown_bits() {
    let cells = [
        CollisionCell::try_new(
            true,
            [
                Aabb {
                    minimum: [0.0, 0.0, 0.0],
                    maximum: [1.0, 1.0, 1.0],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
            ],
            1,
        )
        .unwrap(),
        CollisionCell::try_new(
            true,
            [
                Aabb {
                    minimum: [0.0, 0.0, 0.0],
                    maximum: [1.0, 1.0, 1.0],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
                Aabb {
                    minimum: [0.0; 3],
                    maximum: [0.0; 3],
                },
            ],
            1,
        )
        .unwrap(),
        CollisionCell::try_new(
            false,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            }; 8],
            0,
        )
        .unwrap(),
        CollisionCell::try_new(
            false,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            }; 8],
            0,
        )
        .unwrap(),
        CollisionCell::try_new(
            false,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            }; 8],
            0,
        )
        .unwrap(),
        CollisionCell::try_new(
            false,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            }; 8],
            0,
        )
        .unwrap(),
        CollisionCell::try_new(
            false,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            }; 8],
            0,
        )
        .unwrap(),
        CollisionCell::try_new(
            false,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            }; 8],
            0,
        )
        .unwrap(),
    ];
    let grid = CollisionGrid::try_new([0, 0, 0], [2, 4, 1], &cells).unwrap();

    let req = CollisionRequest {
        position: [0.5, 1.0, 0.5],
        displacement: [-0.0, -0.0, -0.0],
        began_grounded: true,
        step_height: 0.6,
        grid,
    };
    let res = resolve_collision(&req).unwrap();
    assert_eq!(res.position[0].to_bits(), 0.5f32.to_bits());
    assert_eq!(res.position[1].to_bits(), 1.0f32.to_bits());
    assert_eq!(res.position[2].to_bits(), 0.5f32.to_bits());

    let op = NativeCollision;
    let _ = op.resolve(&req).unwrap();
}

#[test]
fn collision_swept_coverage() {
    let cells = [CollisionCell::try_new(
        true,
        [Aabb {
            minimum: [0.0; 3],
            maximum: [1.0; 3],
        }; 8],
        1,
    )
    .unwrap()];
    let grid = CollisionGrid::try_new([0, 0, 0], [1, 1, 1], &cells).unwrap();

    let req = CollisionRequest {
        position: [0.5, 0.5, 0.5],
        displacement: [1.0, 0.0, 0.0], // Moves to x=1.5, requiring cell [1,0,0]
        began_grounded: true,
        step_height: 0.6,
        grid,
    };

    assert_eq!(
        resolve_collision(&req),
        Err(KernelError::DisplacementOutOfBounds)
    );
}

#[test]
fn collision_grid_cell_counts() {
    let cells_4096 = vec![
        CollisionCell::try_new(
            false,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3]
            }; 8],
            0
        )
        .unwrap();
        4096
    ];
    assert!(CollisionGrid::try_new([0, 0, 0], [16, 16, 16], &cells_4096).is_ok());

    let cells_4097 = vec![
        CollisionCell::try_new(
            false,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3]
            }; 8],
            0
        )
        .unwrap();
        4097
    ];
    // This is already checked by CollisionGrid::try_new which is in contracts.rs (read-only).
    // Verify error on invalid length
    assert!(CollisionGrid::try_new([0, 0, 0], [16, 16, 16], &cells_4097).is_err());

    // Also if dimensions multiply to 4097
    assert!(CollisionGrid::try_new([0, 0, 0], [4097, 1, 1], &cells_4097).is_err());
}

#[test]
fn used_vs_unused_nan() {
    // Unused NaN
    let mut nan_box = [Aabb {
        minimum: [0.0; 3],
        maximum: [0.0; 3],
    }; 8];
    nan_box[1].minimum[0] = f32::NAN;
    assert!(CollisionCell::try_new(true, nan_box, 1).is_ok());

    // Used NaN
    assert!(CollisionCell::try_new(true, nan_box, 2).is_err());
}

#[test]
fn grid_zero_dimension_overflow() {
    let empty: [CollisionCell; 0] = [];
    assert!(CollisionGrid::try_new([0, 0, 0], [0, 1, 1], &empty).is_err());

    let cells = [CollisionCell::default(); 1];
    // Far overflow: orig + dim_minus_1 > i32::MAX
    assert!(CollisionGrid::try_new([i32::MAX, 0, 0], [2, 1, 1], &cells).is_err());
}

#[test]
fn negative_step_height_accepted() {
    let cells = [CollisionCell::default(); 4];
    let grid = CollisionGrid::try_new([0, -1, 0], [1, 4, 1], &cells).unwrap();
    let req = CollisionRequest {
        position: [0.5, 0.5, 0.5],
        displacement: [0.0, 0.0, 0.0],
        began_grounded: true,
        step_height: -0.5,
        grid,
    };
    // Should be OK because the bounding box coverage will encompass the negative step and fit in [0,-1,0] to [0,1,0]
    assert!(resolve_collision(&req).is_ok());
}

#[test]
fn alternate_step_fails_must_not_leak_hit_unknown() {
    let mut cells = vec![
        CollisionCell::try_new(
            true,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3]
            }; 8],
            0
        )
        .unwrap();
        8
    ];
    // [2, 4, 1] grid at [0,0,0]
    // 0,0,0
    cells[0] = CollisionCell::try_new(
        true,
        [
            Aabb {
                minimum: [0.0, 0.0, 0.0],
                maximum: [1.0, 1.0, 1.0],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
        ],
        1,
    )
    .unwrap();
    // 1,0,0
    cells[1] = CollisionCell::try_new(
        true,
        [
            Aabb {
                minimum: [0.0, 0.0, 0.0],
                maximum: [1.0, 1.0, 1.0],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
        ],
        1,
    )
    .unwrap();
    // 1,1,0
    cells[3] = CollisionCell::try_new(
        true,
        [
            Aabb {
                minimum: [0.0, 0.0, 0.0],
                maximum: [1.0, 0.5, 1.0],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
        ],
        1,
    )
    .unwrap();
    // 0,3,0
    cells[6] = CollisionCell::try_new(
        false,
        [Aabb {
            minimum: [0.0; 3],
            maximum: [0.0; 3],
        }; 8],
        0,
    )
    .unwrap();

    let grid = CollisionGrid::try_new([0, 0, 0], [2, 4, 1], &cells).unwrap();
    let req = CollisionRequest {
        position: [0.5, 1.0, 0.5],
        displacement: [0.5, 0.0, 0.0],
        began_grounded: true,
        step_height: 0.6,
        grid,
    };

    let res = resolve_collision(&req).unwrap();
    assert!(!res.used_step);
    assert!(!res.hit_unknown);
}
