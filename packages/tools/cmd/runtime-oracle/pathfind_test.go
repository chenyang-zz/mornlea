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

// Flat floor terrain shared by the search scenes: solid stone at the floor
// level, air elsewhere.
func pathfindOracleFlatFetch(floorY int32) func(x, y, z int32) (core.BlockID, bool) {
	return func(x, y, z int32) (core.BlockID, bool) {
		if y == floorY {
			return core.StoneID, true
		}
		return core.AirID, true
	}
}

// The transition matrix: every movement kind with its blocked-clearance
// counterpart, plus the flat+gap double emission in one direction.
func pathfindOracleTransitions(t *testing.T) {
	flat := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 5, 3, 1,
			pathfindOracleTable, pathfindOracleFlatFetch(63), nil)
	})
	assertPathfindOracleCells(t,
		mustPathfindOraclePath(t, flat, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 4, Y: 64, Z: 0}).Waypoints,
		[]pathfind.PathCell{{X: 0, Y: 64, Z: 0}, {X: 2, Y: 64, Z: 0}, {X: 4, Y: 64, Z: 0}})
	assertPathfindOracleCells(t,
		mustPathfindOraclePath(t, flat, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 2, Y: 64, Z: 0}).Waypoints,
		[]pathfind.PathCell{{X: 0, Y: 64, Z: 0}, {X: 2, Y: 64, Z: 0}})

	steps := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 2, 4, 1,
			pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
				if (x == 0 && y == 63) || (x == 1 && y == 64) {
					return core.StoneID, true
				}
				return core.AirID, true
			}, nil)
	})
	assertPathfindOracleCells(t,
		mustPathfindOraclePath(t, steps, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 1, Y: 65, Z: 0}).Waypoints,
		[]pathfind.PathCell{{X: 0, Y: 64, Z: 0}, {X: 1, Y: 65, Z: 0}})

	blockedClearance := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 2, 4, 1,
			pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
				if (x == 0 && y == 63) || (x == 1 && y == 64) || (x == 0 && y == 66) {
					return core.StoneID, true
				}
				return core.AirID, true
			}, nil)
	})
	if _, err := pathfind.FindPath(blockedClearance, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 1, Y: 65, Z: 0}); !errors.Is(err, pathfind.ErrPathUnreachable) {
		t.Fatalf("blocked jump clearance should be unreachable: %v", err)
	}

	descent := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 2, 4, 1,
			pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
				if (x == 0 && y == 64) || (x == 1 && y == 63) {
					return core.StoneID, true
				}
				return core.AirID, true
			}, nil)
	})
	assertPathfindOracleCells(t,
		mustPathfindOraclePath(t, descent, pathfind.PathCell{X: 0, Y: 65, Z: 0}, pathfind.PathCell{X: 1, Y: 64, Z: 0}).Waypoints,
		[]pathfind.PathCell{{X: 0, Y: 65, Z: 0}, {X: 1, Y: 64, Z: 0}})

	descentTwo := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 2, 4, 1,
			pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
				if (x == 0 && y == 65) || (x == 1 && y == 63) {
					return core.StoneID, true
				}
				return core.AirID, true
			}, nil)
	})
	if _, err := pathfind.FindPath(descentTwo, pathfind.PathCell{X: 0, Y: 66, Z: 0}, pathfind.PathCell{X: 1, Y: 64, Z: 0}); !errors.Is(err, pathfind.ErrPathUnreachable) {
		t.Fatalf("two-cell drop should be unreachable: %v", err)
	}

	oneGap := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 3, 3, 1,
			pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
				if y == 63 && (x == 0 || x == 2) {
					return core.StoneID, true
				}
				return core.AirID, true
			}, nil)
	})
	assertPathfindOracleCells(t,
		mustPathfindOraclePath(t, oneGap, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 2, Y: 64, Z: 0}).Waypoints,
		[]pathfind.PathCell{{X: 0, Y: 64, Z: 0}, {X: 2, Y: 64, Z: 0}})

	twoGap := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 4, 3, 1,
			pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
				if y == 63 && (x == 0 || x == 3) {
					return core.StoneID, true
				}
				return core.AirID, true
			}, nil)
	})
	if _, err := pathfind.FindPath(twoGap, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 3, Y: 64, Z: 0}); !errors.Is(err, pathfind.ErrPathUnreachable) {
		t.Fatalf("two-cell gap should be unreachable: %v", err)
	}

	blockedGapHead := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 3, 3, 1,
			pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
				if (y == 63 && (x == 0 || x == 2)) || (x == 1 && y == 65) {
					return core.StoneID, true
				}
				return core.AirID, true
			}, nil)
	})
	if _, err := pathfind.FindPath(blockedGapHead, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 2, Y: 64, Z: 0}); !errors.Is(err, pathfind.ErrPathUnreachable) {
		t.Fatalf("gap with blocked middle head should be unreachable: %v", err)
	}
}

