// Go/Rust parity fixtures for the companion planning projection.
//
// Each case drives the production buildPlanSnapshot over a real runtime.Engine
// whose ready chunks were installed through the ordinary acquisition path, then
// records the inputs next to the Go projection: dense terrain planes, exposed
// blocks, chunk revisions, online players, inventory, task status, world time,
// the source tick and the canonical terrain and snapshot digests. The Rust authority test
// `core::companion_planning` rebuilds the same inputs and must reproduce every
// recorded value. Set MORNLEA_WRITE_COMPANION_PLANNING_FIXTURES=1 to rewrite
// the files; otherwise the test fails when the committed files drift from the
// current Go projection.
package server

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/channing771/mornlea/packages/server/sim/contract"
	"github.com/channing771/mornlea/packages/server/sim/runtime"
	"github.com/channing771/mornlea/packages/shared/companion"
	"github.com/channing771/mornlea/packages/shared/core"
	"github.com/channing771/mornlea/packages/shared/world"
)

const companionPlanningFixtureDir = "testdata/companion_planning"

type planningFixture struct {
	Name     string                  `json:"name"`
	Input    planningFixtureInput    `json:"input"`
	Expected planningFixtureExpected `json:"expected"`
}

type planningFixtureInput struct {
	Dimension core.DimensionID `json:"dimension"`
	// Ticks is the number of completed engine steps when the snapshot is
	// built; chunk acquisition must settle within it.
	Ticks          uint64                   `json:"ticks"`
	WorldTimeTicks uint64                   `json:"worldTimeTicks"`
	Task           string                   `json:"task"`
	Command        string                   `json:"command"`
	Issuer         planningFixturePlayer    `json:"issuer"`
	Companion      planningFixtureCompanion `json:"companion"`
	Players        []planningFixturePlayer  `json:"players"`
	Chunks         []planningFixtureChunk   `json:"chunks"`
}

type planningFixturePlayer struct {
	ID       string     `json:"id"`
	Position [3]float32 `json:"position"`
	Yaw      float32    `json:"yaw"`
	Pitch    float32    `json:"pitch"`
	LookHit  *[3]int32  `json:"lookHit"`
}

type planningFixtureCompanion struct {
	ID        string                `json:"id"`
	Position  [3]float32            `json:"position"`
	Yaw       float32               `json:"yaw"`
	Pitch     float32               `json:"pitch"`
	Inventory []planningFixtureSlot `json:"inventory"`
}

type planningFixtureSlot struct {
	Slot       uint8  `json:"slot"`
	Item       uint16 `json:"item"`
	Count      uint8  `json:"count"`
	Durability uint16 `json:"durability"`
}

// planningFixtureLayer fills every column of a chunk from FromY to ToY inclusive.
type planningFixtureLayer struct {
	FromY int32  `json:"fromY"`
	ToY   int32  `json:"toY"`
	Block uint16 `json:"block"`
}

// planningFixtureChunk is one ready chunk: layers apply first, then the
// explicit world-coordinate blocks in order. Chunks absent from the list are
// never made ready.
type planningFixtureChunk struct {
	X        int32                  `json:"x"`
	Z        int32                  `json:"z"`
	Revision uint64                 `json:"revision"`
	Layers   []planningFixtureLayer `json:"layers"`
	Blocks   [][4]int32             `json:"blocks"`
}

type planningFixtureRevision struct {
	X        int32  `json:"x"`
	Z        int32  `json:"z"`
	Revision uint64 `json:"revision"`
}

type planningFixtureExpected struct {
	Origin          [3]int32                  `json:"origin"`
	ReadyColumnsB64 string                    `json:"readyColumnsB64"`
	HeightsBEI16B64 string                    `json:"heightsBEI16B64"`
	BlocksBEU16B64  string                    `json:"blocksBEU16B64"`
	ReadyColumns    int                       `json:"readyColumns"`
	ExposedBlocks   [][4]int32                `json:"exposedBlocks"`
	ChunkRevisions  []planningFixtureRevision `json:"chunkRevisions"`
	OnlinePlayers   []planningFixturePlayer   `json:"onlinePlayers"`
	Inventory       []planningFixtureSlot     `json:"inventory"`
	TaskStatus      string                    `json:"taskStatus"`
	WorldTimeTicks  uint64                    `json:"worldTimeTicks"`
	// SourceTick is the authority tick at the planning boundary. PlanSnapshot
	// carries no tick; dispatchPlanning runs before engine.StepWithTunables,
	// where the authority tick is engine.TickCount(), the completed step count.
	SourceTick     uint64 `json:"sourceTick"`
	TerrainSHA256  string `json:"terrainSha256"`
	SnapshotSHA256 string `json:"snapshotSha256"`
}

