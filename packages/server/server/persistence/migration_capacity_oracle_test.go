package persistence

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	stdruntime "runtime"
	"testing"
	"unsafe"

	"github.com/channing771/mornlea/packages/server/sim/contract"
	simruntime "github.com/channing771/mornlea/packages/server/sim/runtime"
	"github.com/channing771/mornlea/packages/server/storage"
	"github.com/channing771/mornlea/packages/server/storage/chunk"
	"github.com/channing771/mornlea/packages/shared/core"
)

// migrationCapacitySample is one owned queue observation retained by the test.
// A public status aggregate is never a high-water sample.
type migrationCapacitySample struct {
	CaseID     string `json:"case_id"`
	Boundary   string `json:"boundary"`
	Lane       string `json:"lane,omitempty"`
	Records    int    `json:"records,omitempty"`
	OwnedBytes int    `json:"owned_bytes,omitempty"`
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

type migrationCapacityEstimates struct {
	SnapshotEstimatedBytes int   `json:"snapshot_estimated_bytes,omitempty"`
	StatusEstimatedBytes   int64 `json:"status_estimated_bytes,omitempty"`
}

type migrationOwnershipLane struct {
	Applicable bool   `json:"applicable"`
	Records    int    `json:"records,omitempty"`
	OwnedBytes int    `json:"owned_bytes,omitempty"`
	Boundary   string `json:"boundary,omitempty"`
	Shared     bool   `json:"shared_backing,omitempty"`
}

// migrationOwnershipLanes separates queue, held, completion, and retry bytes.
// Total is the deduplicated ownership of that schedule, not the sum of aliases
// that point at one retained slice.
type migrationOwnershipLanes struct {
	Queue      migrationOwnershipLane `json:"queue"`
	Held       migrationOwnershipLane `json:"held"`
	Completion migrationOwnershipLane `json:"completion"`
	Retry      migrationOwnershipLane `json:"retry"`
	Total      migrationOwnershipLane `json:"total"`
}

type migrationFamilyReport struct {
	Family            string `json:"family"`
	Applicable        bool   `json:"applicable"`
	Count             int    `json:"count,omitempty"`
	LogicalBound      int    `json:"logical_bound,omitempty"`
	EncodedEquivalent int    `json:"encoded_equivalent,omitempty"`
	RetainedSnapshots int    `json:"retained_snapshots,omitempty"`
	OwnedBytes        int    `json:"owned_bytes,omitempty"`
}

type migrationCapacityCase struct {
	CaseID            string                      `json:"case_id"`
	Supported         bool                        `json:"supported"`
	SourceSHA256      string                      `json:"source_sha256"`
	InventoryID       string                      `json:"inventory_id,omitempty"`
	Command           migrationLaneReport         `json:"command"`
	ReadyChunkResults migrationLaneReport         `json:"ready_chunk_results"`
	Persistence       migrationLaneReport         `json:"persistence"`
	Samples           []migrationCapacitySample   `json:"samples"`
	SampleCount       int                         `json:"sample_count"`
	Maximum           migrationCapacityMaximum    `json:"maximum"`
	Estimates         *migrationCapacityEstimates `json:"estimates,omitempty"`
	Lanes             *migrationOwnershipLanes    `json:"lanes,omitempty"`
	Families          []migrationFamilyReport     `json:"families,omitempty"`
	RegionJobs        int                         `json:"region_jobs,omitempty"`
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

const (
	migrationCommandCeiling      = 4096
	migrationReadyChunkCeiling   = 64
	migrationPendingChunkCeiling = 8
	migrationPendingByteCeiling  = 4194304
)

// observeMigrationCapacity records one owned sample. The caller keeps the
// returned value; production types gain no measurement field.
func observeMigrationCapacity(caseID, boundary string, records, ownedBytes int) migrationCapacitySample {
	return migrationCapacitySample{
		CaseID:     caseID,
		Boundary:   boundary,
		Records:    records,
		OwnedBytes: ownedBytes,
		Applicable: true,
	}
}

func migrationSample(caseID, boundary, lane string, records, ownedBytes int) migrationCapacitySample {
	sample := observeMigrationCapacity(caseID, boundary, records, ownedBytes)
	sample.Lane = lane
	return sample
}

func migrationAbsent(caseID, boundary string) migrationCapacitySample {
	return migrationCapacitySample{CaseID: caseID, Boundary: boundary}
}

func migrationLaneApplicable(boundary string, records, ownedBytes int) migrationLaneReport {
	return migrationLaneReport{
		Applicable: true,
		Records:    records,
		OwnedBytes: ownedBytes,
		Boundary:   boundary,
	}
}

func migrationLaneAbsent() migrationLaneReport {
	return migrationLaneReport{}
}

func migrationOwnership(boundary string, records, ownedBytes int, shared bool) migrationOwnershipLane {
	return migrationOwnershipLane{
		Applicable: true,
		Records:    records,
		OwnedBytes: ownedBytes,
		Boundary:   boundary,
		Shared:     shared,
	}
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
		if sample.Lane != "commands" && sample.Lane != "ready_chunk_results" && sample.OwnedBytes == 0 {
			t.Fatalf("case %s boundary %s owns no encoded bytes", item.CaseID, sample.Boundary)
		}
		if item.Supported {
			switch sample.Lane {
			case "commands":
				if sample.Records > migrationCommandCeiling {
					t.Fatalf("BLOCKED commands records=%d", sample.Records)
				}
			case "ready_chunk_results":
				if sample.Records > migrationReadyChunkCeiling {
					t.Fatalf("BLOCKED ready chunk records=%d", sample.Records)
				}
			case "pending_chunk_saves":
				if sample.Records > migrationPendingChunkCeiling {
					t.Fatalf("BLOCKED pending chunk saves records=%d owned_bytes=%d boundary=%s",
						sample.Records, sample.OwnedBytes, sample.Boundary)
				}
			}
			if sample.OwnedBytes > migrationPendingByteCeiling {
				t.Fatalf("BLOCKED pending save bytes=%d records=%d lane=%s boundary=%s",
					sample.OwnedBytes, sample.Records, sample.Lane, sample.Boundary)
			}
		}
		if !found || sample.Records > item.Maximum.Records ||
			(sample.Records == item.Maximum.Records && sample.OwnedBytes >= item.Maximum.OwnedBytes) {
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

func migrationClosedFamilies() []migrationFamilyReport {
	return []migrationFamilyReport{
		{Family: "player"},
		{Family: "companion"},
		{Family: "hostile"},
		{Family: "passive"},
		{Family: "metadata"},
	}
}

func migrationWithFamily(base []migrationFamilyReport, report migrationFamilyReport) []migrationFamilyReport {
	next := append([]migrationFamilyReport(nil), base...)
	report.Applicable = true
	for index := range next {
		if next[index].Family == report.Family {
			next[index] = report
			return next
		}
	}
	return append(next, report)
}

func migrationInventoryByID(t *testing.T, root string) map[string]migrationInventoryRow {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(root, "testdata/runtime-migration/server/capability-inventory.json"))
	if err != nil {
		t.Fatal(err)
	}
	var inventory migrationInventoryFile
	if err := json.Unmarshal(data, &inventory); err != nil {
		t.Fatal(err)
	}
	rows := make(map[string]migrationInventoryRow, len(inventory.Rows))
	for _, row := range inventory.Rows {
		rows[row.ID] = row
	}
	return rows
}

func migrationRepoRoot(t *testing.T) string {
	t.Helper()
	_, file, _, ok := stdruntime.Caller(0)
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

func migrationFilesSHA256(t *testing.T, paths ...string) string {
	t.Helper()
	hasher := sha256.New()
	for _, path := range paths {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		_, _ = hasher.Write(data)
	}
	return "sha256:" + hex.EncodeToString(hasher.Sum(nil))
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

func migrationLogCases(t *testing.T, cases []migrationCapacityCase) {
	t.Helper()
	for _, item := range cases {
		t.Logf("case %s maximum records=%d owned_bytes=%d boundary=%s samples=%d",
			item.CaseID, item.Maximum.Records, item.Maximum.OwnedBytes, item.Maximum.Boundary, item.SampleCount)
	}
}

// migrationWorkerlessWorld matches the flush-frozen harness fields and does
// not start saveWorker goroutines. Callers drive dispatch directly.
func migrationWorkerlessWorld(store storage.Store, engine *simruntime.Engine, options Options) *World {
	if options.AutosaveTicks == 0 {
		options.AutosaveTicks = 1
	}
	return &World{
		store:           store,
		engine:          engine,
		options:         options,
		saveJobs:        make(chan saveJob, 4),
		saveCompletions: make(chan saveCompletion, 4),
		retry:           make(map[storage.RegionKey][]retrySave),
		retryInFlight:   make(map[uint64]retrySave),
	}
}

func migrationChunkSnapshotBytes(t *testing.T, snapshots []contract.ChunkSaveSnapshot) int {
	t.Helper()
	total := 0
	for _, snapshot := range snapshots {
		encoded, err := chunk.Encode(chunk.ChunkSave{
			Key:      snapshot.Key,
			Revision: snapshot.Revision,
			Chunk:    snapshot.Chunk,
		})
		if err != nil {
			t.Fatal(err)
		}
		if len(encoded) == 0 {
			t.Fatalf("encoded chunk %v is empty", snapshot.Key)
		}
		total += len(encoded)
	}
	return total
}

func migrationChunkSaveBytes(t *testing.T, saves []storage.ChunkSave) int {
	t.Helper()
	total := 0
	for _, save := range saves {
		encoded, err := chunk.Encode(save)
		if err != nil {
			t.Fatal(err)
		}
		if len(encoded) == 0 {
			t.Fatalf("encoded chunk %v is empty", save.Key)
		}
		total += len(encoded)
	}
	return total
}

// migrationMetadataEncodedLength is the on-disk world.meta size. encodeMetadata
// stays inside the storage package; OpenDisk Create writes that codec output.
func migrationMetadataEncodedLength(t *testing.T, metadata storage.Metadata) int {
	t.Helper()
	root := t.TempDir()
	store, err := storage.OpenDisk(context.Background(), root, storage.OpenOptions{Create: metadata})
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	info, err := os.Stat(filepath.Join(root, "world.meta"))
	if err != nil {
		t.Fatal(err)
	}
	if info.Size() == 0 {
		t.Fatal("world.meta is empty")
	}
	return int(info.Size())
}

func TestMigrationPersistenceCapacityReplay(t *testing.T) {
	root := migrationRepoRoot(t)
	sourceHash := migrationFileSHA256(t, filepath.Join(root, "packages/server/server/persistence/world.go"))
	rows := migrationInventoryByID(t, root)
	chunkRow, ok := rows["save.chunk"]
	if !ok {
		t.Fatal("inventory missing save.chunk")
	}
	regionRow, ok := rows["save.region"]
	if !ok {
		t.Fatal("inventory missing save.region")
	}
	metadataRow, ok := rows["save.world-metadata"]
	if !ok {
		t.Fatal("inventory missing save.world-metadata")
	}

	chunkCase, regionCase := migrationChunkCapacityCases(t, sourceHash, chunkRow, regionRow)
	cases := []migrationCapacityCase{
		chunkCase,
		regionCase,
		migrationSharedTicketCase(t, sourceHash),
		migrationMixedLaneCase(t, sourceHash),
		migrationWorldMetadataCase(t, sourceHash, metadataRow),
	}
	migrationLogCases(t, cases)
	migrationWriteDraft(t, "persistence.json", migrationCapacityDraft{
		SourceSHA256: sourceHash,
		Cases:        cases,
	})
}

func migrationChunkCapacityCases(
	t *testing.T,
	sourceHash string,
	chunkRow, regionRow migrationInventoryRow,
) (migrationCapacityCase, migrationCapacityCase) {
	t.Helper()
	keys := make([]core.ChunkKey, 0, migrationPendingChunkCeiling)
	for index := range migrationPendingChunkCeiling {
		keys = append(keys, chunkKey(int32(index), 0))
	}
	options := persistenceTestOptions()
	options.AutosaveTicks = 1
	options.SaveChunks = migrationPendingChunkCeiling
	options.SaveBytes = 1 << 20
	engine := dirtyReadyEngine(t, keys)
	world := migrationWorkerlessWorld(newPersistenceTestStore(), engine, options)
	before := len(world.saveJobs)
	if before != 0 {
		t.Fatalf("saveJobs before schedule = %d", before)
	}
	world.mu.Lock()
	world.schedulePersistenceLocked(options.AutosaveTicks)
	world.mu.Unlock()
	if len(world.saveJobs) != 1 {
		t.Fatalf("scheduled jobs=%d, want 1", len(world.saveJobs))
	}
	job := <-world.saveJobs
	if len(job.Snapshots) != migrationPendingChunkCeiling {
		t.Fatalf("scheduled snapshots=%d, want %d", len(job.Snapshots), migrationPendingChunkCeiling)
	}
	region, _ := storage.RegionFor(job.Snapshots[0].Key)
	for _, snapshot := range job.Snapshots[1:] {
		got, _ := storage.RegionFor(snapshot.Key)
		if got != region {
			t.Fatalf("snapshot %v region=%+v, want %+v", snapshot.Key, got, region)
		}
	}
	owned := migrationChunkSnapshotBytes(t, job.Snapshots)
	estimated := 0
	for _, snapshot := range job.Snapshots {
		estimated += snapshot.EstimatedBytes
	}
	if estimated != migrationPendingChunkCeiling*emptyChunkEstimateBytes {
		t.Fatalf("snapshot estimate=%d, want %d", estimated, migrationPendingChunkCeiling*emptyChunkEstimateBytes)
	}
	statusEstimated := engine.PersistenceStats().EstimatedBytes

	directEngine := dirtyReadyEngine(t, []core.ChunkKey{chunkKey(8, 0)})
	directWorld := migrationWorkerlessWorld(newPersistenceTestStore(), directEngine, options)
	directSnapshots := directEngine.PersistenceSnapshots(1, options.SaveBytes, contract.SaveAll)
	if len(directSnapshots) != 1 {
		t.Fatalf("direct snapshots=%d, want 1", len(directSnapshots))
	}
	directWorld.mu.Lock()
	directWorld.dispatchPersistenceLocked(directSnapshots)
	directWorld.mu.Unlock()
	if len(directWorld.saveJobs) != 1 {
		t.Fatalf("direct jobs=%d, want 1", len(directWorld.saveJobs))
	}
	directJob := <-directWorld.saveJobs
	directOwned := migrationChunkSnapshotBytes(t, directJob.Snapshots)

	saves := make([]storage.ChunkSave, len(job.Snapshots))
	for index, snapshot := range job.Snapshots {
		saves[index] = storage.ChunkSave{
			Key: snapshot.Key, Revision: snapshot.Revision, Chunk: snapshot.Chunk,
		}
	}
	completion := saveCompletion{Job: job, Result: committedResult(saves)}
	world.saveCompletions <- completion
	if len(world.saveCompletions) != 1 {
		t.Fatalf("completions on enqueue=%d", len(world.saveCompletions))
	}
	enqueued := <-world.saveCompletions
	world.mu.Lock()
	err := world.applySaveCompletionLocked(enqueued)
	world.mu.Unlock()
	if err != nil {
		t.Fatal(err)
	}
	if len(world.saveCompletions) != 0 {
		t.Fatal("completion remained after apply")
	}

	estimates := &migrationCapacityEstimates{
		SnapshotEstimatedBytes: estimated,
		StatusEstimatedBytes:   statusEstimated,
	}
	chunkSamples := []migrationCapacitySample{
		migrationAbsent("save.chunk", "saveJobs before schedulePersistenceLocked"),
		migrationSample("save.chunk", "World.saveJobs", "pending_chunk_saves", len(job.Snapshots), owned),
		migrationSample("save.chunk", "World.dispatchPersistenceLocked", "pending_chunk_saves", len(directJob.Snapshots), directOwned),
		migrationSample("save.chunk", "saveCompletions on enqueue", "pending_chunk_saves", len(job.Snapshots), owned),
		migrationAbsent("save.chunk", "saveCompletions after consume"),
	}
	chunkCase := finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            "save.chunk",
		Supported:         true,
		SourceSHA256:      chunkRow.SourceSHA256,
		InventoryID:       chunkRow.ID,
		Command:           migrationLaneAbsent(),
		ReadyChunkResults: migrationLaneAbsent(),
		Persistence:       migrationLaneApplicable("World.saveJobs", len(job.Snapshots), owned),
		Samples:           chunkSamples,
		Estimates:         estimates,
		Families:          migrationClosedFamilies(),
		RegionJobs:        1,
	})
	regionCase := finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            "save.region",
		Supported:         true,
		SourceSHA256:      regionRow.SourceSHA256,
		InventoryID:       regionRow.ID,
		Command:           migrationLaneAbsent(),
		ReadyChunkResults: migrationLaneAbsent(),
		Persistence:       migrationLaneApplicable("groupSaveJobs", len(job.Snapshots), owned),
		Samples: []migrationCapacitySample{
			migrationSample("save.region", "groupSaveJobs", "pending_chunk_saves", len(job.Snapshots), owned),
		},
		Estimates:  estimates,
		Families:   migrationClosedFamilies(),
		RegionJobs: 1,
	})
	return chunkCase, regionCase
}

func migrationSharedTicketCase(t *testing.T, sourceHash string) migrationCapacityCase {
	t.Helper()
	const caseID = "world.shared-ticket"
	options := persistenceTestOptions()
	options.AutosaveTicks = 1
	engine := dirtyReadyEngine(t, []core.ChunkKey{chunkKey(0, 0)})
	world := migrationWorkerlessWorld(newPersistenceTestStore(), engine, options)
	snapshots := engine.PersistenceSnapshots(1, options.SaveBytes, contract.SaveAll)
	if len(snapshots) != 1 {
		t.Fatalf("retry snapshots=%d, want 1", len(snapshots))
	}
	original := append([]contract.ChunkSaveSnapshot(nil), snapshots...)
	originalData := unsafe.SliceData(original)
	region, _ := storage.RegionFor(original[0].Key)
	world.retry[region] = []retrySave{{
		Job: saveJob{
			Region:    region,
			Snapshots: original,
			Retry:     true,
			RetryID:   1,
		},
		Attempts: 1,
		NextTick: 1,
	}}
	beforeBytes := migrationChunkSnapshotBytes(t, original)
	world.mu.Lock()
	world.dispatchDueRetriesLocked(1)
	world.mu.Unlock()
	if len(world.saveJobs) != 1 {
		t.Fatalf("shared jobs=%d, want 1", len(world.saveJobs))
	}
	if len(world.retry) != 0 {
		t.Fatal("pending retry cohort remained after dispatch")
	}
	queued := <-world.saveJobs
	inflight, ok := world.retryInFlight[queued.RetryID]
	if !ok {
		t.Fatal("retryInFlight missing dispatched cohort")
	}
	queuedData := unsafe.SliceData(queued.Snapshots)
	inflightData := unsafe.SliceData(inflight.Job.Snapshots)
	if queuedData == originalData {
		t.Fatal("dispatch reused the pending retry slice")
	}
	if queuedData != inflightData || len(queued.Snapshots) != 1 {
		t.Fatal("queue and retryInFlight do not share one snapshot ticket")
	}
	owned := migrationChunkSnapshotBytes(t, queued.Snapshots)
	if owned != beforeBytes {
		t.Fatalf("shared ticket bytes=%d, one snapshot=%d", owned, beforeBytes)
	}
	t.Logf("shared snapshot backing records=1 owned_bytes=%d", owned)
	lanes := &migrationOwnershipLanes{
		Queue:      migrationOwnership("World.saveJobs", 1, owned, true),
		Held:       migrationOwnershipLane{},
		Completion: migrationOwnershipLane{},
		Retry:      migrationOwnership("World.retryInFlight", 1, owned, true),
		Total:      migrationOwnership("deduplicated pending chunks", 1, owned, true),
	}
	return finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            caseID,
		Supported:         true,
		SourceSHA256:      sourceHash,
		Command:           migrationLaneAbsent(),
		ReadyChunkResults: migrationLaneAbsent(),
		Persistence:       migrationLaneApplicable("deduplicated pending chunks", 1, owned),
		Samples: []migrationCapacitySample{
			migrationSample(caseID, "World.saveJobs", "pending_chunk_saves", 1, owned),
			migrationSample(caseID, "World.retryInFlight", "pending_chunk_saves", 1, owned),
			migrationSample(caseID, "deduplicated pending chunks", "pending_chunk_saves", 1, owned),
		},
		Lanes:    lanes,
		Families: migrationClosedFamilies(),
	})
}

