//! The registered producer-family registry target: the frozen
//! `rust-client-core` descriptor table and its fail-closed validation and
//! symbolic-resolution semantics.

// The crate root keeps `feature_negotiation` private until the later adapter
// node owns the public export, so this registered target includes the module
// sources by path instead of editing shared crate-root files. Everything
// exercised here is the real production source, not a copy.
#![allow(dead_code)]

#[path = "../src/abi.rs"]
mod abi;

#[path = "../src/feature_negotiation.rs"]
mod feature_negotiation;

mod family_registry {
    use super::abi::{FAMILY_COUNT, FAMILY_ENVIRONMENT};
    use super::feature_negotiation::{
        NegotiationReason, PILOT_FAMILIES, ProducerRegistryError, RUST_PRODUCER_FAMILIES,
        RUST_PRODUCER_FAMILY_RECORD_LIMIT, RUST_PRODUCER_NAME, negotiate_producer,
        validate_producer_registry,
    };

    /// Rebuild the frozen producer table for mutation tests.
    fn producer_table() -> Vec<super::feature_negotiation::ProducerFamilyDescriptor> {
        RUST_PRODUCER_FAMILIES.to_vec()
    }

    /// The exact frozen producer table: the producer identity, the ten logical
    /// keys in contract order with IDs 1..10, major 1 / minor 0, and the
    /// per-family record limit mirrored from the accepted client-core limits.
    #[test]
    fn all_ten_keys() {
        let pinned: [(&str, u16); 10] = [
            ("session", 1),
            ("input", 2),
            ("terrain", 3),
            ("actors", 4),
            ("player-view", 5),
            ("inventory-ui", 6),
            ("world-ui", 7),
            ("audio-cues", 8),
            ("lifecycle", 9),
            ("diagnostics", 10),
        ];
        assert_eq!(RUST_PRODUCER_NAME, "rust-client-core");
        assert_eq!(RUST_PRODUCER_FAMILIES.len(), pinned.len());
        for (index, descriptor) in RUST_PRODUCER_FAMILIES.iter().enumerate() {
            let (name, id) = pinned[index];
            assert_eq!(descriptor.logical_name, name, "family at position {index}");
            assert_eq!(descriptor.numeric_id, id, "family {name} numeric id");
            assert_eq!(
                (descriptor.major, descriptor.minor),
                (1, 0),
                "family {name} contract version"
            );
            assert_eq!(
                descriptor.record_limit, RUST_PRODUCER_FAMILY_RECORD_LIMIT,
                "family {name} record limit"
            );
            assert_eq!(descriptor.record_bytes, 0, "family {name} record bytes");
        }
        // The mirrored limit is the accepted per-family-per-frame bound; the
        // literal is the no-silent-change gate against the client-core
        // contract value.
        assert_eq!(RUST_PRODUCER_FAMILY_RECORD_LIMIT, 4096);
        assert_eq!(validate_producer_registry(&RUST_PRODUCER_FAMILIES), Ok(()));
    }

    /// Numeric IDs are producer-scoped: the pilot table keeps its own eight
    /// descriptors, the same numeric value means a different family across
    /// producers, and IDs 9 and 10 exist only in the producer table. Within
    /// the producer, IDs are unique and cover exactly 1..=10.
    #[test]
    fn producer_scoped_ids() {
        assert_eq!(PILOT_FAMILIES.len(), FAMILY_COUNT as usize);
        assert_eq!(PILOT_FAMILIES.iter().map(|d| d.family).max(), Some(8));
        // Pilot ID 8 is the environment projection; producer ID 8 is
        // audio-cues. Neither table derives its meaning from the other.
        assert_eq!(FAMILY_ENVIRONMENT, 8);
        assert_eq!(PILOT_FAMILIES[7].family, FAMILY_ENVIRONMENT);
        assert_eq!(RUST_PRODUCER_FAMILIES[7].logical_name, "audio-cues");
        assert_eq!(RUST_PRODUCER_FAMILIES[7].numeric_id, 8);
        assert_eq!(RUST_PRODUCER_FAMILIES[8].logical_name, "lifecycle");
        assert_eq!(RUST_PRODUCER_FAMILIES[9].logical_name, "diagnostics");

        let mut ids: Vec<u16> = RUST_PRODUCER_FAMILIES
            .iter()
            .map(|d| d.numeric_id)
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, (1..=10).collect::<Vec<u16>>());

