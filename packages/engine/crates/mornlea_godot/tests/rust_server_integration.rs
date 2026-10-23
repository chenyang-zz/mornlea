//! Actual authority-to-client qualification through the safe Godot arena.
#![allow(dead_code)]
#[path = "../src/abi.rs"]
mod abi;
#[path = "../src/client_core.rs"]
mod client_core;
#[path = "../src/feature_negotiation.rs"]
mod feature_negotiation;
#[path = "support/rust_server.rs"]
mod rust_server;

#[test]
fn actual_memory_login_reaches_owned_godot_frame() {
    let mut rig = rust_server::Rig::memory();
    rig.login();
    let frame = rig.tick();
    assert_eq!(frame.families().len(), 10);
    assert!(frame.confirmed_revision().get() > 0);
    assert_eq!(rig.server.session(rust_server::player(1)).unwrap().get(), 2);
}
#[test]
fn actual_tcp_login_reaches_owned_godot_frame() {
    let mut rig = rust_server::Rig::tcp();
    rig.login();
    let frame = rig.tick();
    assert_eq!(frame.families().len(), 10);
    assert!(frame.confirmed_revision().get() > 0);
}

use mornlea_client_core::{
    ActorId, ClientError, ClientIntent, FamilyOperation, FamilyRecords, InputAction,
    InventoryUiView, PresentationFrame, WorldUiView,
};
use mornlea_domain::*;
use mornlea_server::contracts::{ActorKey, RuleEffect};
use std::collections::BTreeSet;
use std::sync::Arc;

fn control(x: i8, z: i8, pitch: f32) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: x,
            move_z: z,
            jump: false,
        },
        look: LookAngles::try_new(0.0, pitch).unwrap(),
        actions: HeldActions {
            primary: false,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    })
}
fn action(intent: ClientIntent) -> InputAction {
    InputAction {
        intent,
        container: None,
        crafting: None,
    }
}
fn records<'a>(frame: &'a PresentationFrame, name: &str) -> &'a FamilyRecords {
    frame
        .families()
        .iter()
        .find(|family| family.key().logical_name == name)
        .unwrap()
        .records()
}
fn all_families(tcp: bool) {
    let mut rig = if tcp {
        rust_server::Rig::tcp()
    } else {
        rust_server::Rig::memory()
    };
    rig.login();
    let first = rig.frame();
    let mut frames = vec![Arc::clone(&first)];
    let actors = match records(&first, "actors") {
        FamilyRecords::Actors(records) => records,
        _ => unreachable!(),
    };
    let kinds: BTreeSet<_> = actors
        .iter()
        .map(|record| format!("{:?}", record.kind()))
        .collect();
    assert_eq!(
        kinds,
        BTreeSet::from(
            [
                "RemotePlayer",
                "Drop",
                "Hostile",
                "Passive",
                "Projectile",
                "Companion"
            ]
            .map(str::to_owned)
        )
    );
    assert!(
        actors
            .iter()
            .any(|record| record.id() == &ActorId::RemotePlayer(rust_server::player(2)))
    );
    assert!(actors.iter().any(|record| record.id()
        == &ActorId::Companion(
            CompanionId::try_from_bytes(rust_server::player(9).bytes()).unwrap()
        )
        && record.position() == Some([8.5, 65.0, 8.5])));
    assert!(
        rig.capture.borrow().prepared > 0,
        "native CPU geometry is referenced and resolvable"
    );
    rig.submit(vec![action(ClientIntent::SelectHotbar(
        HotbarSlot::new(2).unwrap(),
    ))])
    .unwrap();
    frames.push(rig.tick());
    rig.submit(vec![action(ClientIntent::OpenContainer(
        LookAngles::try_new(0.0, 0.0).unwrap(),
    ))])
    .unwrap();
    frames.push(rig.tick());
    let chest_frame = rig.frame();
    let token = match records(&chest_frame, "inventory-ui") {
        FamilyRecords::InventoryUi(records) => records
            .iter()
            .find_map(|record| record.token().cloned())
            .unwrap(),
        _ => unreachable!(),
    };
    rig.submit(vec![InputAction {
        intent: ClientIntent::CloseContainer,
        container: Some(token),
        crafting: None,
    }])
    .unwrap();
    frames.push(rig.tick());
    let session = rig.server.session(rust_server::player(1)).unwrap();
    rig.server.stage(move |ctx| {
        let mut inventory = ctx
            .read()
            .inventory(ActorKey::Player(session))
            .cloned()
            .unwrap();
        for (index, item) in [(0, 1), (1, 37), (2, 1), (3, 37)] {
            inventory.crafting[index] = mornlea_storage::ItemStack {
                item,
                count: 1,
                durability: 0,
            };
        }
        ctx.preload_inventory(ActorKey::Player(session), inventory);
    });
    frames.push(rig.tick());
    rig.submit(vec![action(ClientIntent::OpenContainer(
        LookAngles::try_new(std::f32::consts::PI, 0.0).unwrap(),
    ))])
    .unwrap();
    frames.push(rig.tick());
    rig.submit(vec![action(ClientIntent::Chat(
        ChatIntent::try_new(
            CommandText::try_from_canonical("@companion-0900000000004000 mine stone".to_owned())
                .unwrap(),
        )
        .unwrap(),
    ))])
    .unwrap();
    frames.push(rig.tick());
    let companion = CompanionId::try_from_bytes(rust_server::player(9).bytes()).unwrap();
    let planning = rig
        .server
        .authority
        .take_companion_chat_planning(companion)
        .unwrap()
        .unwrap();
    let plan = mornlea_server::contracts::AgentPlan::try_new(
        "follow".to_owned(),
        vec![mornlea_server::contracts::PlanStep::Follow {
            player_id: rust_server::player(1),
        }],
    )
    .unwrap();
    assert!(
        rig.server
            .authority
            .install_companion_chat_plan(companion, planning.generation, plan)
            .unwrap()
    );
    frames.push(rig.tick());
    rig.submit(vec![action(ClientIntent::PlayerInput(control(0, 0, 0.7)))])
        .unwrap();
    frames.push(rig.tick());
    frames.extend(rig.transcript.iter().cloned());
    let nonempty: BTreeSet<_> = frames
        .iter()
        .flat_map(|frame| frame.families().iter())
        .filter(|family| family.records().record_count() > 0)
        .map(|family| family.key().logical_name)
        .collect();
    assert_eq!(nonempty.len(), 10, "nonempty families: {nonempty:?}");
    let inventory: BTreeSet<_> = frames
        .iter()
        .flat_map(|frame| match records(frame, "inventory-ui") {
            FamilyRecords::InventoryUi(records) => records,
            _ => unreachable!(),
        })
        .map(|record| match record.view() {
            InventoryUiView::Inventory(_) => "inventory",
            InventoryUiView::Crafting(_) => "crafting",
            InventoryUiView::Chest(_) => "chest",
            InventoryUiView::Furnace(_) => "furnace",
            _ => "other",
        })
        .collect();
    assert!(
        inventory.is_superset(&BTreeSet::from([
            "inventory",
            "crafting",
            "chest",
            "furnace"
        ])),
        "{inventory:?}"
    );
    let world: BTreeSet<_> = frames
        .iter()
        .flat_map(|frame| match records(frame, "world-ui") {
            FamilyRecords::WorldUi(records) => records,
            _ => unreachable!(),
        })
        .map(|record| match record.view() {
            WorldUiView::Environment(_) => "environment",
            WorldUiView::Survival(_) => "survival",
            WorldUiView::Chat(_) => "chat",
            WorldUiView::Task(_) => "task",
            WorldUiView::Prompt(Some(_)) => "prompt",
            _ => "other",
        })
        .collect();
    assert!(
        world.is_superset(&BTreeSet::from([
            "environment",
            "survival",
            "chat",
            "task",
            "prompt"
        ])),
        "{world:?}"
    );
    assert_source_values(&rig);
    for frame in &frames {
        frame
            .validate(&mornlea_client_core::ClientLimits::try_new().unwrap())
            .unwrap();
    }
}
#[test]
fn actual_memory_transcript_populates_all_ten_families() {
    all_families(false)
}
#[test]
fn actual_tcp_transcript_populates_all_ten_families() {
    all_families(true)
}

