//! Tick-local command ownership shared by ordered provider phase occurrences.

use std::collections::BTreeMap;

use mornlea_domain::CommandEnvelope;

use super::contracts::{EFFECT_BUDGET, Resource, RulePhase, ServerError};

const PHASE_COUNT: usize = RulePhase::EnvironmentEnd as usize + 1;

pub(super) struct DeferredCommands {
    records: Vec<CommandEnvelope>,
    index: BTreeMap<(u64, u64, u64, u64), usize>,
    phases: [Vec<usize>; PHASE_COUNT],
}

impl Default for DeferredCommands {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            index: BTreeMap::new(),
            phases: std::array::from_fn(|_| Vec::new()),
        }
    }
}

impl DeferredCommands {
    pub(super) fn defer(
        &mut self,
        command: CommandEnvelope,
        phase: RulePhase,
    ) -> Result<(), ServerError> {
        let occurrences = self
            .phases
            .get_mut(phase as usize)
            .ok_or(ServerError::Internal {
                invariant: "deferred phase space",
            })?;
        let key = (
            command.tick(),
            command.session(),
            command.sequence(),
            command.arrival_index(),
        );
        let ordinal = self.index.get(&key).copied();
        // A shared ordering identity must still name the same immutable payload.
        if let Some(ordinal) = ordinal
            && self.records[ordinal] != command
        {
            return Err(ServerError::Internal {
                invariant: "deferred command identity",
            });
        }
        // Bound occurrences separately: repeated delivery remains observable to providers.
        if occurrences.len() >= EFFECT_BUDGET
            || (ordinal.is_none() && self.records.len() >= EFFECT_BUDGET)
        {
            return Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: EFFECT_BUDGET,
                observed: EFFECT_BUDGET + 1,
            });
        }
        let ordinal = match ordinal {
            Some(ordinal) => ordinal,
            None => {
                let ordinal = self.records.len();
                self.records.push(command);
                self.index.insert(key, ordinal);
                ordinal
            }
        };
        occurrences.push(ordinal);
        Ok(())
    }

    pub(super) fn for_phase(&self, phase: RulePhase) -> Vec<CommandEnvelope> {
        // Reads preserve phase insertion order and never consume the tick-local owner.
        self.phases[phase as usize]
            .iter()
            .map(|ordinal| self.records[*ordinal])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_domain::{
        Command, CommandEnvelopeParts, HeldActions, LookAngles, Movement, PlayerControl,
        PlayerControlParts,
    };

    fn envelope(key: (u64, u64, u64, u64), command: Command) -> CommandEnvelope {
        CommandEnvelope::try_new(CommandEnvelopeParts {
            tick: key.0,
            session: key.1,
            sequence: key.2,
            arrival_index: key.3,
            command,
        })
        .unwrap()
    }

    fn open(key: (u64, u64, u64, u64)) -> CommandEnvelope {
        envelope(
            key,
            Command::OpenContainer(LookAngles::try_new(0.0, 0.0).unwrap()),
        )
    }

    fn capacity() -> ServerError {
        ServerError::Capacity {
            resource: Resource::Commands,
            limit: EFFECT_BUDGET,
            observed: EFFECT_BUDGET + 1,
        }
    }

    #[test]
    fn full_joint_roles_share_owners_and_preserve_phase_order() {
        let mut owner = DeferredCommands::default();
        let commands: Vec<_> = (1..=EFFECT_BUDGET as u64)
            .map(|sequence| open((0, 1, sequence, 0)))
            .collect();
        for command in &commands {
            owner.defer(*command, RulePhase::ContainerMove).unwrap();
            owner
                .defer(*command, RulePhase::WorkbenchLifecycle)
                .unwrap();
        }
        assert_eq!(owner.records.len(), commands.len());
        assert_eq!(owner.records, commands);
        assert_eq!(owner.index.len(), EFFECT_BUDGET);
        assert_eq!(
            owner.phases[RulePhase::ContainerMove as usize],
            (0..EFFECT_BUDGET).collect::<Vec<_>>()
        );
        assert_eq!(
            owner.phases[RulePhase::WorkbenchLifecycle as usize],
            (0..EFFECT_BUDGET).collect::<Vec<_>>()
        );
        let allocation = owner.records.as_ptr();
        for phase in [RulePhase::ContainerMove, RulePhase::WorkbenchLifecycle] {
            assert_eq!(owner.for_phase(phase), commands);
            assert_eq!(owner.for_phase(phase), commands);
        }
        owner.defer(commands[0], RulePhase::Interaction).unwrap();
        assert_eq!(owner.records.as_ptr(), allocation);
        assert_eq!(owner.records.len(), commands.len());
        assert_eq!(owner.records, commands);
        assert_eq!(owner.index.len(), EFFECT_BUDGET);
        assert_eq!(owner.for_phase(RulePhase::Interaction), [commands[0]]);
        assert_eq!(
            owner.defer(commands[0], RulePhase::ContainerMove),
            Err(capacity())
        );
        assert_eq!(
            owner.defer(
                open((0, 1, EFFECT_BUDGET as u64 + 1, 0)),
                RulePhase::Interaction
            ),
            Err(capacity())
        );
        assert_eq!(owner.records.len(), commands.len());
        assert_eq!(owner.records, commands);
        assert_eq!(owner.index.len(), EFFECT_BUDGET);
        assert_eq!(owner.for_phase(RulePhase::ContainerMove), commands);
        assert_eq!(owner.for_phase(RulePhase::WorkbenchLifecycle), commands);
        assert_eq!(owner.for_phase(RulePhase::Interaction), [commands[0]]);
    }

    #[test]
    fn repeated_same_phase_occurrences_are_retained_and_bounded() {
        let mut owner = DeferredCommands::default();
        let command = open((0, 1, 1, 0));
        owner.defer(command, RulePhase::ContainerMove).unwrap();
        owner.defer(command, RulePhase::ContainerMove).unwrap();
        assert_eq!(
            owner.for_phase(RulePhase::ContainerMove),
            [command, command]
        );
        assert_eq!(owner.records, [command]);
        assert_eq!(owner.index.len(), 1);
        for _ in 2..EFFECT_BUDGET {
            owner.defer(command, RulePhase::ContainerMove).unwrap();
        }
        assert_eq!(
            owner.defer(command, RulePhase::ContainerMove),
            Err(capacity())
        );
        assert_eq!(
            owner.for_phase(RulePhase::ContainerMove),
            vec![command; EFFECT_BUDGET]
        );
        assert_eq!(owner.records, [command]);
        assert_eq!(owner.index.len(), 1);
        owner.defer(command, RulePhase::WorkbenchLifecycle).unwrap();
        assert_eq!(owner.for_phase(RulePhase::WorkbenchLifecycle), [command]);
    }

    #[test]
    fn invalid_controls_share_motion_and_interaction_owner() {
        let mut owner = DeferredCommands::default();
        let command = envelope(
            (0, 1, 1, 0),
            Command::PlayerInput(PlayerControl::new(PlayerControlParts {
                movement: Movement {
                    move_x: 2,
                    move_z: 0,
                    jump: false,
                },
                look: LookAngles::try_new(0.0, 0.0).unwrap(),
                actions: HeldActions {
                    primary: false,
                    eating: false,
                    sprinting: false,
                    sneaking: false,
                },
            })),
        );
        for phase in [RulePhase::PlayerMotion, RulePhase::Interaction] {
            owner.defer(command, phase).unwrap();
        }
        assert_eq!(owner.records, [command]);
        assert_eq!(owner.index.len(), 1);
        for phase in [RulePhase::PlayerMotion, RulePhase::Interaction] {
            assert_eq!(owner.for_phase(phase), [command]);
            assert_eq!(owner.for_phase(phase), [command]);
            assert_eq!(owner.phases[phase as usize], [0]);
        }
    }

    #[test]
    fn complete_scalar_keys_retain_insertion_order() {
        let mut owner = DeferredCommands::default();
        let commands = [
            (3, 2, 7, 0),
            (3, 2, 8, 0),
            (2, 2, 7, 0),
            (3, 1, 7, 0),
            (3, 2, 7, 1),
        ]
        .map(open);
        for command in commands {
            owner.defer(command, RulePhase::Interaction).unwrap();
        }
        assert_eq!(owner.records.len(), commands.len());
        assert_eq!(owner.records, commands);
        assert_eq!(owner.index.len(), commands.len());
        assert_eq!(owner.for_phase(RulePhase::Interaction), commands);
        assert_eq!(owner.for_phase(RulePhase::Interaction), commands);
    }

    #[test]
    fn conflicting_payload_preserves_original_owners_and_roles() {
        let mut owner = DeferredCommands::default();
        let original = open((3, 2, 7, 0));
        owner.defer(original, RulePhase::ContainerMove).unwrap();
        owner
            .defer(original, RulePhase::WorkbenchLifecycle)
            .unwrap();
        let conflicting = envelope(
            (3, 2, 7, 0),
            Command::OpenContainer(LookAngles::try_new(1.0, 0.0).unwrap()),
        );
        assert_eq!(
            owner.defer(conflicting, RulePhase::Interaction),
            Err(ServerError::Internal {
                invariant: "deferred command identity"
            })
        );
        assert_eq!(owner.records, [original]);
        assert_eq!(owner.index.len(), 1);
        assert_eq!(owner.for_phase(RulePhase::ContainerMove), [original]);
        assert_eq!(owner.for_phase(RulePhase::WorkbenchLifecycle), [original]);
        assert!(owner.for_phase(RulePhase::Interaction).is_empty());
        owner.defer(original, RulePhase::Interaction).unwrap();
        assert_eq!(owner.for_phase(RulePhase::Interaction), [original]);
    }

    #[test]
    fn phase_space_uses_real_ordinals_and_default_allocates_no_payload() {
        let mut owner = DeferredCommands::default();
        assert_eq!(PHASE_COUNT, 34);
        assert_eq!(RulePhase::PlayerCommand as usize, 0);
        assert_eq!(RulePhase::EnvironmentEnd as usize, 33);
        assert_eq!(owner.records.capacity(), 0);
        assert!(owner.index.is_empty());
        assert!(owner.phases.iter().all(|phase| phase.capacity() == 0));
        let command = open((0, 1, 1, 0));
        owner.defer(command, RulePhase::PlayerCommand).unwrap();
        owner.defer(command, RulePhase::EnvironmentEnd).unwrap();
        assert_eq!(owner.for_phase(RulePhase::PlayerCommand), [command]);
        assert_eq!(owner.for_phase(RulePhase::EnvironmentEnd), [command]);
        assert_eq!(owner.records, [command]);
    }
}
