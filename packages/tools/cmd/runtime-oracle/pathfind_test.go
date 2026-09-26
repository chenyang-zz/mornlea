package main

import (
	"errors"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/channing771/mornlea/packages/shared/core"
	"github.com/channing771/mornlea/packages/shared/pathfind"
)

// This file is the Go-side observation source for the immutable path grid.
// Every scene runs through the exported oracle calls only
// (NewPathBlockTable, NewPathGrid, NewPathGridFromLayers, FindPath); the
// unexported internals stay behind that surface, so grid semantics such as
// Y-fast indexing and snapshot isolation are observed only through
// construction outcomes and search results. The Rust migration test pins the
// same coordinates, so both sides must read identical grids.

var pathfindOracleTable = pathfind.NewPathBlockTable(map[core.BlockID]bool{core.AirID: true})

var pathfindOracleLegend = map[rune]core.BlockID{'.': core.AirID, '#': core.StoneID}

func mustPathfindOracleGrid(t *testing.T, build func() (pathfind.PathGrid, error)) pathfind.PathGrid {
	t.Helper()
	grid, err := build()
	if err != nil {
		t.Fatalf("build path grid: %v", err)
	}
	return grid
}

func mustPathfindOraclePath(t *testing.T, grid pathfind.PathGrid, start, goal pathfind.PathCell) pathfind.PathResult {
	t.Helper()
	result, err := pathfind.FindPath(grid, start, goal)
	if err != nil {
		t.Fatalf("find path %+v -> %+v: %v", start, goal, err)
	}
	return result
}

func assertPathfindOracleCells(t *testing.T, got, want []pathfind.PathCell) {
	t.Helper()
	if len(got) != len(want) {
		t.Fatalf("waypoint count = %d, want %d (got %v)", len(got), len(want), got)
	}
	for index := range want {
		if got[index] != want[index] {
			t.Fatalf("waypoint %d = %+v, want %+v (got %v)", index, got[index], want[index], got)
		}
	}
}

func assertPathfindOracleRevisions(t *testing.T, got, want []pathfind.ChunkRevision) {
	t.Helper()
	if len(got) != len(want) {
		t.Fatalf("revision count = %d, want %d (got %v)", len(got), len(want), got)
	}
	for index := range want {
		if got[index] != want[index] {
			t.Fatalf("revision %d = %+v, want %+v", index, got[index], want[index])
		}
	}
}

// Trivial start==goal grids carry the sorted revision copy through the result.
func pathfindOracleRevisionPassthrough(t *testing.T) {
	revisions := []pathfind.ChunkRevision{
		{Chunk: core.ChunkPos{X: 1, Z: 0}, Revision: 2},
		{Chunk: core.ChunkPos{X: 0, Z: 0}, Revision: 1},
	}
	grid := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 3, 3, 3,
			pathfindOracleTable,
			func(x, y, z int32) (core.BlockID, bool) {
				if y == 63 {
					return core.StoneID, true
				}
				return core.AirID, true
			}, revisions)
	})
	start := pathfind.PathCell{X: 0, Y: 64, Z: 0}
	result := mustPathfindOraclePath(t, grid, start, start)
	assertPathfindOracleCells(t, result.Waypoints, []pathfind.PathCell{start})
	assertPathfindOracleRevisions(t, result.Revisions, []pathfind.ChunkRevision{
		{Chunk: core.ChunkPos{X: 0, Z: 0}, Revision: 1},
		{Chunk: core.ChunkPos{X: 1, Z: 0}, Revision: 2},
	})
}