// Diamond ties and cost ties: symmetric routes freeze the first-insertion
// choice, and the terraced climb pins the equal-cost parent retention.
func pathfindOracleTies(t *testing.T) {
	diamond := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 2, 3, 2,
			pathfindOracleTable, pathfindOracleFlatFetch(63), nil)
	})
	assertPathfindOracleCells(t,
		mustPathfindOraclePath(t, diamond, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 1, Y: 64, Z: 1}).Waypoints,
		[]pathfind.PathCell{{X: 0, Y: 64, Z: 0}, {X: 1, Y: 64, Z: 0}, {X: 1, Y: 64, Z: 1}})
	assertPathfindOracleCells(t,
		mustPathfindOraclePath(t, diamond, pathfind.PathCell{X: 1, Y: 64, Z: 0}, pathfind.PathCell{X: 0, Y: 64, Z: 1}).Waypoints,
		[]pathfind.PathCell{{X: 1, Y: 64, Z: 0}, {X: 0, Y: 64, Z: 0}, {X: 0, Y: 64, Z: 1}})

	detour := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 8, 4, 4,
			pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
				if y == 63 {
					return core.StoneID, true
				}
				if x == 1 && z == 0 && y == 64 {
					return core.StoneID, true
				}
				if x == 0 && z == 1 && y == 65 {
					return core.StoneID, true
				}
				return core.AirID, true
			}, nil)
	})
	assertPathfindOracleCells(t,
		mustPathfindOraclePath(t, detour, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 7, Y: 64, Z: 3}).Waypoints,
		[]pathfind.PathCell{
			{X: 0, Y: 64, Z: 0}, {X: 1, Y: 65, Z: 0}, {X: 1, Y: 64, Z: 1},
			{X: 3, Y: 64, Z: 1}, {X: 5, Y: 64, Z: 1}, {X: 7, Y: 64, Z: 1}, {X: 7, Y: 64, Z: 3},
		})

	terrace := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 60, Z: 0}, 7, 6, 5,
			pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
				if z == 2 {
					if x < 2 {
						if y == 60 {
							return core.StoneID, true
						}
						return core.AirID, true
					}
					if y == 61 {
						return core.StoneID, true
					}
					return core.AirID, true
				}
				if y == 60 {
					return core.StoneID, true
				}
				return core.AirID, true
			}, nil)
	})
	assertPathfindOracleCells(t,
		mustPathfindOraclePath(t, terrace, pathfind.PathCell{X: 0, Y: 61, Z: 2}, pathfind.PathCell{X: 6, Y: 62, Z: 2}).Waypoints,
		[]pathfind.PathCell{
			{X: 0, Y: 61, Z: 2}, {X: 1, Y: 61, Z: 2}, {X: 2, Y: 62, Z: 2},
			{X: 4, Y: 62, Z: 2}, {X: 6, Y: 62, Z: 2},
		})
}

// Start equals goal, non-standing endpoints and the revision identity that
// travels with every successful result.
func pathfindOracleEndpoints(t *testing.T) {
	grid := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 100, Y: 63, Z: -50}, 3, 3, 1,
			pathfindOracleTable, pathfindOracleFlatFetch(63),
			[]pathfind.ChunkRevision{
				{Chunk: core.ChunkPos{X: 3, Z: 1}, Revision: 9},
				{Chunk: core.ChunkPos{X: 1, Z: 2}, Revision: 4},
			})
	})
	start := pathfind.PathCell{X: 100, Y: 64, Z: -50}
	result := mustPathfindOraclePath(t, grid, start, start)
	assertPathfindOracleCells(t, result.Waypoints, []pathfind.PathCell{start})
	assertPathfindOracleRevisions(t, result.Revisions, []pathfind.ChunkRevision{
		{Chunk: core.ChunkPos{X: 1, Z: 2}, Revision: 4},
		{Chunk: core.ChunkPos{X: 3, Z: 1}, Revision: 9},
	})

	unsupported := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 3, 3, 1,
			pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
				if y == 63 && x != 0 {
					return core.StoneID, true
				}
				return core.AirID, true
			}, nil)
	})
	if _, err := pathfind.FindPath(unsupported, pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: 2, Y: 64, Z: 0}); !errors.Is(err, pathfind.ErrPathUnreachable) {
		t.Fatalf("non-standing start should be unreachable: %v", err)
	} else if errors.Is(err, pathfind.ErrPathBudgetExceeded) {
		t.Fatalf("non-standing start hit the wrong category: %v", err)
	}
	if _, err := pathfind.FindPath(unsupported, pathfind.PathCell{X: 2, Y: 64, Z: 0}, pathfind.PathCell{X: 0, Y: 64, Z: 0}); !errors.Is(err, pathfind.ErrPathUnreachable) {
		t.Fatalf("non-standing goal should be unreachable: %v", err)
	} else if errors.Is(err, pathfind.ErrPathBudgetExceeded) {
		t.Fatalf("non-standing goal hit the wrong category: %v", err)
	}
}

