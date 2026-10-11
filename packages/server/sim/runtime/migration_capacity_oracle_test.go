package runtime

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/channing771/mornlea/packages/shared/core"
	"github.com/channing771/mornlea/packages/shared/world"
)

// migrationCapacitySample is one owned queue observation. The test retains it
// and never treats a public status aggregate as a high-water.
type migrationCapacitySample struct {
	CaseID     string `json:"case_id"`
	Boundary   string `json:"boundary"`
	Lane       string `json:"lane,omitempty"`
	Records    int    `json:"records"`
	OwnedBytes int    `json:"owned_bytes"`
	Applicable bool   `json:"applicable"`
}

type migrationLaneReport struct {
	Applicable bool   `json:"applicable"`
	Records    int    `json:"records,omitempty"`
	OwnedBytes int    `json:"owned_bytes,omitempty"`
	Boundary   string `json:"boundary,omitempty"`
}

type migrationCapacityMaximum struct {
	Records    int    `json:"records"`
	OwnedBytes int    `json:"owned_bytes"`
	Boundary   string `json:"boundary"`
}

type migrationCapacityCase struct {
	CaseID            string                    `json:"case_id"`
	Supported         bool                      `json:"supported"`
	SourceSHA256      string                    `json:"source_sha256"`
	InventoryID       string                    `json:"inventory_id,omitempty"`
	Command           migrationLaneReport       `json:"command"`
	ReadyChunkResults migrationLaneReport       `json:"ready_chunk_results"`
	Persistence       migrationLaneReport       `json:"persistence"`
	Samples           []migrationCapacitySample `json:"samples"`
	SampleCount       int                       `json:"sample_count"`
	Maximum           migrationCapacityMaximum  `json:"maximum"`
}

type migrationCapacityDraft struct {
	SourceSHA256 string                  `json:"source_sha256"`
	Cases        []migrationCapacityCase `json:"cases"`
}

type migrationInventoryFile struct {
	Rows []migrationInventoryRow `json:"rows"`
}

type migrationInventoryRow struct {
	ID           string `json:"id"`
	SourceSHA256 string `json:"source_sha256"`
}

// observeMigrationCapacity records one owned command or ready-chunk sample.
// ownedBytes is 0 because this lane has no encoded save snapshot.
func observeMigrationCapacity(caseID, boundary string, records, ownedBytes int) migrationCapacitySample {
	return migrationCapacitySample{
		CaseID:     caseID,
		Boundary:   boundary,
		Records:    records,
		OwnedBytes: ownedBytes,
		Applicable: true,
	}
}

func migrationCapacityNotApplicable(caseID, boundary string) migrationCapacitySample {
	return migrationCapacitySample{CaseID: caseID, Boundary: boundary}
}

func migrationCountReportError(reported, observed int) error {
	if reported != observed {
		return fmt.Errorf("capacity report records=%d, observed=%d", reported, observed)
	}
	return nil
}