func migrationMixedLaneCase(t *testing.T, sourceHash string) migrationCapacityCase {
	t.Helper()
	const caseID = "world.mixed-lanes"
	keys := []core.ChunkKey{chunkKey(0, 0), chunkKey(32, 0), chunkKey(64, 0), chunkKey(96, 0)}
	options := persistenceTestOptions()
	options.AutosaveTicks = 1
	options.SaveWorkers = 1
	engine := dirtyReadyEngine(t, keys)
	snapshots := make([]contract.ChunkSaveSnapshot, len(keys))
	for index := range keys {
		got := engine.PersistenceSnapshots(1, options.SaveBytes, contract.SaveAll)
		if len(got) != 1 {
			t.Fatalf("mixed snapshot %d count=%d", index, len(got))
		}
		snapshots[index] = got[0]
	}
	store := newPersistenceTestStore()
	store.gate = make(chan struct{})
	world := NewWorld(store, engine, options)
	t.Cleanup(func() {
		store.recoverForShutdownCleanup()
		world.Close()
	})

	regionOf := func(snapshot contract.ChunkSaveSnapshot) storage.RegionKey {
		region, _ := storage.RegionFor(snapshot.Key)
		return region
	}
	heldJob := saveJob{
		Region: regionOf(snapshots[0]), Snapshots: []contract.ChunkSaveSnapshot{snapshots[0]}, Attempt: 1,
	}
	queueJob := saveJob{
		Region: regionOf(snapshots[1]), Snapshots: []contract.ChunkSaveSnapshot{snapshots[1]}, Attempt: 1,
	}
	completionJob := saveJob{
		Region: regionOf(snapshots[2]), Snapshots: []contract.ChunkSaveSnapshot{snapshots[2]}, Attempt: 1,
	}
	retryJob := saveJob{
		Region:    regionOf(snapshots[3]),
		Snapshots: []contract.ChunkSaveSnapshot{snapshots[3]},
		Retry:     true,
		RetryID:   1,
	}
	world.saveJobs <- heldJob
	heldSaves := receiveSaveCall(t, store)
	queueBytes := migrationChunkSnapshotBytes(t, queueJob.Snapshots)
	completionBytes := migrationChunkSnapshotBytes(t, completionJob.Snapshots)
	retryBytes := migrationChunkSnapshotBytes(t, retryJob.Snapshots)
	heldBytes := migrationChunkSaveBytes(t, heldSaves)
	world.saveJobs <- queueJob
	world.saveCompletions <- saveCompletion{Job: completionJob, Err: errors.New("mixed completion failed")}
	world.mu.Lock()
	world.retry[retryJob.Region] = []retrySave{{
		Job: retryJob, Attempts: 1, NextTick: ^uint64(0),
	}}
	queueRecords := len(world.saveJobs)
	completionRecords := len(world.saveCompletions)
	retryRecords := len(world.retry[retryJob.Region])
	world.mu.Unlock()
	if queueRecords != 1 || completionRecords != 1 || retryRecords != 1 || len(heldSaves) != 1 {
		t.Fatalf("mixed lanes queue=%d held=%d completion=%d retry=%d",
			queueRecords, len(heldSaves), completionRecords, retryRecords)
	}
	totalRecords := len(heldSaves) + queueRecords + completionRecords + retryRecords
	totalBytes := heldBytes + queueBytes + completionBytes + retryBytes
	t.Logf("mixed lanes queue=%d held=%d completion=%d retry=%d total_records=%d total_bytes=%d",
		queueBytes, heldBytes, completionBytes, retryBytes, totalRecords, totalBytes)

	world.mu.Lock()
	beforeDrain := len(world.saveCompletions)
	drainErr := world.drainSaveCompletionsLocked()
	afterDrain := len(world.saveCompletions)
	var transferred []contract.ChunkSaveSnapshot
	for _, cohorts := range world.retry {
		for _, cohort := range cohorts {
			if cohort.Job.RetryID == retryJob.RetryID {
				continue
			}
			transferred = append(transferred, cohort.Job.Snapshots...)
		}
	}
	world.mu.Unlock()
	if drainErr == nil {
		t.Fatal("mixed completion drain succeeded")
	}
	if beforeDrain != 1 || afterDrain != 0 {
		t.Fatalf("completion before=%d after=%d", beforeDrain, afterDrain)
	}
	if len(transferred) != 1 {
		t.Fatalf("transferred retry snapshots=%d, want 1", len(transferred))
	}
	transferredBytes := migrationChunkSnapshotBytes(t, transferred)
	lanes := &migrationOwnershipLanes{
		Queue:      migrationOwnership("World.saveJobs", 1, queueBytes, false),
		Held:       migrationOwnership("Store.SaveBatch", len(heldSaves), heldBytes, false),
		Completion: migrationOwnership("World.saveCompletions", 1, completionBytes, false),
		Retry:      migrationOwnership("World.retry", 1, retryBytes, false),
		Total:      migrationOwnership("deduplicated pending chunks", totalRecords, totalBytes, false),
	}
	return finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            caseID,
		Supported:         true,
		SourceSHA256:      sourceHash,
		Command:           migrationLaneAbsent(),
		ReadyChunkResults: migrationLaneAbsent(),
		Persistence:       migrationLaneApplicable("deduplicated pending chunks", totalRecords, totalBytes),
		Samples: []migrationCapacitySample{
			migrationSample(caseID, "World.saveJobs", "pending_chunk_saves", 1, queueBytes),
			migrationSample(caseID, "Store.SaveBatch", "pending_chunk_saves", len(heldSaves), heldBytes),
			migrationSample(caseID, "saveCompletions on enqueue", "pending_chunk_saves", 1, completionBytes),
			migrationSample(caseID, "saveCompletions before drain", "pending_chunk_saves", beforeDrain, completionBytes),
			migrationAbsent(caseID, "saveCompletions after consume"),
			migrationSample(caseID, "World.retry", "pending_chunk_saves", 1, retryBytes),
			migrationSample(caseID, "retry after transfer", "pending_chunk_saves", len(transferred), transferredBytes),
			migrationSample(caseID, "deduplicated pending chunks", "pending_chunk_saves", totalRecords, totalBytes),
		},
		Lanes:    lanes,
		Families: migrationClosedFamilies(),
	})
}

