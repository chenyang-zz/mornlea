#ifndef MORNLEA_CLIENT_CORE_H
#define MORNLEA_CLIENT_CORE_H

#include <stdint.h>

/*
 * Mornlea client-core ABI: the versioned binary contract between the Go
 * c-shared client core (the producer, built from this package with
 * -buildmode=c-shared) and the mornlea_godot Rust GDExtension (the sole
 * consumer). This header is the single source of truth; the Go producer and
 * the Rust consumer each pin it with independent cross-check tests, so a
 * change here cannot land with only one side updated.
 *
 * This ABI is independent from the Rust engine ABI, the Rust client ABI, and
 * the network protocol. It never shares or repurposes their version numbers.
 * Export function declarations are added by later client-core changes; this
 * file currently defines the identity, status, magic, layout, limit, and
 * feature-family vocabulary those exports must agree on.
 *
 * Versioning rules:
 *   - Compatible additions (new families, new records, new fields that older
 *     readers can skip) raise MORNLEA_CLIENT_ABI_MINOR and the affected
 *     family's contract version.
 *   - Any change to an existing layout or semantic requires a new
 *     MORNLEA_CLIENT_ABI_MAJOR; producers then export both generations in
 *     parallel during migration.
 *   - Existing version numbers, family identifiers, status codes, and magic
 *     tags are never repurposed in place.
 *
 * Wire encoding: every multi-byte integer in a record is little-endian.
 * Every fixed header size is a multiple of MORNLEA_CLIENT_ABI_ALIGNMENT and
 * record buffers are allocated with that alignment, so uint64_t fields stay
 * naturally aligned in any concatenated header-plus-payload sequence;
 * 64-bit fields appear only at byte offsets that are multiples of the
 * alignment. Struct members keep natural C alignment (4 for uint32_t-only
 * headers, 8 for headers carrying uint64_t members), which is identical
 * across the supported desktop ABIs because no member wider than uint64_t
 * and no pointer type is ever used.
 *
 * Two-phase capacity protocol for every variable-length output:
 *   1. The caller queries the required byte count (the same export call with
 *      a zero output capacity; a query never consumes producer state).
 *   2. The caller allocates or reuses a buffer of at least that size.
 *   3. The producer validates and encodes completely into owned bounded
 *      scratch space, then performs one exact write into the caller buffer.
 * MORNLEA_CLIENT_STATUS_INSUFFICIENT_CAPACITY reports the required byte count
 * through the size out-parameter and never writes a partial record set; a
 * consumption-side batch commits only after its successful exact write.
 *
 * Reserved fields: members named `reserved` (and all padding) must be zero on
 * the wire. The producer validates them and rejects non-zero values with
 * MORNLEA_CLIENT_STATUS_INPUT_REJECTED instead of forwarding unknown state.
 *
 * Failure atomicity: no family export writes any output byte on a non-OK
 * status, except reporting the required size for the two-phase capacity
 * signal. Both sides convert panics into MORNLEA_CLIENT_STATUS_PANIC and
 * never unwind across the ABI boundary.
 */