func TestMigrationCapacityReplay(t *testing.T) {
	root := migrationRepoRoot(t)
	sourceHash := migrationFileSHA256(t, filepath.Join(root, "packages/server/sim/runtime/engine.go"))
	rows := migrationInventoryRows(t, root)

	commands4096, observed4096 := migrationFillCommands(t, 4096)
	if err := migrationCountReportError(observed4096-1, observed4096); err == nil {
		t.Fatal("understated command report was accepted")
	} else {
		t.Logf("understated command report rejected: %v", err)
	}
	if err := migrationCountReportError(observed4096, observed4096); err != nil {
		t.Fatal(err)
	}
	if observed4096 != 4096 || commands4096 != 4096 {
		t.Fatalf("command high-water=%d, want 4096", observed4096)
	}

	commands4097, observed4097 := migrationFillCommands(t, 4097)
	if err := migrationCountReportError(4096, observed4097); err == nil {
		t.Fatal("understated 4097 command report was accepted")
	} else {
		t.Logf("understated 4097 command report rejected: %v", err)
	}
	if err := migrationCountReportError(observed4097, observed4097); err != nil {
		t.Fatal(err)
	}
	if observed4097 != 4097 || commands4097 != 4097 {
		t.Fatalf("command probe=%d, want 4097", observed4097)
	}

	ready64, observed64 := migrationFillReadyChunks(t, 64)
	if err := migrationCountReportError(observed64-1, observed64); err == nil {
		t.Fatal("understated ready-chunk report was accepted")
	} else {
		t.Logf("understated ready-chunk report rejected: %v", err)
	}
	if err := migrationCountReportError(observed64, observed64); err != nil {
		t.Fatal(err)
	}
	if observed64 != 64 || ready64 != 64 {
		t.Fatalf("ready-chunk high-water=%d, want 64", observed64)
	}

	ready65, observed65 := migrationFillReadyChunks(t, 65)
	if err := migrationCountReportError(64, observed65); err == nil {
		t.Fatal("understated 65 ready-chunk report was accepted")
	} else {
		t.Logf("understated 65 ready-chunk report rejected: %v", err)
	}
	if err := migrationCountReportError(observed65, observed65); err != nil {
		t.Fatal(err)
	}
	if observed65 != 65 || ready65 != 65 {
		t.Fatalf("ready-chunk probe=%d, want 65", observed65)
	}

	cases := []migrationCapacityCase{
		migrationRuntimeQueueCase(t, "runtime.commands.4096", sourceHash, "", "commands", "Engine.commands", observed4096, true),
		migrationRuntimeQueueCase(t, "runtime.commands.4097", sourceHash, "", "commands", "Engine.commands", observed4097, false),
		migrationRuntimeQueueCase(t, "runtime.ready-chunks.64", sourceHash, "", "ready_chunk_results", "Engine.generated", observed64, true),
		migrationRuntimeQueueCase(t, "runtime.ready-chunks.65", sourceHash, "", "ready_chunk_results", "Engine.generated", observed65, false),
	}
	for _, row := range rows {
		if strings.HasPrefix(row.ID, "save.") {
			continue
		}
		cases = append(cases, migrationInventoryAdmissionCase(t, row))
	}
	draft := migrationCapacityDraft{SourceSHA256: sourceHash, Cases: cases}
	migrationWriteDraft(t, "runtime.json", draft)
}

func migrationFillCommands(t *testing.T, count int) (int, int) {
	t.Helper()
	engine := NewEngine(0, 0, 1)
	for index := range count {
		engine.Enqueue(Command{
			Session:  SessionID(1),
			Sequence: uint64(index + 1),
			Kind:     CommandPlayerInput,
		})
	}
	engine.inboxMu.Lock()
	observed := len(engine.commands)
	engine.inboxMu.Unlock()
	return count, observed
}

func migrationFillReadyChunks(t *testing.T, count int) (int, int) {
	t.Helper()
	engine := NewEngine(0, 0, 1)
	for index := range count {
		pos := core.ChunkPos{X: int32(index), Z: 1}
		engine.SubmitGenerated(GeneratedChunk{
			Dimension: core.Overworld,
			Pos:       pos,
			Chunk:     world.NewChunk(pos),
		})
	}
	engine.inboxMu.Lock()
	observed := len(engine.generated)
	engine.inboxMu.Unlock()
	return count, observed
}

func migrationRuntimeQueueCase(
	t *testing.T,
	caseID, sourceHash, inventoryID, lane, boundary string,
	records int,
	supported bool,
) migrationCapacityCase {
	t.Helper()
	sample := observeMigrationCapacity(caseID, boundary, records, 0)
	sample.Lane = lane
	item := migrationCapacityCase{
		CaseID:            caseID,
		Supported:         supported,
		SourceSHA256:      sourceHash,
		InventoryID:       inventoryID,
		Command:           migrationLaneNotApplicable(),
		ReadyChunkResults: migrationLaneNotApplicable(),
		Persistence:       migrationLaneNotApplicable(),
		Samples:           []migrationCapacitySample{sample},
	}
	switch lane {
	case "commands":
		item.Command = migrationLaneApplicable(boundary, records, 0)
	case "ready_chunk_results":
		item.ReadyChunkResults = migrationLaneApplicable(boundary, records, 0)
	default:
		t.Fatalf("unknown runtime lane %s", lane)
	}
	return finalizeMigrationCase(t, item)
}