        // Two different keys sharing one numeric ID are rejected before any
        // feature resolves against the table.
        let mut table = producer_table();
        table[1].numeric_id = table[0].numeric_id;
        assert_eq!(
            validate_producer_registry(&table),
            Err(ProducerRegistryError::DuplicateFamilyId { numeric_id: 1 })
        );
    }

    /// The registry fails closed: every one of the ten keys is mandatory, and
    /// unknown keys, unknown numeric IDs, duplicate keys, a wrong major and a
    /// minor newer than the registered contract version are all rejected.
    #[test]
    fn missing_and_version_fail_closed() {
        for dropped in 0..producer_table().len() {
            let mut table = producer_table();
            let missing = table[dropped].logical_name;
            table.remove(dropped);
            assert_eq!(
                validate_producer_registry(&table),
                Err(ProducerRegistryError::MissingRequiredFamily {
                    logical_name: missing
                }),
                "table without {missing}"
            );
        }

        // "environment" is a pilot-namespace family name, not a producer key.
        for unknown in ["", "environment", "world", "Session", "audio"] {
            let mut table = producer_table();
            table[0].logical_name = unknown;
            assert_eq!(
                validate_producer_registry(&table),
                Err(ProducerRegistryError::UnknownFamilyKey {
                    logical_name: unknown
                }),
                "table with unknown key {unknown:?}"
            );
            let decision = negotiate_producer(&RUST_PRODUCER_FAMILIES, unknown, 1, 0);
            assert!(!decision.accepted, "unknown key {unknown:?}");
            assert_eq!(decision.reason, NegotiationReason::UnknownFamily);
        }

        // The producer ID space is exactly 1..=10; zero and beyond are unknown.
        for unknown_id in [0, 11, u16::MAX] {
            let mut table = producer_table();
            table[0].numeric_id = unknown_id;
            assert_eq!(
                validate_producer_registry(&table),
                Err(ProducerRegistryError::UnknownFamilyId {
                    numeric_id: unknown_id
                }),
                "table with unknown id {unknown_id}"
            );
        }

        let mut table = producer_table();
        table[1].logical_name = table[0].logical_name;
        assert_eq!(
            validate_producer_registry(&table),
            Err(ProducerRegistryError::DuplicateFamilyKey {
                logical_name: table[0].logical_name
            })
        );

        for descriptor in &RUST_PRODUCER_FAMILIES {
            for major in [0, 2] {
                let decision = negotiate_producer(
                    &RUST_PRODUCER_FAMILIES,
                    descriptor.logical_name,
                    major,
                    descriptor.minor,
                );
                assert!(
                    !decision.accepted,
                    "family {} major {major}",
                    descriptor.logical_name
                );
                assert_eq!(decision.reason, NegotiationReason::MajorMismatch);
            }
            let decision = negotiate_producer(
                &RUST_PRODUCER_FAMILIES,
                descriptor.logical_name,
                descriptor.major,
                descriptor.minor + 1,
            );
            assert!(
                !decision.accepted,
                "family {} too-new minor",
                descriptor.logical_name
            );
            assert_eq!(decision.reason, NegotiationReason::MinorTooNew);
        }
    }

    /// Consumers resolve families by symbolic logical name before feature
    /// instantiation: audio-cues and lifecycle resolve at the registered
    /// version, and resolution is fail-closed for unknown names.
    #[test]
    fn symbolic_audio_lifecycle_resolution() {
        for name in ["audio-cues", "lifecycle"] {
            let decision = negotiate_producer(&RUST_PRODUCER_FAMILIES, name, 1, 0);
            assert!(decision.accepted, "{name} at 1.0");
            assert_eq!(decision.reason, NegotiationReason::Compatible);
            let descriptor = RUST_PRODUCER_FAMILIES
                .iter()
                .find(|d| d.logical_name == name)
                .expect("registered family");
            let expected_id = if name == "audio-cues" { 8 } else { 9 };
            assert_eq!(descriptor.numeric_id, expected_id);
        }
        let decision = negotiate_producer(&RUST_PRODUCER_FAMILIES, "audio-cues", 1, 1);
        assert!(!decision.accepted);
        assert_eq!(decision.reason, NegotiationReason::MinorTooNew);
        let decision = negotiate_producer(&RUST_PRODUCER_FAMILIES, "lifecycle", 2, 0);
        assert!(!decision.accepted);
        assert_eq!(decision.reason, NegotiationReason::MajorMismatch);
    }
}
