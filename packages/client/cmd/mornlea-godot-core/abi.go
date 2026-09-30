//go:build cgo

package main

/*
#cgo CFLAGS: -I${SRCDIR}/include
#include "mornlea_client_core.h"
*/
import "C"

import "unsafe"

// The constants below are derived from include/mornlea_client_core.h through
// cgo, so this package and the mornlea_godot Rust consumer compile against one
// header. The tests in abi_test.go pin the values with explicit literals, so a
// co-version bump of the header and these constants without review fails.

// Status is the stable client-core ABI status code returned by every export.
// Values are frozen; new codes append and none is repurposed.
type Status uint32

// Family is one stable client-core ABI feature-family identifier.
type Family uint32

// Magic is a four-byte record-family magic tag whose wire bytes read "MCx1".
type Magic uint32

// Client-core ABI identity. A compatible addition raises `ABIMinor` and the
// affected family version; an incompatible layout or semantic change requires
// a new `ABIMajor`.
const (
	ABIMajor = uint32(C.MORNLEA_CLIENT_ABI_MAJOR)
	ABIMinor = uint32(C.MORNLEA_CLIENT_ABI_MINOR)
)

// Status codes. `StatusInsufficientCapacity` is the two-phase capacity signal:
// it reports the required byte count and never writes a partial record set.
// No family export writes any output byte on any non-OK status.
const (
	StatusOK                   Status = Status(C.MORNLEA_CLIENT_STATUS_OK)
	StatusInvalidArgument      Status = Status(C.MORNLEA_CLIENT_STATUS_INVALID_ARGUMENT)
	StatusABIMismatch          Status = Status(C.MORNLEA_CLIENT_STATUS_ABI_MISMATCH)
	StatusInputRejected        Status = Status(C.MORNLEA_CLIENT_STATUS_INPUT_REJECTED)
	StatusInsufficientCapacity Status = Status(C.MORNLEA_CLIENT_STATUS_INSUFFICIENT_CAPACITY)
	StatusInvalidHandle        Status = Status(C.MORNLEA_CLIENT_STATUS_INVALID_HANDLE)
	StatusInvalidState         Status = Status(C.MORNLEA_CLIENT_STATUS_INVALID_STATE)
	StatusDisconnected         Status = Status(C.MORNLEA_CLIENT_STATUS_DISCONNECTED)
	StatusInternal             Status = Status(C.MORNLEA_CLIENT_STATUS_INTERNAL)
	StatusPanic                Status = Status(C.MORNLEA_CLIENT_STATUS_PANIC)
	StatusCount                Status = Status(C.MORNLEA_CLIENT_STATUS_COUNT)
)

// Magic tags. Each composes `MagicTagPrefix`, one distinct family character,
// and `MagicGeneration`; the generation digit moves only with a new major.
const (
	MagicTagPrefix  Magic = Magic(C.MORNLEA_CLIENT_MAGIC_TAG_PREFIX)
	MagicGeneration Magic = Magic(C.MORNLEA_CLIENT_MAGIC_GENERATION)
	MagicIdentity   Magic = Magic(C.MORNLEA_CLIENT_MAGIC_IDENTITY)
	MagicConnection Magic = Magic(C.MORNLEA_CLIENT_MAGIC_CONNECTION)
	MagicInput      Magic = Magic(C.MORNLEA_CLIENT_MAGIC_INPUT)
	MagicStep       Magic = Magic(C.MORNLEA_CLIENT_MAGIC_STEP)
	MagicWorld      Magic = Magic(C.MORNLEA_CLIENT_MAGIC_WORLD)
	MagicFrame      Magic = Magic(C.MORNLEA_CLIENT_MAGIC_FRAME)
	MagicStatus     Magic = Magic(C.MORNLEA_CLIENT_MAGIC_STATUS)
)

// ABIAlignment is the byte alignment of every record buffer and the size
// granularity of every fixed header, keeping embedded uint64 fields naturally
// aligned in any header-plus-payload sequence.
const ABIAlignment = uint32(C.MORNLEA_CLIENT_ABI_ALIGNMENT)