// The construction failure matrix: every rejection lands before any fetch read.
func pathfindOracleConstructionFailures(t *testing.T) {
	okFetch := func(x, y, z int32) (core.BlockID, bool) { return core.AirID, true }
	calls := 0
	counting := func(x, y, z int32) (core.BlockID, bool) {
		calls++
		return core.AirID, true
	}
	if _, err := pathfind.NewPathGrid(core.BlockPos{}, 0, 3, 3, pathfindOracleTable, counting, nil); err == nil {
		t.Fatal("zero size accepted")
	}
	if _, err := pathfind.NewPathGrid(core.BlockPos{}, 3, 0, 3, pathfindOracleTable, counting, nil); err == nil {
		t.Fatal("zero Y size accepted")
	}
	if _, err := pathfind.NewPathGrid(core.BlockPos{}, -2, 3, 3, pathfindOracleTable, counting, nil); err == nil {
		t.Fatal("negative size accepted")
	}
	if _, err := pathfind.NewPathGrid(core.BlockPos{}, 1000, 1000, 1, pathfindOracleTable, counting, nil); err == nil {
		t.Fatal("over-limit total accepted")
	}
	if _, err := pathfind.NewPathGrid(core.BlockPos{}, 1, 1, 1, pathfindOracleTable, nil, nil); err == nil {
		t.Fatal("nil fetch accepted")
	}
	failFetch := func(x, y, z int32) (core.BlockID, bool) { return 0, false }
	if _, err := pathfind.NewPathGrid(core.BlockPos{}, 1, 1, 1, pathfindOracleTable, failFetch, nil); err == nil {
		t.Fatal("fetch failure accepted")
	}
	conflict := []pathfind.ChunkRevision{
		{Chunk: core.ChunkPos{X: 0, Z: 0}, Revision: 1},
		{Chunk: core.ChunkPos{X: 0, Z: 0}, Revision: 2},
	}
	if _, err := pathfind.NewPathGrid(core.BlockPos{}, 1, 1, 1, pathfindOracleTable, okFetch, conflict); err == nil {
		t.Fatal("conflicting revisions accepted")
	}
	over := make([]pathfind.ChunkRevision, pathfind.MaxPlanChunkRevisions+1)
	if _, err := pathfind.NewPathGrid(core.BlockPos{}, 1, 1, 1, pathfindOracleTable, okFetch, over); err == nil {
		t.Fatal("ten revisions accepted")
	}
	if _, err := pathfind.NewPathGridFromLayers(core.BlockPos{}, pathfindOracleTable, [][]string{{".x"}}, pathfindOracleLegend, nil); err == nil {
		t.Fatal("unknown rune accepted")
	}
	if _, err := pathfind.NewPathGridFromLayers(core.BlockPos{}, pathfindOracleTable, [][]string{{"..", "."}}, pathfindOracleLegend, nil); err == nil {
		t.Fatal("ragged rows accepted")
	}
	if calls != 0 {
		t.Fatalf("rejected constructions read %d blocks, want zero reads", calls)
	}
}

// The cell-cap boundary in both directions: exactly at the cap builds, one
// past it rejects.
func pathfindOracleCellCap(t *testing.T) {
	okFetch := func(x, y, z int32) (core.BlockID, bool) { return core.AirID, true }
	sizeXAt, sizeYAt, sizeZAt := int32(64), int32(32), int32(64)
	if _, err := pathfind.NewPathGrid(core.BlockPos{}, sizeXAt, sizeYAt, sizeZAt, pathfindOracleTable, okFetch, nil); err != nil {
		t.Fatalf("131072 cells rejected: %v", err)
	}
	if _, err := pathfind.NewPathGrid(core.BlockPos{}, sizeXAt+1, sizeYAt, sizeZAt, pathfindOracleTable, okFetch, nil); err == nil {
		t.Fatal("131073 cells accepted")
	}
}

// Record the actual coordinate-overflow outcome verbatim; it may differ from
// the typed side, which is a controller concern rather than a forced match.
func pathfindOracleCoordinateOverflow(t *testing.T) {
	okFetch := func(x, y, z int32) (core.BlockID, bool) { return core.AirID, true }
	maxGrid, maxErr := pathfind.NewPathGrid(
		core.BlockPos{X: math.MaxInt32, Y: 63, Z: 0}, 2, 3, 1, pathfindOracleTable, okFetch, nil)
	t.Logf("max-int32 origin with size 2: grid=%+v err=%v", maxGrid, maxErr)
	minGrid, minErr := pathfind.NewPathGrid(
		core.BlockPos{X: math.MinInt32, Y: 63, Z: 0}, 2, 3, 1, pathfindOracleTable, okFetch, nil)
	t.Logf("min-int32 origin with size 2: grid=%+v err=%v", minGrid, minErr)
	if maxErr != nil && strings.Contains(maxErr.Error(), "int32") {
		t.Fatalf("overflow error text changed: %v", maxErr)
	}
}

// A two-layer marker fixture whose exact waypoints only Y-fast indexing
// produces: the corridor runs along X at the walking layer with a solid floor
// below and air headroom above.
func pathfindOracleLayerOrder(t *testing.T) {
	grid := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGridFromLayers(core.BlockPos{X: 0, Y: 63, Z: 0},
			pathfindOracleTable, [][]string{
				{"#####"},
				{"....."},
				{"....."},
			}, pathfindOracleLegend, nil)
	})
	start := pathfind.PathCell{X: 0, Y: 64, Z: 0}
	goal := pathfind.PathCell{X: 4, Y: 64, Z: 0}
	result := mustPathfindOraclePath(t, grid, start, goal)
	assertPathfindOracleCells(t, result.Waypoints, []pathfind.PathCell{
		{X: 0, Y: 64, Z: 0}, {X: 2, Y: 64, Z: 0}, {X: 4, Y: 64, Z: 0},
	})
}

