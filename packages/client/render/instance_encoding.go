package render

import (
	"encoding/binary"
	"math"

	"github.com/go-gl/mathgl/mgl32"

	"github.com/channing771/mornlea/packages/client/assets"
	"github.com/channing771/mornlea/packages/shared/core"
)

// `InstanceEncoder` retains reusable CPU buffers and presentation anchors.
// `Reset` the anchors at session boundaries; no GPU resources are owned here.
type InstanceEncoder struct {
	ordered    []Avatar
	parts      []avatarPart
	bursts     BreakBursts
	falls      DropFalls
	burstBytes []byte
	kickBytes  []byte
	snowKicks  SnowKicks
	tracks     map[EntityKey]swingTrack
}

// `swingTrack` stores confirmed-tick locomotion samples and pending same-tick distance.
type swingTrack struct {
	pos      [3]float32
	tick     uint64
	speed    float32
	distance float32
	pending  float32
}

// `EncodeAvatarInstances` encodes sorted bodies as 96-byte instances.
// Confirmed ticks and accumulated XZ movement drive swings. Same-tick samples
// retain pending distance; tick rollback and jumps beyond eight blocks reset
// the anchor. Stationary bodies retain their neutral pose.
func (e *InstanceEncoder) EncodeAvatarInstances(dst []byte, tick uint64, avatars []Avatar) []byte {
	e.ordered = orderedAvatarsInto(e.ordered[:0], avatars)
	e.applyLocomotionSwing(tick)
	e.parts = buildOrderedAvatarParts(e.parts[:0], e.ordered)
	dst = growEncodeBuffer(dst, len(e.parts)*avatarInstanceBytes)
	encodeAvatarPartsInto(dst, e.parts)
	return dst
}

// applyLocomotionSwing updates per-entity distance and drops departed entities.
func (e *InstanceEncoder) applyLocomotionSwing(tick uint64) {
	if e.tracks == nil {
		e.tracks = make(map[EntityKey]swingTrack, len(e.ordered))
	}
	for index := range e.ordered {
		avatar := &e.ordered[index]
		track := swingTrack{pos: [3]float32(avatar.Position), tick: tick}
		if last, ok := e.tracks[avatar.Key]; ok && tick >= last.tick {
			delta := avatar.Position.Sub(mgl32.Vec3(last.pos))
			if delta.LenSqr() <= 64 {
				moved := float32(math.Hypot(float64(delta[0]), float64(delta[2])))
				track.distance = last.distance + moved
				track.speed = last.speed
				track.pending = last.pending + moved
				if tick > last.tick {
					track.speed = track.pending / float32(tick-last.tick)
					track.pending = 0
				}
			}
		}
		e.tracks[avatar.Key] = track
		avatar.Swing = avatarKindSwingAngle(avatar.Key.Kind, track.distance, swingPhaseID(avatar.Key), track.speed)
	}
	if len(e.tracks) > len(e.ordered) {
		seen := make(map[EntityKey]struct{}, len(e.ordered))
		for index := range e.ordered {
			seen[e.ordered[index].Key] = struct{}{}
		}
		for key := range e.tracks {
			if _, ok := seen[key]; !ok {
				delete(e.tracks, key)
			}
		}
	}
}

// `ResetLocomotion` clears presentation distance, speed, and tick anchors.
func (e *InstanceEncoder) ResetLocomotion() {
	clear(e.tracks)
}

// `EncodeItemDropInstances` encodes item drops and advances their presentation fall anchors.
func (e *InstanceEncoder) EncodeItemDropInstances(dst []byte, serverTick uint64, drops []ItemDrop, gravity, terminal float32) []byte {
	e.parts = e.falls.buildItemDropParts(e.parts[:0], serverTick, drops, gravity, terminal)
	dst = growEncodeBuffer(dst, len(e.parts)*avatarInstanceBytes)
	encodeAvatarPartsInto(dst, e.parts)
	return dst
}

// `EncodeBreakBurstInstances` replaces dst with the current 96-byte burst instances.
func (e *InstanceEncoder) EncodeBreakBurstInstances(dst []byte, serverTick uint64, drops []ItemDrop) []byte {
	e.parts = e.bursts.BuildParts(e.parts[:0], serverTick, drops)
	dst = growEncodeBuffer(dst, len(e.parts)*avatarInstanceBytes)
	encodeAvatarPartsInto(dst, e.parts)
	return dst
}