func TestCompanionPlanningProjectionFixtures(t *testing.T) {
	write := os.Getenv("MORNLEA_WRITE_COMPANION_PLANNING_FIXTURES") == "1"
	for _, input := range companionPlanningFixtureInputs() {
		t.Run(input.Name, func(t *testing.T) {
			fixture := input
			fixture.Expected = buildPlanningFixtureExpected(t, fixture.Input)
			encoded, err := json.MarshalIndent(fixture, "", "  ")
			if err != nil {
				t.Fatalf("encode fixture: %v", err)
			}
			encoded = append(encoded, '\n')
			path := filepath.Join(companionPlanningFixtureDir, fixture.Name+".json")
			if write {
				if err := os.MkdirAll(companionPlanningFixtureDir, 0o755); err != nil {
					t.Fatalf("create fixture dir: %v", err)
				}
				if err := os.WriteFile(path, encoded, 0o644); err != nil {
					t.Fatalf("write fixture: %v", err)
				}
				return
			}
			committed, err := os.ReadFile(path)
			if err != nil {
				t.Fatalf("read committed fixture: %v", err)
			}
			if !bytes.Equal(committed, encoded) {
				t.Fatalf("%s drifted from the Go projection; rerun with MORNLEA_WRITE_COMPANION_PLANNING_FIXTURES=1", path)
			}
		})
	}
}