func migrationInventoryAdmissionCase(t *testing.T, row migrationInventoryRow) migrationCapacityCase {
	t.Helper()
	engine := NewEngine(0, 0, 1)
	engine.Enqueue(Command{Session: 1, Sequence: 1, Kind: CommandPlayerInput})
	pos := core.ChunkPos{X: 0, Z: 2}
	engine.SubmitGenerated(GeneratedChunk{
		Dimension: core.Overworld,
		Pos:       pos,
		Chunk:     world.NewChunk(pos),
	})
	engine.inboxMu.Lock()
	commands := len(engine.commands)
	ready := len(engine.generated)
	engine.inboxMu.Unlock()
	if commands != 1 || ready != 1 {
		t.Fatalf("inventory %s admission commands=%d ready=%d", row.ID, commands, ready)
	}
	commandSample := observeMigrationCapacity(row.ID, "Engine.commands", commands, 0)
	commandSample.Lane = "commands"
	readySample := observeMigrationCapacity(row.ID, "Engine.generated", ready, 0)
	readySample.Lane = "ready_chunk_results"
	return finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            row.ID,
		Supported:         true,
		SourceSHA256:      row.SourceSHA256,
		InventoryID:       row.ID,
		Command:           migrationLaneApplicable("Engine.commands", commands, 0),
		ReadyChunkResults: migrationLaneApplicable("Engine.generated", ready, 0),
		Persistence:       migrationLaneNotApplicable(),
		Samples:           []migrationCapacitySample{commandSample, readySample},
	})
}

func migrationLaneApplicable(boundary string, records, ownedBytes int) migrationLaneReport {
	return migrationLaneReport{
		Applicable: true,
		Records:    records,
		OwnedBytes: ownedBytes,
		Boundary:   boundary,
	}
}

func migrationLaneNotApplicable() migrationLaneReport {
	return migrationLaneReport{}
}

func finalizeMigrationCase(t *testing.T, item migrationCapacityCase) migrationCapacityCase {
	t.Helper()
	item.SampleCount = len(item.Samples)
	found := false
	for _, sample := range item.Samples {
		if !sample.Applicable {
			continue
		}
		if sample.Records == 0 {
			t.Fatalf("case %s boundary %s is a zero sample", item.CaseID, sample.Boundary)
		}
		if item.Supported {
			switch sample.Lane {
			case "commands":
				if sample.Records > 4096 {
					t.Fatalf("BLOCKED commands records=%d", sample.Records)
				}
			case "ready_chunk_results":
				if sample.Records > 64 {
					t.Fatalf("BLOCKED ready chunk records=%d", sample.Records)
				}
			}
			if sample.OwnedBytes > 4194304 {
				t.Fatalf("BLOCKED owned bytes=%d", sample.OwnedBytes)
			}
		}
		if !found || sample.Records > item.Maximum.Records ||
			(sample.Records == item.Maximum.Records && sample.OwnedBytes > item.Maximum.OwnedBytes) {
			found = true
			item.Maximum = migrationCapacityMaximum{
				Records:    sample.Records,
				OwnedBytes: sample.OwnedBytes,
				Boundary:   sample.Boundary,
			}
		}
	}
	if !found {
		t.Fatalf("case %s has no applicable sample", item.CaseID)
	}
	return item
}

func migrationInventoryRows(t *testing.T, root string) []migrationInventoryRow {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(root, "testdata/runtime-migration/server/capability-inventory.json"))
	if err != nil {
		t.Fatal(err)
	}
	var inventory migrationInventoryFile
	if err := json.Unmarshal(data, &inventory); err != nil {
		t.Fatal(err)
	}
	if len(inventory.Rows) == 0 {
		t.Fatal("capability inventory has no rows")
	}
	return inventory.Rows
}

func migrationRepoRoot(t *testing.T) string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime caller unavailable")
	}
	dir := filepath.Dir(file)
	for {
		if _, err := os.Stat(filepath.Join(dir, "go.work")); err == nil {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			t.Fatal("repository root not found")
		}
		dir = parent
	}
}

func migrationFileSHA256(t *testing.T, path string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	sum := sha256.Sum256(data)
	return "sha256:" + hex.EncodeToString(sum[:])
}

func migrationWriteDraft(t *testing.T, name string, draft migrationCapacityDraft) {
	t.Helper()
	if len(draft.Cases) == 0 {
		t.Fatal("draft case set is empty")
	}
	dir := os.Getenv("MIGRATION_CAPACITY_EXPORT_DIR")
	if dir == "" {
		return
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	encoded, err := json.MarshalIndent(draft, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	encoded = append(encoded, '\n')
	path := filepath.Join(dir, name)
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := file.Write(encoded); err != nil {
		_ = file.Close()
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
}