// Feature-family identifiers for descriptor records and family negotiation.
// New families append; existing identifiers are never reused or reordered.
const (
	FamilyIdentity    Family = Family(C.MORNLEA_CLIENT_FAMILY_IDENTITY)
	FamilyConnection  Family = Family(C.MORNLEA_CLIENT_FAMILY_CONNECTION)
	FamilyInput       Family = Family(C.MORNLEA_CLIENT_FAMILY_INPUT)
	FamilyStep        Family = Family(C.MORNLEA_CLIENT_FAMILY_STEP)
	FamilyWorld       Family = Family(C.MORNLEA_CLIENT_FAMILY_WORLD)
	FamilyFrame       Family = Family(C.MORNLEA_CLIENT_FAMILY_FRAME)
	FamilyStatus      Family = Family(C.MORNLEA_CLIENT_FAMILY_STATUS)
	FamilyEnvironment Family = Family(C.MORNLEA_CLIENT_FAMILY_ENVIRONMENT)
	FamilyCount       Family = Family(C.MORNLEA_CLIENT_FAMILY_COUNT)
)

// Per-family contract versions. A version rises with any compatible change to
// that family's records; value 1 is the pilot generation.
const (
	IdentityVersion    = uint32(C.MORNLEA_CLIENT_IDENTITY_VERSION)
	ConnectionVersion  = uint32(C.MORNLEA_CLIENT_CONNECTION_VERSION)
	InputVersion       = uint32(C.MORNLEA_CLIENT_INPUT_VERSION)
	StepVersion        = uint32(C.MORNLEA_CLIENT_STEP_VERSION)
	WorldVersion       = uint32(C.MORNLEA_CLIENT_WORLD_VERSION)
	SectionEdgeBlocks  = uint32(C.MORNLEA_CLIENT_SECTION_EDGE_BLOCKS)
	WorldYBiasBlocks   = uint32(C.MORNLEA_CLIENT_WORLD_Y_BIAS_BLOCKS)
	FrameVersion       = uint32(C.MORNLEA_CLIENT_FRAME_VERSION)
	StatusVersion      = uint32(C.MORNLEA_CLIENT_STATUS_VERSION)
	EnvironmentVersion = uint32(C.MORNLEA_CLIENT_ENVIRONMENT_VERSION)
	EnvironmentBytes   = uint32(C.MORNLEA_CLIENT_ENVIRONMENT_BYTES)
	MagicEnvironment   = uint32(C.MORNLEA_CLIENT_MAGIC_ENVIRONMENT)
)

// Bounded-family limits. The world, step, entity, and frame-snapshot limits
// mirror frozen packages/client/presentation and packages/client/runtime
// constants; abi_test.go keeps both sides equal. The input, connection
// address, target-name, and status-record limits are client-core pilot
// bounds defined by this header alone.
const (
	MaxInputEvents            = uint32(C.MORNLEA_CLIENT_MAX_INPUT_EVENTS)
	MaxConnectionAddressBytes = uint32(C.MORNLEA_CLIENT_MAX_CONNECTION_ADDRESS_BYTES)
	MaxStepMessageBudget      = uint32(C.MORNLEA_CLIENT_MAX_STEP_MESSAGE_BUDGET)
	MaxStepMeshBudget         = uint32(C.MORNLEA_CLIENT_MAX_STEP_MESH_BUDGET)
	MaxWorldBatchOperations   = uint32(C.MORNLEA_CLIENT_MAX_WORLD_BATCH_OPERATIONS)
	MaxSectionMeshQuads       = uint32(C.MORNLEA_CLIENT_MAX_SECTION_MESH_QUADS)
	MaxWorldBatchQuads        = uint32(C.MORNLEA_CLIENT_MAX_WORLD_BATCH_QUADS)
	MaxEntityRecords          = uint32(C.MORNLEA_CLIENT_MAX_ENTITY_RECORDS)
	MaxTargetNameBytes        = uint32(C.MORNLEA_CLIENT_MAX_TARGET_NAME_BYTES)
	MaxStatusRecords          = uint32(C.MORNLEA_CLIENT_MAX_STATUS_RECORDS)
	FrameSnapshotVersion      = uint32(C.MORNLEA_CLIENT_FRAME_SNAPSHOT_VERSION)
)

// Fixed-header wire sizes in bytes. Every size is a multiple of
// `ABIAlignment`.
const (
	IdentityHeaderBytes   = uint32(C.MORNLEA_CLIENT_IDENTITY_HEADER_BYTES)
	FamilyDescriptorBytes = uint32(C.MORNLEA_CLIENT_FAMILY_DESCRIPTOR_BYTES)
	ConnectionHeaderBytes = uint32(C.MORNLEA_CLIENT_CONNECTION_HEADER_BYTES)
	InputHeaderBytes      = uint32(C.MORNLEA_CLIENT_INPUT_HEADER_BYTES)
	StepRequestBytes      = uint32(C.MORNLEA_CLIENT_STEP_REQUEST_BYTES)
	WorldHeaderBytes      = uint32(C.MORNLEA_CLIENT_WORLD_HEADER_BYTES)
	FrameHeaderBytes      = uint32(C.MORNLEA_CLIENT_FRAME_HEADER_BYTES)
	StatusHeaderBytes     = uint32(C.MORNLEA_CLIENT_STATUS_HEADER_BYTES)
)