// buildPlanningFixtureExpected runs the production buildPlanSnapshot and
// records every projected part.
func buildPlanningFixtureExpected(t *testing.T, input planningFixtureInput) planningFixtureExpected {
	t.Helper()
	engine := readyPlanningFixtureEngine(t, input)
	companionID, err := companion.ParseID(input.Companion.ID)
	if err != nil {
		t.Fatalf("companion id: %v", err)
	}
	manager := &companionManager{
		engine: engine,
		slots: map[companion.ID]*companionTaskSlot{
			companionID: planningFixtureSlot_(t, input),
		},
		onlinePlayers: func() []companion.PlanPlayer {
			players := make([]companion.PlanPlayer, 0, len(input.Players))
			for _, player := range input.Players {
				players = append(players, planningFixturePlanPlayer(t, player))
			}
			return companion.BoundOnlinePlayers(players)
		},
	}
	issuer := planningFixturePlanPlayer(t, input.Issuer)
	body := companion.Body{
		ID:        companionID,
		Dimension: input.Dimension,
		Position:  input.Companion.Position,
		Yaw:       input.Companion.Yaw,
		Pitch:     input.Companion.Pitch,
		Inventory: planningFixtureInventory(t, input.Companion.Inventory),
	}
	sourceTick := engine.TickCount()
	snapshot, err := manager.buildPlanSnapshot(
		companion.Definition{ID: companionID, Name: "Nova"},
		companion.TaskCommand(input.Command),
		companionTaskIssuer{
			playerID:   issuer.ID,
			name:       "Issuer",
			position:   issuer.Position,
			yaw:        issuer.Yaw,
			pitch:      issuer.Pitch,
			lookHit:    issuer.LookHit,
			hasLookHit: issuer.HasLookHit,
		},
		body,
	)
	if err != nil {
		t.Fatalf("buildPlanSnapshot: %v", err)
	}

	terrainJSON, err := companion.CanonicalTerrainDigest(snapshot.Terrain)
	if err != nil {
		t.Fatalf("terrain digest: %v", err)
	}
	var terrain struct {
		BlocksBEU16B64  string `json:"blocks_be_u16_b64"`
		HeightsBEI16B64 string `json:"heights_be_i16_b64"`
		ReadyColumnsB64 string `json:"ready_columns_b64"`
	}
	if err := json.Unmarshal(terrainJSON, &terrain); err != nil {
		t.Fatalf("decode terrain digest: %v", err)
	}
	terrainSum := sha256.Sum256(terrainJSON)
	_, snapshotSum, err := companion.CanonicalSnapshotDigest(snapshot)
	if err != nil {
		t.Fatalf("snapshot digest: %v", err)
	}

	origin := snapshot.Terrain.Origin()
	expected := planningFixtureExpected{
		Origin:          [3]int32{origin.X, origin.Y, origin.Z},
		ReadyColumnsB64: terrain.ReadyColumnsB64,
		HeightsBEI16B64: terrain.HeightsBEI16B64,
		BlocksBEU16B64:  terrain.BlocksBEU16B64,
		ReadyColumns:    len(snapshot.Heights),
		ExposedBlocks:   make([][4]int32, 0, len(snapshot.ExposedBlocks)),
		ChunkRevisions:  make([]planningFixtureRevision, 0, len(snapshot.ChunkRevisions)),
		OnlinePlayers:   make([]planningFixturePlayer, 0, len(snapshot.OnlinePlayers)),
		Inventory:       make([]planningFixtureSlot, 0, core.InventorySlots),
		TaskStatus:      snapshot.Companion.TaskStatus,
		WorldTimeTicks:  snapshot.WorldTimeTicks,
		SourceTick:      sourceTick,
		TerrainSHA256:   hex.EncodeToString(terrainSum[:]),
		SnapshotSHA256:  snapshotSum,
	}
	for _, block := range snapshot.ExposedBlocks {
		expected.ExposedBlocks = append(expected.ExposedBlocks,
			[4]int32{block.Pos.X, block.Pos.Y, block.Pos.Z, int32(block.Block)})
	}
	for _, revision := range snapshot.ChunkRevisions {
		expected.ChunkRevisions = append(expected.ChunkRevisions, planningFixtureRevision{
			X: revision.Chunk.X, Z: revision.Chunk.Z, Revision: revision.Revision,
		})
	}
	for _, player := range snapshot.OnlinePlayers {
		recorded := planningFixturePlayer{
			ID: player.ID.String(), Position: player.Position, Yaw: player.Yaw, Pitch: player.Pitch,
		}
		if player.HasLookHit {
			recorded.LookHit = &[3]int32{player.LookHit.X, player.LookHit.Y, player.LookHit.Z}
		}
		expected.OnlinePlayers = append(expected.OnlinePlayers, recorded)
	}
	for slot := uint8(0); slot < core.InventorySlots; slot++ {
		stack, _ := snapshot.Companion.Inventory.Slot(slot)
		if stack.Item == core.ItemNone {
			continue
		}
		expected.Inventory = append(expected.Inventory, planningFixtureSlot{
			Slot: slot, Item: uint16(stack.Item), Count: stack.Count, Durability: stack.Durability,
		})
	}
	return expected
}

// readyPlanningFixtureEngine makes exactly the fixture chunks Ready through
// the engine acquisition path and leaves every other wanted chunk Loading.
func readyPlanningFixtureEngine(t *testing.T, input planningFixtureInput) *runtime.Engine {
	t.Helper()
	engine := runtime.NewEngine(2, 0, 0, core.DifficultyPeaceful)
	center := (core.BlockPos{
		X: planningFixtureFloor(input.Companion.Position[0]),
		Z: planningFixtureFloor(input.Companion.Position[2]),
	}).Chunk()
	engine.RegisterSession(1, input.Dimension, center)
	chunks := make(map[core.ChunkKey]planningFixtureChunk, len(input.Chunks))
	for _, chunk := range input.Chunks {
		chunks[core.ChunkKey{Dimension: input.Dimension, Pos: core.ChunkPos{X: chunk.X, Z: chunk.Z}}] = chunk
	}
	for engine.TickCount() < input.Ticks {
		result := engine.Step()
		if planningFixtureChunksReady(engine, chunks) {
			continue
		}
		for _, key := range result.Acquire {
			chunk, ok := chunks[key]
			if !ok {
				continue
			}
			engine.SubmitAcquired(contract.AcquiredChunk{
				Key:               key,
				Chunk:             buildPlanningFixtureChunk(t, chunk),
				Revision:          chunk.Revision,
				PersistedRevision: chunk.Revision,
			})
		}
	}
	if engine.TickCount() != input.Ticks {
		t.Fatalf("engine completed %d steps, want %d", engine.TickCount(), input.Ticks)
	}
	for key, chunk := range chunks {
		cloned, revision, ready := engine.CloneReadyChunk(key)
		if !ready {
			t.Fatalf("fixture chunk %+v never became ready", key.Pos)
		}
		if revision != chunk.Revision || cloned.Hash() != buildPlanningFixtureChunk(t, chunk).Hash() {
			t.Fatalf("fixture chunk %+v changed while the engine settled (revision %d)", key.Pos, revision)
		}
	}
	engine.SetWorldTimeForTest(input.WorldTimeTicks)
	return engine
}