#ifdef __cplusplus
extern "C" {
#endif

/* Client-core ABI identity. The major rises only for incompatible layout or
 * semantic breaks; the minor rises for compatible additions. */
#define MORNLEA_CLIENT_ABI_MAJOR 1u
#define MORNLEA_CLIENT_ABI_MINOR 1u

/*
 * Stable status codes returned by every client-core export. Values are
 * frozen: new codes append at the end, existing codes never change meaning.
 */
/* The call succeeded and every committed output byte is valid. */
#define MORNLEA_CLIENT_STATUS_OK 0u
/* Null, misaligned, overlapping, or oversized pointer and length arguments. */
#define MORNLEA_CLIENT_STATUS_INVALID_ARGUMENT 1u
/* Caller ABI major, magic, or record identity does not match the producer. */
#define MORNLEA_CLIENT_STATUS_ABI_MISMATCH 2u
/* Readable buffer whose content violates the family domain; the whole batch
 * is rejected and no producer state is consumed. */
#define MORNLEA_CLIENT_STATUS_INPUT_REJECTED 3u
/* Two-phase capacity signal: output buffer too small; the required byte count
 * is reported and nothing is written. */
#define MORNLEA_CLIENT_STATUS_INSUFFICIENT_CAPACITY 4u
/* Unknown or wrong-type handle. */
#define MORNLEA_CLIENT_STATUS_INVALID_HANDLE 5u
/* Correct handle in the wrong lifecycle phase or epoch. */
#define MORNLEA_CLIENT_STATUS_INVALID_STATE 6u
/* The session already reached its terminal disconnect. */
#define MORNLEA_CLIENT_STATUS_DISCONNECTED 7u
/* Producer-internal failure without a narrower stable classification. */
#define MORNLEA_CLIENT_STATUS_INTERNAL 8u
/* A recovered panic converted at the ABI boundary; no output is written. */
#define MORNLEA_CLIENT_STATUS_PANIC 9u
/* Number of defined status codes. */
#define MORNLEA_CLIENT_STATUS_COUNT 10u

/*
 * Four-byte record-family magic tags. On the wire the bytes read "MCx1": the
 * low 16 bits are always MORNLEA_CLIENT_MAGIC_TAG_PREFIX (the ASCII bytes
 * "MC" marking the client-core ABI), byte 2 carries one distinct family
 * character, and byte 3 is MORNLEA_CLIENT_MAGIC_GENERATION, the layout-era
 * digit following the engine header's "MGW1"-style precedent. The generation
 * digit moves only with a new ABI major.
 */
/* Shared "MC" prefix occupying the low 16 bits of every tag. */
#define MORNLEA_CLIENT_MAGIC_TAG_PREFIX 0x434Du
/* Layout-era digit occupying the top byte of every tag: ASCII "1". */
#define MORNLEA_CLIENT_MAGIC_GENERATION 0x31u
/* Identity/lifecycle family tag; wire bytes read "MCI1". */
#define MORNLEA_CLIENT_MAGIC_IDENTITY 0x3149434Du
/* Connection family tag; wire bytes read "MCC1". */
#define MORNLEA_CLIENT_MAGIC_CONNECTION 0x3143434Du
/* Input family tag; wire bytes read "MCN1". */
#define MORNLEA_CLIENT_MAGIC_INPUT 0x314E434Du
/* Step family tag; wire bytes read "MCS1". */
#define MORNLEA_CLIENT_MAGIC_STEP 0x3153434Du
/* World family tag; wire bytes read "MCW1". */
#define MORNLEA_CLIENT_MAGIC_WORLD 0x3157434Du
/* Frame family tag; wire bytes read "MCF1". */
#define MORNLEA_CLIENT_MAGIC_FRAME 0x3146434Du
/* Status/metrics family tag; wire bytes read "MCM1". */
#define MORNLEA_CLIENT_MAGIC_STATUS 0x314D434Du

/* Byte alignment of every record buffer and size granularity of every fixed
 * header; see the wire-encoding rules in the header orientation comment. */
#define MORNLEA_CLIENT_ABI_ALIGNMENT 8u

/*
 * Stable feature-family identifiers used by descriptor records and family
 * negotiation. The eight pilot families map one-to-one to the bridge
 * responsibility split (identity/lifecycle, connection, input, step, world,
 * frame, status/metrics). New families append new identifiers; existing
 * identifiers are never reused or reordered.
 */
#define MORNLEA_CLIENT_FAMILY_IDENTITY 1u
#define MORNLEA_CLIENT_FAMILY_CONNECTION 2u
#define MORNLEA_CLIENT_FAMILY_INPUT 3u
#define MORNLEA_CLIENT_FAMILY_STEP 4u
#define MORNLEA_CLIENT_FAMILY_WORLD 5u
#define MORNLEA_CLIENT_FAMILY_FRAME 6u
#define MORNLEA_CLIENT_FAMILY_STATUS 7u
#define MORNLEA_CLIENT_FAMILY_ENVIRONMENT 8u
/* Number of families defined by the pilot contract. */
#define MORNLEA_CLIENT_FAMILY_COUNT 8u

/*
 * Per-family contract versions. A family version rises with any compatible
 * change to that family's records; the ABI major rises instead when an
 * existing layout or semantic changes incompatibly. Value 1 is the pilot
 * generation of every family.
 */
#define MORNLEA_CLIENT_IDENTITY_VERSION 1u
#define MORNLEA_CLIENT_CONNECTION_VERSION 1u
#define MORNLEA_CLIENT_INPUT_VERSION 1u
#define MORNLEA_CLIENT_STEP_VERSION 1u
#define MORNLEA_CLIENT_WORLD_VERSION 1u
/* Existing world-record coordinates: X/Z are signed section coordinates;
 * Y is a zero-based storage index. The world origin is Y * edge - bias.
 * These constants clarify generation-1 semantics without changing wire bytes. */
#define MORNLEA_CLIENT_SECTION_EDGE_BLOCKS 16u
#define MORNLEA_CLIENT_WORLD_Y_BIAS_BLOCKS 64u
#define MORNLEA_CLIENT_FRAME_VERSION 1u
#define MORNLEA_CLIENT_STATUS_VERSION 1u
/* Additive projection of the retained frame; existing frame bytes stay frozen.
 * The 48-byte record contains magic/layout/ready/reserved u32 words,
 * revision/epoch u64 words, then daylight and sky RGB f32 words. */
#define MORNLEA_CLIENT_ENVIRONMENT_VERSION 1u
#define MORNLEA_CLIENT_ENVIRONMENT_BYTES 48u
#define MORNLEA_CLIENT_MAGIC_ENVIRONMENT 0x3145434du

/*
 * Bounded-family limits. Each limit names its provenance: values mirrored
 * from frozen Go constants must stay equal to the platform-independent client
 * core, while client-core-only bounds are pilot decisions that may change
 * only through a reviewed family-version bump.
 */
/* Device events in one input batch. Client-core-only pilot bound: one
 * frame's keys, mouse deltas, text, and action events; a larger batch is
 * rejected whole. */
#define MORNLEA_CLIENT_MAX_INPUT_EVENTS 128u
/* UTF-8 bytes of one "host:port" connection address. Client-core-only pilot
 * bound: a practical size for loopback and ordinary remote addresses that
 * keeps connect requests fixed-max; it does not claim to cover the longest
 * legal hostname. */
#define MORNLEA_CLIENT_MAX_CONNECTION_ADDRESS_BYTES 256u
/* Receiver polls in one step. Mirrors packages/client/runtime
 * MaxStepMessageBudget so a step-driven host observes the same inbound
 * backpressure as the legacy frame loop. */
#define MORNLEA_CLIENT_MAX_STEP_MESSAGE_BUDGET 4096u
/* Ready sections drained in one step. Mirrors the runtime step validation
 * domain, which bounds the mesh budget by the presentation world batch
 * operation limit. */
#define MORNLEA_CLIENT_MAX_STEP_MESH_BUDGET 4096u
/* Upsert plus drop operations in one world batch. Mirrors packages/client/
 * presentation MaxWorldBatchOperations, which bounds validation and host
 * publication work per batch. */
#define MORNLEA_CLIENT_MAX_WORLD_BATCH_OPERATIONS 4096u
/* Packed quads in one section mesh payload. Mirrors presentation
 * MaxSectionMeshQuads, the production mesher's worst case of six faces per
 * block over 4096 blocks per section. */
#define MORNLEA_CLIENT_MAX_SECTION_MESH_QUADS 24576u
/* Packed quads in one whole world batch. Mirrors presentation
 * MaxWorldBatchPackedQuads: the 4 MiB semantic mesh payload bound divided by
 * the 8-byte packed face. */
#define MORNLEA_CLIENT_MAX_WORLD_BATCH_QUADS 524288u
/* Entity records in one frame snapshot. Mirrors presentation
 * MaxEntityBatchRecords, the protocol-aligned maximum number of remote
 * players visible to one client. */
#define MORNLEA_CLIENT_MAX_ENTITY_RECORDS 7u
/* UTF-8 bytes of the frame target-name payload. Client-core-only pilot
 * bound: target block names are short registry identifiers. */
#define MORNLEA_CLIENT_MAX_TARGET_NAME_BYTES 64u
/* Records in one status/metrics pull. Client-core-only pilot bound: bounded
 * stable counters and timing samples per query. */
#define MORNLEA_CLIENT_MAX_STATUS_RECORDS 64u
/* Layout version of the frame snapshot aggregate carried inside frame
 * records. Mirrors packages/client/presentation FrameSnapshotVersion; it is
 * equal to the frame family version in the pilot generation and may diverge
 * when the family adds skippable records without changing the aggregate. */
#define MORNLEA_CLIENT_FRAME_SNAPSHOT_VERSION 1u

/*
 * Fixed record headers. Every variable-length family stream starts with its
 * fixed header; the `layout` member must equal the family's contract version
 * constant above. Member byte offsets are part of the contract and are
 * validated on both sides by the cross-language layout tests.
 */

/*
 * Identity/lifecycle family output header: the producer reports its ABI
 * identity followed by `family_count` MornleaClientFamilyDescriptor records.
 * Offsets: magic 0, layout 4, abi_major 8, abi_minor 12, family_count 16,
 * reserved 20.
 */
typedef struct MornleaClientIdentityHeader {
    uint32_t magic;        /* MORNLEA_CLIENT_MAGIC_IDENTITY */
    uint32_t layout;       /* must equal MORNLEA_CLIENT_IDENTITY_VERSION */
    uint32_t abi_major;    /* producer exports MORNLEA_CLIENT_ABI_MAJOR */
    uint32_t abi_minor;    /* producer exports MORNLEA_CLIENT_ABI_MINOR */
    uint32_t family_count; /* descriptors following; <= MORNLEA_CLIENT_FAMILY_COUNT */
    uint32_t reserved;     /* must be zero; validated */
} MornleaClientIdentityHeader;
#define MORNLEA_CLIENT_IDENTITY_HEADER_BYTES 24u

/*
 * One feature-family descriptor record. Offsets: family 0, version 4,
 * record_limit 8, record_bytes 12, reserved 16 (two u32 padding words).
 */
typedef struct MornleaClientFamilyDescriptor {
    uint32_t family;       /* MORNLEA_CLIENT_FAMILY_* */
    uint32_t version;      /* family contract version */
    uint32_t record_limit; /* max records per batch; 0 = unbounded or not applicable */
    uint32_t record_bytes; /* fixed wire bytes per record; 0 = variable-length records */
    uint32_t reserved[2];  /* must be zero; validated */
} MornleaClientFamilyDescriptor;
#define MORNLEA_CLIENT_FAMILY_DESCRIPTOR_BYTES 24u

/*
 * Connection family request header: begin a TCP login whose variable payload
 * is `address_len` UTF-8 "host:port" bytes. Offsets: magic 0, layout 4,
 * address_len 8, reserved 12.
 */
typedef struct MornleaClientConnectionHeader {
    uint32_t magic;        /* MORNLEA_CLIENT_MAGIC_CONNECTION */
    uint32_t layout;       /* must equal MORNLEA_CLIENT_CONNECTION_VERSION */
    uint32_t address_len;  /* address bytes following; <= MORNLEA_CLIENT_MAX_CONNECTION_ADDRESS_BYTES */
    uint32_t reserved;     /* must be zero; validated */
} MornleaClientConnectionHeader;
#define MORNLEA_CLIENT_CONNECTION_HEADER_BYTES 16u

/*
 * Input family batch header: one per-frame device event stream. Offsets:
 * magic 0, layout 4, event_count 8, reserved 12.
 */
typedef struct MornleaClientInputHeader {
    uint32_t magic;        /* MORNLEA_CLIENT_MAGIC_INPUT */
    uint32_t layout;       /* must equal MORNLEA_CLIENT_INPUT_VERSION */
    uint32_t event_count;  /* events following; <= MORNLEA_CLIENT_MAX_INPUT_EVENTS */
    uint32_t reserved;     /* must be zero; validated */
} MornleaClientInputHeader;
#define MORNLEA_CLIENT_INPUT_HEADER_BYTES 16u

/*
 * Step family request record (fixed size, no variable payload): one bounded
 * step driven only by the explicit elapsed nanoseconds and the two budgets.
 * No implicit wall clock and no network or GPU wait participates in a step.
 * Offsets: magic 0, layout 4, elapsed_ns 8, message_budget 16,
 * mesh_budget 20.
 */
typedef struct MornleaClientStepRequest {
    uint32_t magic;          /* MORNLEA_CLIENT_MAGIC_STEP */
    uint32_t layout;         /* must equal MORNLEA_CLIENT_STEP_VERSION */
    uint64_t elapsed_ns;     /* explicit step elapsed time in nanoseconds */
    uint32_t message_budget; /* <= MORNLEA_CLIENT_MAX_STEP_MESSAGE_BUDGET */
    uint32_t mesh_budget;    /* <= MORNLEA_CLIENT_MAX_STEP_MESH_BUDGET */
} MornleaClientStepRequest;
#define MORNLEA_CLIENT_STEP_REQUEST_BYTES 24u

/*
 * World family output header: one all-or-nothing section batch with monotonic
 * epoch and atlas revision, bounded operation and packed-quad counts,
 * published through the two-phase capacity protocol. Offsets: magic 0,
 * layout 4, operation_count 8, quad_count 12, epoch 16, atlas_revision 24.
 */
typedef struct MornleaClientWorldHeader {
    uint32_t magic;           /* MORNLEA_CLIENT_MAGIC_WORLD */
    uint32_t layout;          /* must equal MORNLEA_CLIENT_WORLD_VERSION */
    uint32_t operation_count; /* <= MORNLEA_CLIENT_MAX_WORLD_BATCH_OPERATIONS */
    uint32_t quad_count;      /* <= MORNLEA_CLIENT_MAX_WORLD_BATCH_QUADS */
    uint64_t epoch;           /* nonzero monotonic publication epoch */
    uint64_t atlas_revision;  /* nonzero monotonic atlas and registry revision */
} MornleaClientWorldHeader;
#define MORNLEA_CLIENT_WORLD_HEADER_BYTES 32u

/*
 * Frame family output header: the per-step semantic snapshot identity, entity
 * record count, and target-name length, followed by the family payload
 * records. At most one frame is published per step. Offsets: magic 0,
 * layout 4, frame_version 8, entity_count 12, name_len 16, reserved 20,
 * revision 24, epoch 32.
 */
typedef struct MornleaClientFrameHeader {
    uint32_t magic;         /* MORNLEA_CLIENT_MAGIC_FRAME */
    uint32_t layout;        /* must equal MORNLEA_CLIENT_FRAME_VERSION */
    uint32_t frame_version; /* must equal MORNLEA_CLIENT_FRAME_SNAPSHOT_VERSION */
    uint32_t entity_count;  /* entity records following; <= MORNLEA_CLIENT_MAX_ENTITY_RECORDS */
    uint32_t name_len;      /* target-name bytes; <= MORNLEA_CLIENT_MAX_TARGET_NAME_BYTES */
    uint32_t reserved;      /* must be zero; validated */
    uint64_t revision;      /* nonzero monotonic frame revision */
    uint64_t epoch;         /* nonzero publication epoch */
} MornleaClientFrameHeader;
#define MORNLEA_CLIENT_FRAME_HEADER_BYTES 40u

/*
 * Status/metrics family output header: a bounded pull of stable error,
 * counter, and timing-sample records. Offsets: magic 0, layout 4,
 * record_count 8, reserved 12.
 */
typedef struct MornleaClientStatusHeader {
    uint32_t magic;        /* MORNLEA_CLIENT_MAGIC_STATUS */
    uint32_t layout;       /* must equal MORNLEA_CLIENT_STATUS_VERSION */
    uint32_t record_count; /* records following; <= MORNLEA_CLIENT_MAX_STATUS_RECORDS */
    uint32_t reserved;     /* must be zero; validated */
} MornleaClientStatusHeader;
#define MORNLEA_CLIENT_STATUS_HEADER_BYTES 16u

#ifdef __cplusplus
}
#endif

#endif