// `AppendBreakBurstInstances` appends bursts after any existing instance stream.
// Capacity reuse keeps both paths allocation-free after their buffers grow.
func (e *InstanceEncoder) AppendBreakBurstInstances(dst []byte, serverTick uint64, drops []ItemDrop) []byte {
	e.parts = e.bursts.BuildParts(e.parts[:0], serverTick, drops)
	budget := maxAvatarParts*avatarInstanceBytes - len(dst)
	if budget < 0 {
		budget = 0
	}
	keep := len(e.parts)
	if max := budget / avatarInstanceBytes; keep > max {
		keep = max
	}
	keep -= keep % breakBurstParticlesPerBurst
	tail := e.parts[len(e.parts)-keep:]
	e.burstBytes = growEncodeBuffer(e.burstBytes, len(tail)*avatarInstanceBytes)
	encodeAvatarPartsInto(e.burstBytes, tail)
	return append(dst, e.burstBytes...)
}

// `ResetBursts` clears burst anchors at a session or scene boundary.
func (e *InstanceEncoder) ResetBursts() {
	e.bursts.Reset()
}

// `ResetFalls` clears item-drop presentation fall anchors.
func (e *InstanceEncoder) ResetFalls() {
	e.falls.Reset()
}

// `EncodeWeatherInstances` encodes precipitation from confirmed weather and season values.
func (e *InstanceEncoder) EncodeWeatherInstances(dst []byte, cam mgl32.Vec3, yaw float32, serverTick uint64, kind core.WeatherKind, yearPhase float64, effPhase uint16) []byte {
	if kind != core.WeatherRain && kind != core.WeatherThunder {
		return dst[:0]
	}
	e.parts = BuildWeatherParts(e.parts[:0], cam, yaw, serverTick, kind, yearPhase, effPhase)
	dst = growEncodeBuffer(dst, len(e.parts)*avatarInstanceBytes)
	encodeAvatarPartsInto(dst, e.parts)
	return dst
}

// `AppendSnowKickInstances` appends locally derived snow particles to the weather stream.
func (e *InstanceEncoder) AppendSnowKickInstances(dst []byte, serverTick uint64, input SnowKickInput) []byte {
	e.parts = e.snowKicks.BuildParts(e.parts[:0], serverTick, input)
	budget := WeatherMaxParticles*avatarInstanceBytes - len(dst)
	if budget < 0 {
		budget = 0
	}
	keep := len(e.parts)
	if max := budget / avatarInstanceBytes; keep > max {
		keep = max
	}
	tail := e.parts[len(e.parts)-keep:]
	e.kickBytes = growEncodeBuffer(e.kickBytes, len(tail)*avatarInstanceBytes)
	encodeAvatarPartsInto(e.kickBytes, tail)
	return append(dst, e.kickBytes...)
}

// `ResetSnowKicks` clears local particle anchors at a session boundary.
func (e *InstanceEncoder) ResetSnowKicks() {
	e.snowKicks.Reset()
}

// `EncodeBlockOutlineInstances` encodes a CPU-derived target outline as 96-byte instances.
func (e *InstanceEncoder) EncodeBlockOutlineInstances(dst []byte, outline BlockOutline) []byte {
	if !outline.Visible {
		return dst[:0]
	}
	e.parts = buildBlockOutlineParts(e.parts[:0], outline.Position)
	dst = growEncodeBuffer(dst, len(e.parts)*avatarInstanceBytes)
	encodeAvatarPartsInto(dst, e.parts)
	return dst
}

// `EncodeBlockCrackInstances` encodes a bounded target crack projection; zero progress produces no instances.
func (e *InstanceEncoder) EncodeBlockCrackInstances(dst []byte, crack BlockCrack) []byte {
	if !crack.valid() {
		return dst[:0]
	}
	dst = growEncodeBuffer(dst, blockCrackInstanceBytes)
	part := buildBlockCrackPart(crack.Position)
	for index, value := range part.transform {
		binary.LittleEndian.PutUint32(dst[index*4:], math.Float32bits(value))
	}
	binary.LittleEndian.PutUint32(
		dst[64:], math.Float32bits(float32(int(assets.LayerCrack0)+crack.Stage)),
	)
	clear(dst[68:blockCrackInstanceBytes])
	return dst
}

// `EncodeBillboardCameraBytes` writes the camera basis in the established native billboard layout.
func EncodeBillboardCameraBytes(dst []byte, camera BillboardCamera) []byte {
	dst = growEncodeBuffer(dst, nameTagCameraBytes)
	encodeBillboardCamera(dst, camera)
	return dst
}