func planningFixtureChunksReady(engine *runtime.Engine, chunks map[core.ChunkKey]planningFixtureChunk) bool {
	for key := range chunks {
		if info, ok := engine.ChunkInfo(key); !ok || info.State != contract.ChunkReady {
			return false
		}
	}
	return true
}

func buildPlanningFixtureChunk(t *testing.T, input planningFixtureChunk) *world.Chunk {
	t.Helper()
	chunk := world.NewChunk(core.ChunkPos{X: input.X, Z: input.Z})
	for _, layer := range input.Layers {
		for y := layer.FromY; y <= layer.ToY; y++ {
			for lz := 0; lz < core.SectionSize; lz++ {
				for lx := 0; lx < core.SectionSize; lx++ {
					chunk.SetBlock(lx, y, lz, core.BlockID(layer.Block))
				}
			}
		}
	}
	for _, block := range input.Blocks {
		pos := core.BlockPos{X: block[0], Y: block[1], Z: block[2]}
		if pos.Chunk() != (core.ChunkPos{X: input.X, Z: input.Z}) {
			t.Fatalf("fixture block %+v outside chunk (%d,%d)", pos, input.X, input.Z)
		}
		chunk.SetBlock(int(pos.X&core.SectionMask), pos.Y, int(pos.Z&core.SectionMask), core.BlockID(block[3]))
	}
	chunk.Compact()
	return chunk
}

// planningFixtureSlot_ drives a real task queue into the requested state.
func planningFixtureSlot_(t *testing.T, input planningFixtureInput) *companionTaskSlot {
	t.Helper()
	slot := &companionTaskSlot{}
	if !slot.queue.Enqueue(companion.TaskCommand(input.Command)) || !slot.queue.BeginHead() {
		t.Fatal("fixture task enqueue failed")
	}
	if input.Task == "queued" {
		return slot
	}
	if !slot.queue.BeginPlanning() {
		t.Fatal("fixture task planning failed")
	}
	if input.Task == "planning" {
		return slot
	}
	slot.queue.AcceptPlan(companion.Plan{
		Summary: "fixture",
		Steps:   []companion.PlanStep{{Kind: companion.PlanStepGoTo, X: 1, Y: 64, Z: 1}},
	})
	slot.queue.FinishValidation(input.WorldTimeTicks, 10)
	if current, ok := slot.queue.Current(); !ok || current.State != companion.TaskRunning {
		t.Fatal("fixture task never reached running")
	}
	return slot
}

func planningFixturePlanPlayer(t *testing.T, input planningFixturePlayer) companion.PlanPlayer {
	t.Helper()
	id, err := core.ParsePlayerID(input.ID)
	if err != nil {
		t.Fatalf("player id: %v", err)
	}
	player := companion.PlanPlayer{ID: id, Position: input.Position, Yaw: input.Yaw, Pitch: input.Pitch}
	if input.LookHit != nil {
		player.LookHit = core.BlockPos{X: input.LookHit[0], Y: input.LookHit[1], Z: input.LookHit[2]}
		player.HasLookHit = true
	}
	return player
}

func planningFixtureInventory(t *testing.T, slots []planningFixtureSlot) core.Inventory {
	t.Helper()
	var inventory core.Inventory
	for _, slot := range slots {
		stack := core.ItemStack{Item: core.ItemID(slot.Item), Count: slot.Count, Durability: slot.Durability}
		if !stack.Valid() || slot.Slot >= core.InventorySlots {
			t.Fatalf("fixture inventory slot %+v invalid", slot)
		}
		if slot.Slot < core.HotbarSlots {
			inventory.Hotbar.Slots[slot.Slot] = stack
		} else {
			inventory.Backpack[slot.Slot-core.HotbarSlots] = stack
		}
	}
	return inventory
}

func planningFixtureFloor(value float32) int32 {
	floor := int32(value)
	if float32(floor) > value {
		floor--
	}
	return floor
}