// headerLayoutFacts derives every fixed-header size, struct alignment, and
// member offset from the cgo-included C structs. cgo cannot be used in test
// files, so the derived facts live here and abi_test.go compares them with the
// declared constants and the pinned offsets; the Rust abi module mirrors the
// same numbers from the header text.
func headerLayoutFacts() map[string]uintptr {
	var (
		identityHeader   C.MornleaClientIdentityHeader
		familyDescriptor C.MornleaClientFamilyDescriptor
		connectionHeader C.MornleaClientConnectionHeader
		inputHeader      C.MornleaClientInputHeader
		stepRequest      C.MornleaClientStepRequest
		worldHeader      C.MornleaClientWorldHeader
		frameHeader      C.MornleaClientFrameHeader
		statusHeader     C.MornleaClientStatusHeader
	)
	return map[string]uintptr{
		"identity header size":                  unsafe.Sizeof(identityHeader),
		"family descriptor size":                unsafe.Sizeof(familyDescriptor),
		"connection header size":                unsafe.Sizeof(connectionHeader),
		"input header size":                     unsafe.Sizeof(inputHeader),
		"step request size":                     unsafe.Sizeof(stepRequest),
		"world header size":                     unsafe.Sizeof(worldHeader),
		"frame header size":                     unsafe.Sizeof(frameHeader),
		"status header size":                    unsafe.Sizeof(statusHeader),
		"identity header align":                 unsafe.Alignof(identityHeader),
		"family descriptor align":               unsafe.Alignof(familyDescriptor),
		"connection header align":               unsafe.Alignof(connectionHeader),
		"input header align":                    unsafe.Alignof(inputHeader),
		"step request align":                    unsafe.Alignof(stepRequest),
		"world header align":                    unsafe.Alignof(worldHeader),
		"frame header align":                    unsafe.Alignof(frameHeader),
		"status header align":                   unsafe.Alignof(statusHeader),
		"identity header magic offset":          unsafe.Offsetof(identityHeader.magic),
		"identity header abi_major offset":      unsafe.Offsetof(identityHeader.abi_major),
		"identity header family_count offset":   unsafe.Offsetof(identityHeader.family_count),
		"identity header reserved offset":       unsafe.Offsetof(identityHeader.reserved),
		"family descriptor family offset":       unsafe.Offsetof(familyDescriptor.family),
		"family descriptor record_limit offset": unsafe.Offsetof(familyDescriptor.record_limit),
		"family descriptor record_bytes offset": unsafe.Offsetof(familyDescriptor.record_bytes),
		"family descriptor reserved offset":     unsafe.Offsetof(familyDescriptor.reserved),
		"connection header address_len offset":  unsafe.Offsetof(connectionHeader.address_len),
		"input header event_count offset":       unsafe.Offsetof(inputHeader.event_count),
		"step request elapsed_ns offset":        unsafe.Offsetof(stepRequest.elapsed_ns),
		"step request message_budget offset":    unsafe.Offsetof(stepRequest.message_budget),
		"step request mesh_budget offset":       unsafe.Offsetof(stepRequest.mesh_budget),
		"world header operation_count offset":   unsafe.Offsetof(worldHeader.operation_count),
		"world header quad_count offset":        unsafe.Offsetof(worldHeader.quad_count),
		"world header epoch offset":             unsafe.Offsetof(worldHeader.epoch),
		"world header atlas_revision offset":    unsafe.Offsetof(worldHeader.atlas_revision),
		"frame header frame_version offset":     unsafe.Offsetof(frameHeader.frame_version),
		"frame header entity_count offset":      unsafe.Offsetof(frameHeader.entity_count),
		"frame header name_len offset":          unsafe.Offsetof(frameHeader.name_len),
		"frame header reserved offset":          unsafe.Offsetof(frameHeader.reserved),
		"frame header revision offset":          unsafe.Offsetof(frameHeader.revision),
		"frame header epoch offset":             unsafe.Offsetof(frameHeader.epoch),
		"status header record_count offset":     unsafe.Offsetof(statusHeader.record_count),
	}
}