func migrationWorldMetadataCase(
	t *testing.T,
	sourceHash string,
	row migrationInventoryRow,
) migrationCapacityCase {
	t.Helper()
	const caseID = "world.metadata"
	options := persistenceTestOptions()
	options.AutosaveTicks = 1
	engine := simruntime.NewEngine(0, 0, 42)
	world := migrationWorkerlessWorld(newPersistenceTestStore(), engine, options)
	if len(world.saveJobs) != 0 {
		t.Fatal("metadata world started with a queued job")
	}
	world.Observe(options.AutosaveTicks, 7000)
	if len(world.saveJobs) != 1 {
		t.Fatalf("observe jobs=%d, want one metadata job", len(world.saveJobs))
	}
	job := <-world.saveJobs
	if job.Kind != saveKindMetadata {
		t.Fatalf("observe job kind=%d, want metadata", job.Kind)
	}
	owned := migrationMetadataEncodedLength(t, job.Metadata)
	family := migrationFamilyReport{
		Family:            "metadata",
		Count:             1,
		LogicalBound:      1,
		EncodedEquivalent: owned,
		RetainedSnapshots: 1,
		OwnedBytes:        owned,
	}
	return finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            caseID,
		Supported:         true,
		SourceSHA256:      sourceHash,
		InventoryID:       row.ID,
		Command:           migrationLaneAbsent(),
		ReadyChunkResults: migrationLaneAbsent(),
		Persistence:       migrationLaneApplicable("World.scheduleMetadataSaveLocked", 1, owned),
		Samples: []migrationCapacitySample{
			migrationAbsent(caseID, "saveJobs before Observe"),
			migrationSample(caseID, "World.scheduleMetadataSaveLocked", "metadata", 1, owned),
		},
		Families: migrationWithFamily(migrationClosedFamilies(), family),
	})
}