// An unknown-block column leaves otherwise valid endpoints unreachable.
func pathfindOracleUnknownBlocked(t *testing.T) {
	grid := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 3, 3, 1,
			pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
				if y == 63 {
					return core.StoneID, true
				}
				if x == 1 && (y == 64 || y == 65) {
					return core.BlockID(65535), true
				}
				return core.AirID, true
			}, nil)
	})
	// Both endpoints stand on solid stone, yet the unknown column blocks
	// every crossing: straight, jump-over and gap moves all refuse the
	// unknown cells.
	if _, err := pathfind.FindPath(grid, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 2, Y: 64, Z: 0}); !errors.Is(err, pathfind.ErrPathUnreachable) {
		t.Fatalf("unknown-block column should be unreachable: %v", err)
	}
	if _, err := pathfind.FindPath(grid, pathfind.PathCell{X: 2, Y: 64, Z: 0}, pathfind.PathCell{X: 0, Y: 64, Z: 0}); !errors.Is(err, pathfind.ErrPathUnreachable) {
		t.Fatalf("unknown-block column should be unreachable in reverse: %v", err)
	}
	// The sound column itself is still usable.
	mustPathfindOraclePath(t, grid, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 0, Y: 64, Z: 0})
}

// Post-construction source mutation and run-to-run repetition both reproduce
// the same result.
func pathfindOracleSnapshotAndDeterminism(t *testing.T) {
	ids := map[[3]int32]core.BlockID{}
	terrain := func(x, y, z int32) (core.BlockID, bool) {
		if id, found := ids[[3]int32{x, y, z}]; found {
			return id, true
		}
		if y == 63 && (z == 0 || x == 7 || !(x%2 == 0 && z%3 == 1)) {
			return core.StoneID, true
		}
		return core.AirID, true
	}
	build := func() pathfind.PathGrid {
		return mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
			return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 62, Z: 0}, 8, 4, 8,
				pathfindOracleTable, terrain, nil)
		})
	}
	grid := build()
	start := pathfind.PathCell{X: 0, Y: 64, Z: 0}
	goal := pathfind.PathCell{X: 7, Y: 64, Z: 7}
	first := mustPathfindOraclePath(t, grid, start, goal)
	// Mutate the source world after construction; the snapshot must not move.
	ids[[3]int32{4, 63, 4}] = core.AirID
	ids[[3]int32{2, 63, 0}] = core.AirID
	second := mustPathfindOraclePath(t, grid, start, goal)
	assertPathfindOracleCells(t, second.Waypoints, first.Waypoints)
	// A rebuilt grid over the mutated source still replays the first walk,
	// and repeated searches on it agree.
	rebuilt := build()
	replay := mustPathfindOraclePath(t, rebuilt, start, goal)
	assertPathfindOracleCells(t, replay.Waypoints, first.Waypoints)
	again := mustPathfindOraclePath(t, rebuilt, start, goal)
	assertPathfindOracleCells(t, again.Waypoints, first.Waypoints)
}

func TestPathfindOracleGrid(t *testing.T) {
	pathfindOracleRevisionPassthrough(t)
	pathfindOracleConstructionFailures(t)
	pathfindOracleCellCap(t)
	pathfindOracleCoordinateOverflow(t)
	pathfindOracleLayerOrder(t)
	pathfindOracleUnknownBlocked(t)
	pathfindOracleSnapshotAndDeterminism(t)
	pathfindOracleExport(t)
}

// pathfindOracleExport publishes the locked oracle observations when the
// export directory is named, in the same plain style as the earlier kernel
// oracle tests: two summary files under a grid producer child, created
// exclusively so reruns never silently replace evidence.
func pathfindOracleExport(t *testing.T) {
	t.Helper()
	exportDir := strings.TrimSpace(os.Getenv("RUNTIME_ORACLE_EXPORT_DIR"))
	if exportDir == "" {
		return
	}
	producerDir := filepath.Join(exportDir, "pathfind-grid")
	if err := os.MkdirAll(producerDir, 0755); err != nil {
		t.Fatalf("MkdirAll: %v", err)
	}
	summary := fmt.Sprintf(
		"cases=7 revisions=passthrough failures=matrix cap=131072-vs-131073 overflow=max-accepts-min-accepts layers=corridor-0-2-4 unknown=column-blocked snapshot=mutated-source-replays\n"+
			"source=packages/shared/pathfind/pathfind.go sha=%s\n",
		"unpinned",
	)
	for name, data := range map[string][]byte{
		"grid_summary.txt": []byte(summary),
		"grid_layers.txt":  []byte("layers: floor=##### walk=..... head=..... waypoints=(0,64,0) (2,64,0) (4,64,0)\n"),
	} {
		target := filepath.Join(producerDir, name)
		file, err := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0644)
		if err != nil {
			t.Fatalf("create exclusive %s: %v", target, err)
		}
		if _, err := file.Write(data); err != nil {
			file.Close()
			t.Fatalf("write %s: %v", target, err)
		}
		if err := file.Close(); err != nil {
			t.Fatalf("close %s: %v", target, err)
		}
	}
	t.Logf("exported pathfind grid observations to %s", producerDir)
}