fn lifecycle(tcp: bool) {
    let mut rig = if tcp {
        rust_server::Rig::tcp()
    } else {
        rust_server::Rig::memory()
    };
    rig.login();
    let old_epoch = rig.epoch;
    let old_owned = rig.boundary.clone().unwrap();
    let old_memory = rig.memory_connector().map(|connector| {
        let ticket = connector.ticket();
        (connector, ticket)
    });
    rig.close_server();
    let deadline = std::time::Instant::now() + rust_server::DEADLINE;
    loop {
        let report = rig.step();
        if report.terminal().is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real close did not terminate client"
        );
        std::thread::yield_now();
    }
    assert!(rig.server.session(rust_server::player(1)).is_none());
    assert_eq!(
        rig.arena.reset(rig.token, old_epoch),
        Err(ClientError::Disconnected)
    );
    rig.reconnect();
    assert_ne!(rig.epoch, old_epoch);
    assert_eq!(
        rig.arena.pull_frame(rig.token, old_epoch),
        Err(ClientError::StaleEpoch)
    );
    if let Some((connector, ticket)) = old_memory {
        assert_eq!(
            connector.feed(ticket, &[0xff]),
            Err(ClientError::StaleEpoch)
        );
    }
    rig.login();
    assert_ne!(rig.boundary.as_ref().unwrap(), &old_owned);
    assert!(rig.server.session(rust_server::player(1)).is_some());
    rig.arena.close(rig.token).unwrap();
    rig.arena.close(rig.token).unwrap();
    assert_eq!(rig.arena.releases(), 1);
    assert!(rig.arena.visible_frame(rig.token).is_err());
}
#[test]
fn actual_memory_close_reset_fences_retired_generation() {
    lifecycle(false)
}
#[test]
fn actual_tcp_close_reset_fences_retired_epoch() {
    lifecycle(true)
}