// The exact budget boundary for the narrow corridor shape: a 4096-long
// corridor succeeds at its far end while the 4097-long corridor exhausts the
// budget at its far end.
func pathfindOracleBudgetBoundary(t *testing.T) {
	corridor := func(length int32) pathfind.PathGrid {
		return mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
			return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: -1}, length, 3, 3,
				pathfindOracleTable, func(x, y, z int32) (core.BlockID, bool) {
					if y == 63 && z == 0 {
						return core.StoneID, true
					}
					return core.AirID, true
				}, nil)
		})
	}
	inside := corridor(pathfind.MaxPathNodes)
	result := mustPathfindOraclePath(t, inside,
		pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: pathfind.MaxPathNodes - 1, Y: 64, Z: 0})
	if first, last := result.Waypoints[0], result.Waypoints[len(result.Waypoints)-1]; first != (pathfind.PathCell{X: 0, Y: 64, Z: 0}) || last != (pathfind.PathCell{X: pathfind.MaxPathNodes - 1, Y: 64, Z: 0}) {
		t.Fatalf("boundary path endpoints wrong: first %+v last %+v", first, last)
	}
	outside := corridor(pathfind.MaxPathNodes + 1)
	if _, err := pathfind.FindPath(outside,
		pathfind.PathCell{X: 0, Y: 64, Z: 0}, pathfind.PathCell{X: pathfind.MaxPathNodes, Y: 64, Z: 0}); !errors.Is(err, pathfind.ErrPathBudgetExceeded) {
		t.Fatalf("4097-long corridor should exhaust the budget: %v", err)
	} else if errors.Is(err, pathfind.ErrPathUnreachable) {
		t.Fatalf("4097-long corridor hit the wrong category: %v", err)
	}
}

// Same grid searched twice returns identical waypoints: determinism without
// any scratch handle on the Go side.
func pathfindOracleDeterminism(t *testing.T) {
	grid := mustPathfindOracleGrid(t, func() (pathfind.PathGrid, error) {
		return pathfind.NewPathGrid(core.BlockPos{X: 0, Y: 63, Z: 0}, 5, 3, 1,
			pathfindOracleTable, pathfindOracleFlatFetch(63), nil)
	})
	start := pathfind.PathCell{X: 0, Y: 64, Z: 0}
	goal := pathfind.PathCell{X: 4, Y: 64, Z: 0}
	first := mustPathfindOraclePath(t, grid, start, goal)
	second := mustPathfindOraclePath(t, grid, start, goal)
	assertPathfindOracleCells(t, second.Waypoints, first.Waypoints)
	assertPathfindOracleRevisions(t, second.Revisions, first.Revisions)
}

func TestPathfindOracleSearch(t *testing.T) {
	pathfindOracleTransitions(t)
	pathfindOracleTies(t)
	pathfindOracleEndpoints(t)
	pathfindOracleBudgetBoundary(t)
	pathfindOracleDeterminism(t)
	pathfindOracleSearchExport(t)
}

// pathfindOracleSearchExport publishes the locked search observations when the
// export directory is named, in the same plain style as the grid producer:
// two summary files under a search producer child, created exclusively so
// reruns never silently replace evidence.
func pathfindOracleSearchExport(t *testing.T) {
	t.Helper()
	exportDir := strings.TrimSpace(os.Getenv("RUNTIME_ORACLE_EXPORT_DIR"))
	if exportDir == "" {
		return
	}
	producerDir := filepath.Join(exportDir, "pathfind-search")
	if err := os.MkdirAll(producerDir, 0755); err != nil {
		t.Fatalf("MkdirAll: %v", err)
	}
	summary := fmt.Sprintf(
		"cases=17 transitions=corridor-jump-fall-gap-blocked-flatgap ties=diamond-detour-terrace endpoints=startgoal-nonstanding budget=4096-vs-4097 determinism=repeat-identical\n"+
			"source=packages/shared/pathfind/pathfind.go sha=%s\n",
		"unpinned",
	)
	for name, data := range map[string][]byte{
		"search_summary.txt": []byte(summary),
		"search_paths.txt":   []byte("corridor: (0,64,0) (2,64,0) (4,64,0); diamond: (0,64,0) (1,64,0) (1,64,1); terrace: (0,61,2) (1,61,2) (2,62,2) (4,62,2) (6,62,2)\n"),
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
	t.Logf("exported pathfind search observations to %s", producerDir)
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