func companionPlanningFixtureInputs() []planningFixture {
	block := func(x, y, z int32, id core.BlockID) [4]int32 { return [4]int32{x, y, z, int32(id)} }
	surface := []planningFixtureLayer{
		{FromY: core.MinY, ToY: 59, Block: uint16(core.StoneID)},
		{FromY: 60, ToY: 62, Block: uint16(core.DirtID)},
		{FromY: 63, ToY: 63, Block: uint16(core.GrassID)},
	}
	return []planningFixture{
		{
			Name: "running_surface_capped",
			Input: planningFixtureInput{
				Dimension:      core.Overworld,
				Ticks:          12,
				WorldTimeTicks: 30123,
				Task:           "running",
				Command:        "把木头砍下来",
				Issuer: planningFixturePlayer{
					ID: "1a2b3c4d-0000-4000-8000-000000000001", Position: [3]float32{2.5, 64, -7.75},
					Yaw: 90, Pitch: -12.5, LookHit: &[3]int32{5, 65, -6},
				},
				Companion: planningFixtureCompanion{
					ID: "c0ffee00-0000-4000-8000-0000000000aa", Position: [3]float32{8.75, 64, -3.25},
					Yaw: -45.5, Pitch: 3.25,
					Inventory: []planningFixtureSlot{
						{Slot: 0, Item: uint16(core.ItemStonePickaxe), Count: 1, Durability: 77},
						{Slot: 4, Item: uint16(core.ItemCobblestone), Count: 12},
						{Slot: 35, Item: uint16(core.ItemOakPlanks), Count: 64},
					},
				},
				Players: []planningFixturePlayer{
					{ID: "ffffffff-0000-4000-8000-000000000003", Position: [3]float32{-30.5, 70, 12.25}, Yaw: 180, Pitch: 0},
					{ID: "1a2b3c4d-0000-4000-8000-000000000001", Position: [3]float32{4.5, 64, -5.5}, Yaw: 10.5, Pitch: -2},
					{ID: "00000001-0000-4000-8000-000000000002", Position: [3]float32{9.25, 63.5, -1}, Yaw: -0.5, Pitch: 45},
				},
				Chunks: []planningFixtureChunk{
					{X: -1, Z: -2, Revision: 11, Layers: surface},
					{X: -1, Z: -1, Revision: 12, Layers: surface},
					{X: 0, Z: -2, Revision: 21, Layers: surface},
					{X: 0, Z: -1, Revision: 22, Layers: surface, Blocks: [][4]int32{
						block(8, 63, -3, core.AirID),
						block(8, 62, -3, core.AirID),
						block(5, 64, -6, core.OakLogID),
						block(5, 65, -6, core.OakLogID),
						block(5, 66, -6, core.OakLogID),
						block(5, 67, -6, core.LeavesID),
						block(4, 66, -6, core.LeavesID),
						block(6, 66, -6, core.LeavesID),
						block(10, 64, -3, core.GlassID),
						block(12, 72, -9, core.CobblestoneID),
						block(12, 73, -9, core.CobblestoneID),
						block(1, 55, -16, core.GravelID),
					}},
					{X: 0, Z: 0, Revision: 23, Layers: surface},
					{X: 1, Z: -1, Revision: 32, Layers: surface},
					{X: 1, Z: 0, Revision: 33, Layers: surface, Blocks: [][4]int32{
						block(24, 64, 12, core.OakPlanksID),
					}},
				},
			},
		},
		{
			Name: "planning_world_bottom_sparse",
			Input: planningFixtureInput{
				Dimension:      core.Overworld,
				Ticks:          25,
				WorldTimeTicks: 7,
				Task:           "planning",
				Command:        "在这里挖一个坑",
				Issuer: planningFixturePlayer{
					ID: "0f0f0f0f-0000-4000-8000-0000000000f0", Position: [3]float32{-12, -58, 25.5},
					Yaw: -90, Pitch: 30, LookHit: &[3]int32{-21, -59, 30},
				},
				Companion: planningFixtureCompanion{
					ID: "c0ffee00-0000-4000-8000-0000000000bb", Position: [3]float32{-20.5, -60, 30.25},
					Yaw: 0, Pitch: 0,
				},
				Chunks: []planningFixtureChunk{
					{X: -3, Z: 0, Revision: 1, Blocks: [][4]int32{
						block(-37, core.MinY, 14, core.BedrockID),
					}},
					{X: -3, Z: 1, Revision: 2},
					{X: -2, Z: 0, Revision: 3},
					{X: -2, Z: 1, Revision: 4, Blocks: [][4]int32{
						block(-21, core.MinY, 30, core.BedrockID),
						block(-21, -59, 30, core.StoneID),
						block(-21, -58, 30, core.SandID),
						block(-22, -59, 30, core.StoneID),
						block(-20, -52, 31, core.GlassID),
						block(-17, -61, 27, core.CobblestoneID),
					}},
					{X: -2, Z: 2, Revision: 5, Blocks: [][4]int32{
						block(-25, -55, 46, core.StoneBrickID),
					}},
					{X: -1, Z: 0, Revision: 6, Layers: []planningFixtureLayer{{FromY: -50, ToY: -50, Block: uint16(core.StoneID)}}},
					{X: -1, Z: 1, Revision: 7},
					{X: -1, Z: 2, Revision: 8, Blocks: [][4]int32{
						block(-5, core.MinY, 46, core.BedrockID),
						block(-5, core.MinY+1, 46, core.BedrockID),
						block(-6, core.MinY, 46, core.BedrockID),
					}},
				},
			},
		},
		{
			Name: "queued_negative_world_top",
			Input: planningFixtureInput{
				Dimension:      core.Overworld,
				Ticks:          40,
				WorldTimeTicks: 1<<40 + 7,
				Task:           "queued",
				Command:        "跟着我",
				Issuer: planningFixturePlayer{
					ID: "22222222-0000-4000-8000-000000000002", Position: [3]float32{0, 1, 0},
				},
				Companion: planningFixtureCompanion{
					ID: "c0ffee00-0000-4000-8000-0000000000cc", Position: [3]float32{-0.5, 315, -16},
					Yaw: 359.5, Pitch: -89.75,
					Inventory: []planningFixtureSlot{
						{Slot: 8, Item: uint16(core.ItemTorch), Count: 3},
						{Slot: 9, Item: uint16(core.ItemCobblestone), Count: 1},
					},
				},
				Players: []planningFixturePlayer{
					{ID: "88888888-0000-4000-8000-000000000008", Position: [3]float32{8, 8, 8}},
					{ID: "77777777-0000-4000-8000-000000000007", Position: [3]float32{7, 7, 7}},
					{ID: "66666666-0000-4000-8000-000000000006", Position: [3]float32{6, 6, 6}},
					{ID: "55555555-0000-4000-8000-000000000005", Position: [3]float32{5, 5, 5}},
					{ID: "44444444-0000-4000-8000-000000000004", Position: [3]float32{4, 4, 4}},
					{ID: "33333333-0000-4000-8000-000000000003", Position: [3]float32{3, 3, 3}},
					{ID: "22222222-0000-4000-8000-000000000002", Position: [3]float32{-1.5, 316, -15}, Yaw: 1, Pitch: 2},
					{ID: "11111111-0000-4000-8000-000000000001", Position: [3]float32{1, 1, 1}},
				},
				Chunks: []planningFixtureChunk{
					{X: -2, Z: -2, Revision: 101, Layers: surface},
					{X: -2, Z: -1, Revision: 102, Layers: surface},
					{X: -2, Z: 0, Revision: 103, Layers: surface},
					{X: -1, Z: -2, Revision: 104, Layers: []planningFixtureLayer{{FromY: 319, ToY: 319, Block: uint16(core.GlassID)}}},
					{X: -1, Z: -1, Revision: 105, Layers: surface, Blocks: [][4]int32{
						block(-1, 319, -16, core.StoneID),
						block(-2, 318, -16, core.StoneID),
						block(-1, 314, -16, core.OakPlanksID),
						block(-16, 307, -1, core.CobblestoneID),
					}},
					{X: -1, Z: 0, Revision: 106},
					{X: 0, Z: -2, Revision: 107, Layers: surface},
					{X: 0, Z: -1, Revision: 1 << 62, Layers: surface},
					{X: 0, Z: 0, Revision: 109, Layers: []planningFixtureLayer{{FromY: 308, ToY: 308, Block: uint16(core.LeavesID)}}},
				},
			},
		},
	}
}