#[test]
fn injected_malformed_and_capacity_frames_preserve_actual_last_visible_copy() {
    let mut rig = rust_server::Rig::memory();
    rig.login();
    let frame = rig.frame();
    let before = rig.arena.visible_frame(rig.token).unwrap().clone();
    let mut families = frame.families().to_vec();
    families.push(families[0].clone());
    rig.capture.borrow_mut().inject = Some(
        PresentationFrame::try_new(
            frame.session_epoch(),
            frame.confirmed_revision(),
            frame.frame_index() + 1,
            families,
        )
        .unwrap(),
    );
    assert_eq!(
        rig.arena.pull_frame(rig.token, rig.epoch),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(rig.arena.visible_frame(rig.token).unwrap(), &before);
    let mut families = frame.families().to_vec();
    let index = families
        .iter()
        .position(|family| family.key().logical_name == "diagnostics")
        .unwrap();
    let diagnostic = match families[index].records() {
        FamilyRecords::Diagnostics(records) => records[0],
        _ => unreachable!(),
    };
    families[index] = mornlea_client_core::FamilyFrame::try_new(
        *families[index].key(),
        FamilyRecords::Diagnostics(vec![diagnostic; 4097]),
    )
    .unwrap();
    rig.capture.borrow_mut().inject = Some(
        PresentationFrame::try_new(
            frame.session_epoch(),
            frame.confirmed_revision(),
            frame.frame_index() + 1,
            families,
        )
        .unwrap(),
    );
    assert_eq!(
        rig.arena.pull_frame(rig.token, rig.epoch),
        Err(ClientError::Capacity)
    );
    assert_eq!(rig.arena.visible_frame(rig.token).unwrap(), &before);
    let stale =
        mornlea_client_core::SessionEpoch::try_new(frame.session_epoch().get() + 1).unwrap();
    assert_eq!(
        rig.arena.pull_frame(rig.token, stale),
        Err(ClientError::StaleEpoch)
    );
    assert_eq!(rig.arena.visible_frame(rig.token).unwrap(), &before);
}

#[test]
fn actual_input_capacity_and_invalid_mixed_batch_preserve_sequence() {
    use mornlea_client_core::{InputBatch, InputReceipt};
    let mut rig = rust_server::Rig::memory();
    rig.login();
    let select = action(ClientIntent::SelectHotbar(HotbarSlot::new(2).unwrap()));
    let before = rig.arena.visible_frame(rig.token).unwrap().clone();
    assert_eq!(
        InputBatch::try_new(rig.epoch, vec![select.clone(); 129]),
        Err(ClientError::Capacity)
    );
    assert_eq!(
        rig.arena.submit(
            rig.token,
            InputBatch::try_new(
                rig.epoch,
                vec![select.clone(), action(ClientIntent::CloseContainer)]
            )
            .unwrap()
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(rig.arena.visible_frame(rig.token).unwrap(), &before);
    let receipt = rig
        .arena
        .submit(
            rig.token,
            InputBatch::try_new(rig.epoch, vec![select.clone(); 128]).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        receipt,
        InputReceipt::Queued {
            first_sequence: Some(1),
            sequenced_count: 128,
            ..
        }
    ));
    assert_eq!(
        rig.arena.submit(
            rig.token,
            InputBatch::try_new(rig.epoch, vec![select.clone()]).unwrap()
        ),
        Err(ClientError::Capacity)
    );
    rig.step();
    rig.pump();
    rig.tick();
    assert!(matches!(
        rig.submit(vec![select]).unwrap(),
        InputReceipt::Queued {
            first_sequence: Some(129),
            sequenced_count: 1,
            ..
        }
    ));
}

fn actor_removal(tcp: bool) {
    let mut rig = if tcp {
        rust_server::Rig::tcp()
    } else {
        rust_server::Rig::memory()
    };
    rig.login();
    let mut frames = vec![rig.frame()];
    for _ in 0..3 {
        frames.push(rig.tick());
    }
    let session = rig.server.session(rust_server::player(1)).unwrap();
    rig.server.stage(move |ctx| {
        let mut actor = ctx
            .read()
            .actor(ActorKey::Player(session))
            .cloned()
            .unwrap();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([400.5, 65.0, 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        ctx.stage(RuleEffect::Actor(actor)).unwrap();
    });
    frames.push(rig.tick());
    let removed: BTreeSet<_> = frames
        .iter()
        .flat_map(|frame| match records(frame, "actors") {
            FamilyRecords::Actors(records) => records,
            _ => unreachable!(),
        })
        .filter(|record| record.header().operation() == FamilyOperation::Remove)
        .map(|record| format!("{:?}", record.kind()))
        .collect();
    assert_eq!(
        removed,
        BTreeSet::from(
            [
                "RemotePlayer",
                "Drop",
                "Hostile",
                "Passive",
                "Projectile",
                "Companion"
            ]
            .map(str::to_owned)
        )
    );
    let terrain = match records(frames.last().unwrap(), "terrain") {
        FamilyRecords::Terrain(records) => records,
        _ => unreachable!(),
    };
    assert!(
        terrain.iter().any(
            |record| record.header().operation() == FamilyOperation::Remove
                && record.header().source_tick().is_none()
        ),
        "actual ForgetChunks has no source tick"
    );
}
#[test]
fn actual_memory_all_actor_removals_and_terrain_forget() {
    actor_removal(false)
}
#[test]
fn actual_tcp_all_actor_removals_and_terrain_forget() {
    actor_removal(true)
}

fn assert_source_values(rig: &rust_server::Rig) {
    let inventory: Vec<_> = rig
        .transcript
        .iter()
        .flat_map(|frame| match records(frame, "inventory-ui") {
            FamilyRecords::InventoryUi(records) => records.iter(),
            _ => unreachable!(),
        })
        .collect();
    let world: Vec<_> = rig
        .transcript
        .iter()
        .flat_map(|frame| match records(frame, "world-ui") {
            FamilyRecords::WorldUi(records) => records.iter(),
            _ => unreachable!(),
        })
        .collect();
    let player: Vec<_> = rig
        .transcript
        .iter()
        .flat_map(|frame| match records(frame, "player-view") {
            FamilyRecords::PlayerView(records) => records.iter(),
            _ => unreachable!(),
        })
        .collect();
    let actors: Vec<_> = rig
        .transcript
        .iter()
        .flat_map(|frame| match records(frame, "actors") {
            FamilyRecords::Actors(records) => records.iter(),
            _ => unreachable!(),
        })
        .collect();
    for event in rig
        .wire
        .iter()
        .cloned()
        .filter_map(|packet| Event::try_from(packet).ok())
    {
        match &event {
            Event::InventoryState(state)=>assert!(inventory.iter().any(|record|matches!(record.view(),InventoryUiView::Inventory(observed) if observed==state))),
            Event::ChestState(state)=>assert!(inventory.iter().any(|record|matches!(record.view(),InventoryUiView::Chest(observed) if observed==state))),
            Event::CraftingState(state)=>assert!(inventory.iter().any(|record|matches!(record.view(),InventoryUiView::Crafting(observed) if observed==state))),
            Event::FurnaceState(state)=>assert!(inventory.iter().any(|record|matches!(record.view(),InventoryUiView::Furnace(observed) if observed==state))),
            Event::Chat(chat)=>{
                assert!(world.iter().any(|record|matches!(record.view(),WorldUiView::Chat(observed) if observed==chat)));
                if let ChatBody::Task{companion,command,state}=chat.body(){
                    assert!(world.iter().any(|record|record.header().source_tick().is_none() && matches!(record.view(),WorldUiView::Task(observed) if observed.companion()==companion && observed.command()==command && observed.state()==state)));
                }
            },
            Event::PlayerState(state)=>{
                assert!(player.iter().any(|record|record.header().source_tick().is_none() && record.confirmed_pose().position()==state.motion().position().get().map(f64::from) && record.confirmed_pose().yaw()==f64::from(state.look().yaw()) && record.confirmed_pose().pitch()==f64::from(state.look().pitch()) && record.mining()==&state.mining()));
                assert!(world.iter().any(|record|record.header().source_tick()==Some(state.server_tick()) && matches!(record.view(),WorldUiView::Environment(observed) if observed==&state.world())));
                assert!(world.iter().any(|record|record.header().source_tick()==Some(state.server_tick()) && matches!(record.view(),WorldUiView::Survival(observed) if observed==&state.survival())));
            }
            Event::RemotePlayerSpawn(spawn)=>assert!(actors.iter().any(|record|record.id()==&ActorId::RemotePlayer(spawn.player_id()) && record.header().source_tick()==Some(spawn.server_tick()) && record.position()==Some(spawn.position().get().map(f64::from)) && record.yaw()==Some(f64::from(spawn.look().yaw())) && record.pitch()==Some(f64::from(spawn.look().pitch())) && record.velocity().is_none() && matches!(record.detail(),Some(mornlea_client_core::ActorDetail::RemotePlayer{display_name:Some(name),reset:None}) if name.as_str()==spawn.display_name().as_str()))),
            Event::CompanionSpawn(spawn)=>assert!(actors.iter().any(|record|record.id()==&ActorId::Companion(spawn.id()) && record.header().source_tick()==Some(spawn.server_tick()) && record.position()==Some(spawn.position().get().map(f64::from)) && record.yaw()==Some(f64::from(spawn.look().yaw())) && record.pitch()==Some(f64::from(spawn.look().pitch())) && record.velocity().is_none() && matches!(record.detail(),Some(mornlea_client_core::ActorDetail::Companion{name:Some(name),reset:None}) if name.as_str()==spawn.name().as_str()))),
            Event::HostileSpawn(batch)=>for spawn in batch.spawns(){assert!(actors.iter().any(|record|record.id()==&ActorId::Hostile(spawn.id()) && record.header().source_tick()==Some(batch.server_tick()) && record.position()==Some(spawn.position().get().map(f64::from)) && record.yaw()==Some(f64::from(spawn.yaw())) && matches!(record.detail(),Some(mornlea_client_core::ActorDetail::Hostile{health,..}) if *health==spawn.health())));},
            Event::PassiveSpawn(batch)=>for spawn in batch.spawns(){assert!(actors.iter().any(|record|record.id()==&ActorId::Passive(spawn.id()) && record.header().source_tick()==Some(batch.server_tick()) && record.position()==Some(spawn.position().get().map(f64::from)) && record.yaw()==Some(f64::from(spawn.yaw()))));},
            Event::ProjectileSpawn(batch)=>for spawn in batch.spawns(){assert!(actors.iter().any(|record|record.id()==&ActorId::Projectile(spawn.id()) && record.header().source_tick()==Some(batch.server_tick()) && record.position()==Some(spawn.position().get().map(f64::from)) && record.velocity()==Some(spawn.velocity().get().map(f64::from)) && record.yaw().is_none() && record.pitch().is_none()));},
            Event::ItemDropUpserts(batch)=>for drop in batch.drops(){assert!(actors.iter().any(|record|record.id()==&ActorId::Drop(drop.id()) && record.header().source_tick()==Some(batch.server_tick()) && matches!(record.detail(),Some(mornlea_client_core::ActorDetail::Drop{block_index,stack}) if *block_index==drop.block_index() && *stack==drop.stack())));},
            _=>{}
        }
    }
}

fn live_reset(tcp: bool, cycles: usize) {
    let mut rig = if tcp {
        rust_server::Rig::tcp()
    } else {
        rust_server::Rig::memory()
    };
    rig.login();
    for _ in 0..cycles {
        rig.transcript.clear();
        let old = rig.epoch;
        let old_session = rig.server.session(rust_server::player(1)).unwrap();
        let retired_resources = rig.capture.borrow().retired_resources;
        let memory = rig.memory_connector().map(|connector| {
            let ticket = connector.ticket();
            (connector, ticket)
        });
        rig.reset();
        assert!(
            rig.capture.borrow().retired_resources > retired_resources,
            "actual terrain references retire with the epoch"
        );
        assert_ne!(old, rig.epoch);
        assert_eq!(
            rig.arena.pull_frame(rig.token, old),
            Err(ClientError::StaleEpoch)
        );
        if let Some((connector, ticket)) = memory {
            assert_eq!(
                connector.feed(ticket, &[0xff]),
                Err(ClientError::StaleEpoch)
            );
        }
        rig.login();
        assert_ne!(
            rig.server.session(rust_server::player(1)).unwrap(),
            old_session,
            "the same player receives a fresh authoritative session after client retirement"
        );
        rig.prove_retired_memory_close();
        assert!(
            rig.step().terminal().is_none(),
            "retired generation terminal cannot close the fresh session"
        );
        let receipt = rig
            .submit(vec![action(ClientIntent::SelectHotbar(
                HotbarSlot::new(1).unwrap(),
            ))])
            .unwrap();
        assert!(matches!(
            receipt,
            mornlea_client_core::InputReceipt::Queued {
                first_sequence: Some(1),
                ..
            }
        ));
        rig.tick();
    }
    rig.arena.close(rig.token).unwrap();
}
#[test]
fn actual_memory_live_reset_reconnects_and_restarts_sequence() {
    live_reset(false, 1)
}
#[test]
fn actual_tcp_live_reset_reconnects_and_restarts_sequence() {
    live_reset(true, 1)
}
#[test]
fn actual_tcp_repeated_same_player_reset_requires_client_socket_retirement() {
    live_reset(true, 32)
}

/// Repeated pulls retain the same publication; identity, not snapshot reference count, defines one frame.
fn published_frames(rig: &rust_server::Rig) -> impl Iterator<Item = &PresentationFrame> {
    let mut seen = BTreeSet::new();
    rig.transcript
        .iter()
        .filter(move |frame| seen.insert((frame.session_epoch().get(), frame.frame_index())))
        .map(Arc::as_ref)
}

fn assert_confirmations(
    rig: &rust_server::Rig,
    expected: &[(u64, mornlea_client_core::ClientIntentKind)],
) {
    use mornlea_client_core::{ClientIntentKind, InputReceiptState};
    let mut confirmed = Vec::new();
    for frame in published_frames(rig) {
        let input = match records(frame, "input") {
            FamilyRecords::Input(records) => records,
            _ => unreachable!(),
        };
        for record in input {
            let InputReceiptState::Confirmed { server_tick } = record.receipt() else {
                continue;
            };
            let sequence = record
                .local_sequence()
                .expect("only sequenced commands can be confirmed");
            assert_ne!(
                record.intent_kind(),
                ClientIntentKind::Chat,
                "chat has no authority sequence acknowledgment"
            );
            assert!(
                expected.contains(&(sequence, record.intent_kind())),
                "confirmation preserves its admitted command kind"
            );
            let session = rig.server.session(rust_server::player(1)).unwrap();
            assert!(rig.server.submissions.iter().any(|(owner,intent)|{
                *owner==session && matches!(intent,mornlea_protocol::PlayIntent::Sequenced{sequence:source,command} if *source==sequence && matches!((command,record.intent_kind()),(Command::PlayerInput(_),ClientIntentKind::PlayerInput)|(Command::PlaceBlock(_),ClientIntentKind::PlaceBlock)))
            }),"confirmation kind matches actual authoritative ingress");
            assert_eq!(record.header().epoch(), frame.session_epoch());
            assert_eq!(record.header().revision(), frame.confirmed_revision());
            assert_eq!(record.header().source_tick(), *server_tick);
            assert!(
                rig.wire
                    .iter()
                    .cloned()
                    .filter_map(|packet| Event::try_from(packet).ok())
                    .enumerate()
                    .any(|(index, event)| {
                        if index as u64 + 1 > frame.confirmed_revision().get() {
                            return false;
                        }
                        match (event, server_tick) {
                            (Event::PlayerState(state), Some(tick)) => {
                                state.server_tick() == *tick
                                    && state.last_input_sequence() >= sequence
                            }
                            (Event::PlaceBlockSucceeded(success), None) => {
                                record.intent_kind() == ClientIntentKind::PlaceBlock
                                    && success.sequence() == sequence
                            }
                            _ => false,
                        }
                    }),
                "confirmation must attest an actual consumed outbox acknowledgment"
            );
            confirmed.push((sequence, record.intent_kind()));
        }
    }
    for entry in expected {
        assert_eq!(
            confirmed
                .iter()
                .filter(|confirmed| *confirmed == entry)
                .count(),
            1,
            "one authoritative acknowledgment publishes each held sequence and kind once"
        );
    }
}

fn correction_and_audio(tcp: bool) {
    use mornlea_client_core::{CueProvenance, InputReceipt, InputReceiptState};
    let mut rig = if tcp {
        rust_server::Rig::tcp()
    } else {
        rust_server::Rig::memory()
    };
    rig.server.snow_ground();
    rig.login();
    let session = rig.server.session(rust_server::player(1)).unwrap();
    for _ in 0..16 {
        if rig
            .server
            .authority
            .settled_read()
            .unwrap()
            .actor(ActorKey::Player(session))
            .unwrap()
            .motion
            .on_ground()
        {
            break;
        }
        rig.tick();
    }
    assert!(
        rig.server
            .authority
            .settled_read()
            .unwrap()
            .actor(ActorKey::Player(session))
            .unwrap()
            .motion
            .on_ground()
    );
    let initial = rig.frame();
    let initial_position = match records(&initial, "player-view") {
        FamilyRecords::PlayerView(records) => records[0].confirmed_pose().position(),
        _ => unreachable!(),
    };
    assert_eq!(
        initial_position,
        rig.server
            .authority
            .settled_read()
            .unwrap()
            .actor(ActorKey::Player(session))
            .unwrap()
            .motion
            .position()
            .get()
            .map(f64::from)
    );
    let mut sounded = None;
    for sequence in 1..=64 {
        let before = rig.frame().frame_index();
        let receipt = rig
            .submit(vec![action(ClientIntent::PlayerInput(control(1, 0, 0.0)))])
            .unwrap();
        assert!(
            matches!(receipt,InputReceipt::Queued{first_sequence:Some(observed),sequenced_count:1,..} if observed==sequence)
        );
        let frame = rig.frame();
        let cues = match records(&frame, "audio-cues") {
            FamilyRecords::AudioCues(records) => records,
            _ => unreachable!(),
        };
        assert_eq!(
            frame.frame_index(),
            before + 1,
            "transport ingress waiting preserves the semantic publication"
        );
        if cues.iter().any(|cue|matches!(cue.provenance(),CueProvenance::Predicted{input_sequence} if *input_sequence==sequence)) {
            let pose=match records(&frame,"player-view"){FamilyRecords::PlayerView(records)=>records[0].predicted_pose().unwrap().position(),_=>unreachable!()};
            let foot=BlockPos::new(pose[0].floor() as i32,pose[1].floor() as i32,pose[2].floor() as i32);
            assert_eq!(rig.server.authority.settled_read().unwrap().block(Dimension::OVERWORLD,foot),Some(88));
            assert!(pose[0]-initial_position[0]>=0.8 && pose[0]-initial_position[0]<8.0);
            sounded=Some(sequence);break;
        }
    }
    let sequence = sounded.expect("actual predicted movement on source snow emits a stride cue");
    let session = rig.server.session(rust_server::player(1)).unwrap();
    assert!(rig.server.submissions.iter().any(|(owner,intent)|*owner==session && matches!(intent,mornlea_protocol::PlayIntent::Sequenced{sequence:observed,command:mornlea_domain::Command::PlayerInput(_)} if *observed==sequence)));
    rig.server.stage(move |ctx| {
        let mut actor = ctx
            .read()
            .actor(ActorKey::Player(session))
            .cloned()
            .unwrap();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([2.5, actor.motion.position().get()[1], 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        actor.survival = SurvivalState::try_new(SurvivalStateParts {
            health: 19,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap();
        ctx.stage(RuleEffect::Actor(actor)).unwrap();
    });
    let frame =
        rig.tick_commands(1);
    assert_eq!(
        rig.server
            .authority
            .session(session)
            .unwrap()
            .last_applied_sequence,
        1
    );
    let view = match records(&frame, "player-view") {
        FamilyRecords::PlayerView(records) => &records[0],
        _ => unreachable!(),
    };
    assert_eq!(view.correction().unwrap().last_input_sequence(), 1);
    assert!(
        view.predicted_pose().is_some(),
        "later admitted movement is replayed after correction"
    );
    assert_ne!(view.predicted_pose().unwrap(), view.confirmed_pose());
    assert!(rig.transcript.iter().any(|frame| matches!(records(frame, "input"), FamilyRecords::Input(records) if records.iter().any(|record|
        record.local_sequence() == Some(1)
            && record.intent_kind() == mornlea_client_core::ClientIntentKind::PlayerInput
            && matches!(record.receipt(), InputReceiptState::Confirmed { server_tick: Some(_) })
            && record.header().revision() == frame.confirmed_revision()
    ))));
    assert!(rig.transcript.iter().any(|frame|matches!(records(frame,"input"),FamilyRecords::Input(records) if records.iter().any(|record|record.local_sequence()==Some(1) && matches!(record.receipt(),InputReceiptState::Queued)))));
    assert!(
        matches!(records(&frame,"audio-cues"),FamilyRecords::AudioCues(records) if !records.iter().any(|cue|matches!(cue.provenance(),CueProvenance::Predicted{input_sequence} if *input_sequence==sequence))),
        "correction replay does not emit the same predicted stride twice"
    );
    rig.tick();
    let confirmed = rig.frame();
    let view = match records(&confirmed, "player-view") {
        FamilyRecords::PlayerView(records) => &records[0],
        _ => unreachable!(),
    };
    assert!(view.predicted_pose().is_none());
    assert_eq!(
        rig.server
            .authority
            .session(session)
            .unwrap()
            .last_applied_sequence,
        sequence
    );
    rig.submit(vec![action(ClientIntent::Chat(
        ChatIntent::try_new(
            CommandText::try_from_canonical("source audio acknowledgment".to_owned()).unwrap(),
        )
        .unwrap(),
    ))])
    .unwrap();
    rig.tick();
    assert!(rig.transcript.iter().any(|frame|matches!(records(frame,"audio-cues"),FamilyRecords::AudioCues(records) if records.iter().any(|cue|matches!(cue.provenance(),CueProvenance::Confirmed{authoritative_event_id:Some(_),..})))));
    assert_source_values(&rig);
    let expected: Vec<_> = (1..=sequence)
        .map(|held| (held, mornlea_client_core::ClientIntentKind::PlayerInput))
        .collect();
    assert_confirmations(&rig, &expected);
}
#[test]
fn actual_memory_correction_replays_pending_input_and_keeps_audio_provenance() {
    correction_and_audio(false)
}
#[test]
fn actual_tcp_correction_replays_pending_input_and_keeps_audio_provenance() {
    correction_and_audio(true)
}

#[test]
fn actual_tcp_two_clients_with_earlier_bare_probe_keep_connection_ownership() {
    use mornlea_client_core::{
        ClientConfig, ClientCore, ClientEndpoint, ClientIdentity, ClientLimits, ClientWorkBudget,
        ConnectorRegistry, Endpoint, StdMonotonicClock,
    };
    use std::time::{Duration, Instant};
    let mut server = rust_server::TcpServer::new();
    let probe = std::net::TcpStream::connect(server.address()).unwrap();
    server.pump();
    let mut clients = Vec::new();
    for (tag, name) in [(1, "Ada"), (3, "Cid")] {
        let config = ClientConfig::try_new(
            ClientLimits::try_new().unwrap(),
            Duration::from_secs(5),
            Duration::from_secs(10),
            Arc::new(StdMonotonicClock),
            Arc::new(ConnectorRegistry::new()),
        )
        .unwrap();
        let mut core = ClientCore::new(config).unwrap();
        let epoch = core
            .connect(
                Endpoint::Tcp(server.address()),
                ClientIdentity::try_new(
                    mornlea_protocol::LoginStart::new(rust_server::player(tag), name, 8).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        clients.push((tag, core, epoch));
    }
    let deadline = Instant::now() + rust_server::DEADLINE;
    loop {
        server.pump();
        for (_, core, epoch) in &mut clients {
            core.step(*epoch, ClientWorkBudget::try_new(4096, 4096).unwrap())
                .unwrap();
        }
        if server.admitted.len() == 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "concurrent actual admissions stalled"
        );
        std::thread::yield_now();
    }
    server.server.tick();
    for tag in [1, 3] {
        let session = server.server.session(rust_server::player(tag)).unwrap();
        server.server.stage(move |ctx| {
            let mut actor = ctx
                .read()
                .actor(ActorKey::Player(session))
                .cloned()
                .unwrap();
            actor.motion = MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new([f32::from(tag) * 2.0 + 0.5, 65.0, 0.5]).unwrap(),
                velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
                on_ground: true,
            });
            ctx.stage(RuleEffect::Actor(actor)).unwrap();
        });
    }
    server.server.tick();
    loop {
        server.pump();
        let mut ready = true;
        for (tag, core, epoch) in &mut clients {
            core.step(*epoch, ClientWorkBudget::try_new(4096, 4096).unwrap())
                .unwrap();
            let frame = core.snapshot(*epoch).unwrap();
            let session = match records(&frame, "session") {
                FamilyRecords::Session(records) => &records[0],
                _ => unreachable!(),
            };
            if let Some(player) = session.player_id() {
                assert_eq!(player, rust_server::player(*tag));
            } else {
                ready = false;
            }
            if frame.confirmed_revision().get() < 2 {
                ready = false;
            }
            let view = match records(&frame, "player-view") {
                FamilyRecords::PlayerView(records) => records.first(),
                _ => unreachable!(),
            };
            if let Some(view) = view {
                let expected = f64::from(*tag) * 2.0 + 0.5;
                let observed = view.confirmed_pose().position()[0];
                assert!(
                    observed == 0.5 || observed == expected,
                    "source player state reached the wrong owner"
                );
                if observed != expected {
                    ready = false;
                }
            } else {
                ready = false;
            }
        }
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "source packets were misrouted");
        std::thread::yield_now();
    }
    assert!(server.errors.is_empty());
    assert!(server.server.session(rust_server::player(1)).is_some());
    assert!(server.server.session(rust_server::player(3)).is_some());
    drop(probe);
    for (_, mut core, _) in clients {
        core.close().unwrap();
    }
    server.pump();
    server.close_connections();
    assert_eq!(server.transport.retained_sockets(), 0);
}

fn placement_delta(tcp: bool) -> Vec<mornlea_server::contracts::TickPublication> {
    let mut rig = if tcp {
        rust_server::Rig::tcp()
    } else {
        rust_server::Rig::memory()
    };
    rig.server.ground_only();
    rig.login();
    let session = rig.server.session(rust_server::player(1)).unwrap();
    rig.server.stage(move |ctx| {
        let mut inventory = ctx
            .read()
            .inventory(ActorKey::Player(session))
            .cloned()
            .unwrap();
        inventory.slots[0] = mornlea_storage::ItemStack {
            item: 2,
            count: 3,
            durability: 0,
        };
        ctx.preload_inventory(ActorKey::Player(session), inventory);
    });
    rig.tick();
    rig.frame();
    let old = rig
        .server
        .authority
        .settled_read()
        .unwrap()
        .ready_chunk_revision(mornlea_server::contracts::ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        })
        .unwrap();
    assert!(rig.transcript.iter().any(|frame|matches!(records(frame,"terrain"),FamilyRecords::Terrain(records) if records.iter().any(|record|record.resource().is_some()))));
    rig.submit(vec![action(ClientIntent::PlaceBlock(
        PlacementIntent::try_new(
            LookAngles::try_new(std::f32::consts::PI, -std::f32::consts::FRAC_PI_4).unwrap(),
            0,
        )
        .unwrap(),
    ))])
    .unwrap();
    let frame = rig.tick();
    assert_confirmations(
        &rig,
        &[(1, mornlea_client_core::ClientIntentKind::PlaceBlock)],
    );
    let change = rig
        .wire
        .iter()
        .rev()
        .find_map(|packet| match Event::try_from(packet.clone()).ok() {
            Some(Event::BlockChanges(changes))
                if changes.changes().iter().any(|change| {
                    change == &BlockChange::try_new(BlockPos::new(0, 65, 2), 3).unwrap()
                }) =>
            {
                Some(changes)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(change.base_revision(), old);
    assert_eq!(change.new_revision(), old + 1);
    assert_eq!(
        change.changes(),
        &[BlockChange::try_new(BlockPos::new(0, 65, 2), 3).unwrap()]
    );
    assert_eq!(
        rig.server
            .authority
            .settled_read()
            .unwrap()
            .block(Dimension::OVERWORLD, BlockPos::new(0, 65, 2)),
        Some(3)
    );
    let terrain = match records(&frame, "terrain") {
        FamilyRecords::Terrain(records) => records,
        _ => unreachable!(),
    };
    assert!(
        terrain
            .iter()
            .any(|record| record.content_revision() == change.new_revision()
                && record.resource().is_some())
    );
    assert_source_values(&rig);
    rig.publications
}
#[test]
fn actual_memory_and_tcp_placement_publications_match_with_prepared_terrain_delta() {
    assert_eq!(placement_delta(false), placement_delta(true));
}
