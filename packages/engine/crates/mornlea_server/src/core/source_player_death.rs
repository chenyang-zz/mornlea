//! Fixed preparation for one source player death; scan ownership remains external.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SourceDeathReset {
    pub(crate) dimension: mornlea_domain::Dimension,
    pub(crate) candidate: Option<super::actor_placement::RestoreCandidate>,
}

pub(super) struct PreparedDeathInventory {
    pub(super) after: super::contracts::InventoryRecord,
    pub(super) drops: Vec<super::contracts::RuleEffect>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PreparedDeathBed {
    pub(super) respawn: Option<(mornlea_domain::Dimension, mornlea_domain::BlockPos)>,
    pub(super) candidate: Option<super::actor_placement::RestoreCandidate>,
}

/// Repack losslessly, then clear only slots accepted by one cumulative rehearsal.
/// The context checks the Ready cardinality before this bounded candidate walk.
pub(super) fn prepare_inventory(
    view: &super::state::AuthorityReadView<'_>,
    actor: super::contracts::ActorKey,
    dimension: mornlea_domain::Dimension,
    position: [f32; 3],
    pickup_delay: u8,
    before: super::contracts::InventoryRecord,
) -> Result<PreparedDeathInventory, super::contracts::ServerError> {
    let mut after = crate::rules::crafting::repack_all(before).ok_or(
        super::contracts::ServerError::Internal {
            invariant: "source player death crafting",
        },
    )?;
    let mut drops = Vec::new();
    if let Some(block) = crate::rules::hostile_outcomes::death_block(position) {
        let mut rehearsal = view.drop_rehearsal();
        for candidate in crate::rules::hostile_outcomes::death_candidates(view, dimension, block) {
            for stack in after.slots.iter_mut().chain(after.armor.iter_mut()) {
                crate::rules::hostile_outcomes::drop_player_stack(
                    &mut rehearsal,
                    actor,
                    view.tick(),
                    dimension,
                    block,
                    candidate,
                    stack,
                    pickup_delay,
                    &mut drops,
                );
            }
        }
    }
    Ok(PreparedDeathInventory { after, drops })
}

/// Preserve unavailable live beds, clear proven invalid pairs, and defer geometry.
/// The placement boundary supplies outside-height AIR before horizontal readiness.
pub(super) fn prepare_bed(
    view: &super::state::AuthorityReadView<'_>,
    dimension: mornlea_domain::Dimension,
    respawn: Option<(mornlea_domain::Dimension, mornlea_domain::BlockPos)>,
) -> Result<PreparedDeathBed, super::contracts::ServerError> {
    let retained = PreparedDeathBed {
        respawn,
        candidate: None,
    };
    let Some((bed_dimension, foot)) = respawn else {
        return Ok(retained);
    };
    if bed_dimension != dimension {
        return Ok(retained);
    }
    let Some(form) = super::actor_placement::PlacementWorld::block_at(view, dimension, foot) else {
        return Ok(retained);
    };
    let invalid = PreparedDeathBed {
        respawn: None,
        candidate: None,
    };
    if !crate::rules::sleep::is_bed_foot(form) {
        return Ok(invalid);
    }
    let Some(direction) = crate::rules::sleep::bed_dir(form) else {
        return Ok(invalid);
    };
    let Some(head) = crate::rules::sleep::bed_head_neighbor(foot, direction) else {
        return Ok(retained);
    };
    let Some(head_form) = super::actor_placement::PlacementWorld::block_at(view, dimension, head)
    else {
        return Ok(retained);
    };
    if head_form != form + 4 {
        return Ok(invalid);
    }
    let shape = crate::rules::player_motion::collision_cell(form)?;
    let top = shape
        .boxes()
        .first()
        .ok_or(super::contracts::ServerError::Internal {
            invariant: "source player death bed",
        })?
        .maximum[1];
    Ok(PreparedDeathBed {
        respawn,
        candidate: Some(super::actor_placement::RestoreCandidate {
            dimension,
            position: [
                foot.x() as f32 + 0.5,
                foot.y() as f32 + top,
                foot.z() as f32 + 0.5,
            ],
            require_support: false,
        }),
    })
}
