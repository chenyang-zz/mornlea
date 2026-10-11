package persistence

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"testing"

	"github.com/channing771/mornlea/packages/server/sim/contract"
	simruntime "github.com/channing771/mornlea/packages/server/sim/runtime"
	"github.com/channing771/mornlea/packages/server/storage"
	companioncodec "github.com/channing771/mornlea/packages/server/storage/companion"
	hostilecodec "github.com/channing771/mornlea/packages/server/storage/hostile"
	passivecodec "github.com/channing771/mornlea/packages/server/storage/passive"
	playercodec "github.com/channing771/mornlea/packages/server/storage/player"
	"github.com/channing771/mornlea/packages/shared/companion"
	"github.com/channing771/mornlea/packages/shared/core"
)

// migrationPlayerLogicalBound is the server default player cap. This package
// does not import the server root, so the bound stays a literal.
const migrationPlayerLogicalBound = 8

func migrationUnblock(results chan error) {
	select {
	case results <- context.Canceled:
	default:
	}
}

func migrationPlayerSnapshot(position float32) contract.PlayerSnapshot {
	snapshot := testPlayerSnapshot(position)
	snapshot.Health = core.MaxHealth
	snapshot.Hunger = core.MaxHunger
	snapshot.SaturationMilli = core.InitialSaturationMilli
	return snapshot
}

func migrationPlayerBytes(t *testing.T, save storage.PlayerSave) int {
	t.Helper()
	encoded, err := playercodec.Encode(save)
	if err != nil {
		t.Fatal(err)
	}
	if len(encoded) == 0 {
		t.Fatal("encoded player is empty")
	}
	return len(encoded)
}

func migrationCompanionBytes(t *testing.T, save storage.CompanionSave) int {
	t.Helper()
	encoded, err := companioncodec.Encode(save)
	if err != nil {
		t.Fatal(err)
	}
	if len(encoded) == 0 {
		t.Fatal("encoded companion aggregate is empty")
	}
	return len(encoded)
}

func migrationHostileBytes(t *testing.T, save storage.HostileMobsSave) int {
	t.Helper()
	encoded, err := hostilecodec.Encode(save)
	if err != nil {
		t.Fatal(err)
	}
	if len(encoded) == 0 {
		t.Fatal("encoded hostile aggregate is empty")
	}
	return len(encoded)
}

func migrationPassiveBytes(t *testing.T, save storage.PassiveMobsSave) int {
	t.Helper()
	encoded, err := passivecodec.Encode(save)
	if err != nil {
		t.Fatal(err)
	}
	if len(encoded) == 0 {
		t.Fatal("encoded passive aggregate is empty")
	}
	return len(encoded)
}

// migrationWorkerlessPlayers builds the player owner without starting the
// scheduler goroutines from newPlayerSaveScheduler.
func migrationWorkerlessPlayers(store storage.PlayerStore, options Options) *Players {
	ctx, cancel := context.WithCancel(context.Background())
	jobs := make(chan playerSaveJob, playerSaveJobCapacity)
	completions := make(chan playerSaveCompletion, playerSaveDoneCapacity)
	return &Players{
		store:   store,
		options: options,
		cache:   make(map[core.PlayerID]*cachedPlayer),
		scheduler: &playerSaveScheduler{
			store:       store,
			jobs:        jobs,
			completions: completions,
			ctx:         ctx,
			cancel:      cancel,
		},
		jobs:        jobs,
		completions: completions,
		done:        make(chan struct{}),
	}
}

func migrationAdmitPlayer(t *testing.T, players *Players, index int, force bool) {
	t.Helper()
	id := playerID(byte(index))
	name := fmt.Sprintf("P%d", index)
	if _, err := players.Prepare(context.Background(), id, name, testMetadata()); err != nil {
		t.Fatal(err)
	}
	if err := players.Activate(id, name); err != nil {
		t.Fatal(err)
	}
	snapshot := migrationPlayerSnapshot(float32(index))
	if err := players.Observe(id, name, snapshot, 1, false); err != nil {
		t.Fatal(err)
	}
	players.Confirm(id)
	if force {
		if err := players.Observe(id, name, snapshot, 1, true); err != nil {
			t.Fatal(err)
		}
	}
}

func TestMigrationActorCapacityReplay(t *testing.T) {
	root := migrationRepoRoot(t)
	ownerDir := filepath.Join(root, "packages/server/server/persistence")
	sourceHash := migrationFilesSHA256(t,
		filepath.Join(ownerDir, "players.go"),
		filepath.Join(ownerDir, "companions.go"),
		filepath.Join(ownerDir, "hostiles.go"),
		filepath.Join(ownerDir, "passives.go"),
		filepath.Join(ownerDir, "world.go"),
	)
	rows := migrationInventoryByID(t, root)
	playerFamily, playerCase := migrationPlayerCapacityCase(t, rows["save.player"])
	companionFamily, companionCase := migrationCompanionCapacityCase(t, rows["save.companion"])
	hostileFamily, hostileCase := migrationHostileCapacityCase(t, rows["save.hostile"])
	passiveFamily, passiveCase := migrationPassiveCapacityCase(t, rows["save.passive"])
	metadataFamily, metadataCase := migrationActorMetadataCase(t, rows["save.world-metadata"])
	if playerCase.CaseID == "" || companionCase.CaseID == "" || hostileCase.CaseID == "" ||
		passiveCase.CaseID == "" || metadataCase.CaseID == "" {
		t.Fatal("actor inventory case was not built")
	}
	families := []migrationFamilyReport{
		playerFamily, companionFamily, hostileFamily, passiveFamily, metadataFamily,
	}
	retained := 0
	owned := 0
	for _, family := range families {
		if !family.Applicable || family.RetainedSnapshots == 0 || family.OwnedBytes == 0 {
			t.Fatalf("family %s is not a measured schedule", family.Family)
		}
		retained += family.RetainedSnapshots
		owned += family.OwnedBytes
	}
	rollup := finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            "actor.supported-retained",
		Supported:         true,
		SourceSHA256:      sourceHash,
		Command:           migrationLaneAbsent(),
		ReadyChunkResults: migrationLaneAbsent(),
		Persistence:       migrationLaneApplicable("actor retained snapshots", retained, owned),
		Samples: []migrationCapacitySample{
			migrationSample("actor.supported-retained", "actor retained snapshots", "actor_snapshots", retained, owned),
		},
		Families: families,
	})
	cases := []migrationCapacityCase{
		playerCase, companionCase, hostileCase, passiveCase, metadataCase, rollup,
	}
	migrationLogCases(t, cases)
	migrationWriteDraft(t, "actor.json", migrationCapacityDraft{
		SourceSHA256: sourceHash,
		Cases:        cases,
	})
}

func migrationPlayerCapacityCase(
	t *testing.T,
	row migrationInventoryRow,
) (migrationFamilyReport, migrationCapacityCase) {
	t.Helper()
	const caseID = "save.player"
	if row.ID != caseID {
		t.Fatal("inventory missing save.player")
	}
	options := playerPersistenceTestConfig()
	players := migrationWorkerlessPlayers(newControllablePlayerStore(), options)
	t.Cleanup(players.Close)
	for index := 1; index <= migrationPlayerLogicalBound; index++ {
		migrationAdmitPlayer(t, players, index, false)
	}
	if err := players.Poll(options.AutosaveTicks); err != nil {
		t.Fatal(err)
	}
	players.mu.Lock()
	selectionBytes := 0
	selectionCount := 0
	for _, player := range players.sortedPlayersLocked(func(player *cachedPlayer) bool {
		return player.hasSnapshot
	}) {
		selectionBytes += migrationPlayerBytes(t, player.save(player.persisted+1))
		selectionCount++
	}
	queuedCount := len(players.jobs)
	players.mu.Unlock()
	if selectionCount != migrationPlayerLogicalBound || queuedCount != migrationPlayerLogicalBound {
		t.Fatalf("player selection=%d queued=%d, want %d", selectionCount, queuedCount, migrationPlayerLogicalBound)
	}
	jobs := make([]playerSaveJob, 0, queuedCount)
	for range queuedCount {
		jobs = append(jobs, <-players.jobs)
	}
	queueBytes := 0
	for _, job := range jobs {
		queueBytes += migrationPlayerBytes(t, job.Save)
	}
	players.completions <- playerSaveCompletion{Job: jobs[0], Err: errors.New("player capacity retry")}
	enqueuedCompletions := len(players.completions)
	completionBytes := migrationPlayerBytes(t, jobs[0].Save)
	if err := players.Poll(1); err == nil {
		t.Fatal("player retry completion was accepted")
	}
	players.mu.Lock()
	retried := players.cache[jobs[0].Save.PlayerID]
	if retried == nil || retried.retry == nil {
		t.Fatal("player retry ticket missing")
	}
	retryBytes := migrationPlayerBytes(t, retried.retry.Save)
	players.mu.Unlock()
	if enqueuedCompletions != 1 || len(players.completions) != 0 {
		t.Fatalf("player completions before=%d after=%d", enqueuedCompletions, len(players.completions))
	}

	heldStore := newControllablePlayerStore()
	live := NewPlayers(heldStore, options)
	t.Cleanup(func() {
		migrationUnblock(heldStore.saveResults)
		live.Close()
	})
	migrationAdmitPlayer(t, live, 1, true)
	held := receivePlayerSave(t, heldStore)
	heldBytes := migrationPlayerBytes(t, held)
	heldStore.complete(nil)

	retained := selectionCount + queuedCount
	owned := selectionBytes + queueBytes
	family := migrationFamilyReport{
		Family:            "player",
		Applicable:        true,
		Count:             selectionCount,
		LogicalBound:      migrationPlayerLogicalBound,
		EncodedEquivalent: selectionBytes,
		RetainedSnapshots: retained,
		OwnedBytes:        owned,
	}
	item := finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            caseID,
		Supported:         true,
		SourceSHA256:      row.SourceSHA256,
		InventoryID:       row.ID,
		Command:           migrationLaneAbsent(),
		ReadyChunkResults: migrationLaneAbsent(),
		Persistence:       migrationLaneApplicable("player retained snapshots", retained, owned),
		Samples: []migrationCapacitySample{
			migrationAbsent(caseID, "Players.inFlightJob"),
			migrationSample(caseID, "cachedPlayer.snapshot", "actor_snapshots", selectionCount, selectionBytes),
			migrationSample(caseID, "playerSaveScheduler.jobs", "actor_snapshots", queuedCount, queueBytes),
			migrationSample(caseID, "player retained snapshots", "actor_snapshots", retained, owned),
			migrationSample(caseID, "PlayerStore.SavePlayer", "actor_snapshots", 1, heldBytes),
			migrationSample(caseID, "saveCompletions on enqueue", "actor_snapshots", enqueuedCompletions, completionBytes),
			migrationSample(caseID, "saveCompletions before drain", "actor_snapshots", enqueuedCompletions, completionBytes),
			migrationAbsent(caseID, "saveCompletions after consume"),
			migrationSample(caseID, "cachedPlayer.retry", "actor_snapshots", 1, retryBytes),
		},
		Families: migrationWithFamily(migrationClosedFamilies(), family),
	})
	return family, item
}

func migrationCompanionCapacityCase(
	t *testing.T,
	row migrationInventoryRow,
) (migrationFamilyReport, migrationCapacityCase) {
	t.Helper()
	const caseID = "save.companion"
	if row.ID != caseID {
		t.Fatal("inventory missing save.companion")
	}
	options := companionPersistenceTestOptions()
	loaded := persistenceV5Loaded(1)
	ctx, cancel := context.WithCancel(context.Background())
	owner := &Companions{
		store:        newControllableCompanionStore(),
		options:      options,
		records:      cloneAndSortCompanionBodies(loaded.Records),
		namespace:    loaded.AgentNamespaceID,
		lifecycles:   append([]storage.StoredCompanionLifecycle(nil), loaded.Lifecycles...),
		loadedQueues: cloneStoredQueues(loaded.Queues),
		persisted:    loaded.Revision,
		jobs:         make(chan companionSaveJob, 1),
		completions:  make(chan companionSaveCompletion, 1),
		ctx:          ctx,
		cancel:       cancel,
	}
	t.Cleanup(owner.Close)
	active := companionBody(1, 33)
	owner.Observe([]companion.Body{active}, []companion.TaskQueueState{{
		ID: active.ID, Pending: []companion.TaskCommand{"queued"},
	}})
	owner.mu.Lock()
	selection, err := owner.latestJobLocked()
	recordCount := len(owner.records)
	owner.mu.Unlock()
	if err != nil {
		t.Fatal(err)
	}
	selectionBytes := migrationCompanionBytes(t, selection.Save)
	if err := owner.Poll(options.AutosaveTicks); err != nil {
		t.Fatal(err)
	}
	owner.mu.Lock()
	if !owner.inFlight {
		t.Fatal("companion dispatch did not retain inFlightJob")
	}
	inFlightBytes := migrationCompanionBytes(t, owner.inFlightJob.Save)
	queuedCount := len(owner.jobs)
	owner.mu.Unlock()
	if queuedCount != 1 || recordCount == 0 {
		t.Fatalf("companion records=%d queued=%d", recordCount, queuedCount)
	}
	queued := <-owner.jobs
	queueBytes := migrationCompanionBytes(t, queued.Save)
	owner.completions <- companionSaveCompletion{Job: queued, Err: errors.New("companion capacity retry")}
	enqueuedCompletions := len(owner.completions)
	if err := owner.Poll(options.AutosaveTicks); err == nil {
		t.Fatal("companion retry completion was accepted")
	}
	owner.mu.Lock()
	if owner.retry == nil {
		t.Fatal("companion retry ticket missing")
	}
	retryBytes := migrationCompanionBytes(t, owner.retry.Save)
	owner.mu.Unlock()
	if enqueuedCompletions != 1 || len(owner.completions) != 0 {
		t.Fatalf("companion completions before=%d after=%d", enqueuedCompletions, len(owner.completions))
	}

	heldStore := newControllableCompanionStore()
	live := NewCompanions(heldStore, persistenceV5Loaded(1), options)
	t.Cleanup(func() {
		migrationUnblock(heldStore.results)
		live.Close()
	})
	live.Observe([]companion.Body{active}, nil)
	if err := live.Poll(options.AutosaveTicks); err != nil {
		t.Fatal(err)
	}
	held := receiveCompanionSave(t, heldStore)
	heldBytes := migrationCompanionBytes(t, held)
	live.mu.Lock()
	if !live.inFlight {
		t.Fatal("companion worker lost inFlightJob while SaveCompanions was blocked")
	}
	liveInFlightBytes := migrationCompanionBytes(t, live.inFlightJob.Save)
	live.mu.Unlock()
	heldStore.complete(nil)

	retained := 3
	owned := selectionBytes + queueBytes + inFlightBytes
	family := migrationFamilyReport{
		Family:            "companion",
		Applicable:        true,
		Count:             recordCount,
		LogicalBound:      companion.MaxStored,
		EncodedEquivalent: selectionBytes,
		RetainedSnapshots: retained,
		OwnedBytes:        owned,
	}
	item := finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            caseID,
		Supported:         true,
		SourceSHA256:      row.SourceSHA256,
		InventoryID:       row.ID,
		Command:           migrationLaneAbsent(),
		ReadyChunkResults: migrationLaneAbsent(),
		Persistence:       migrationLaneApplicable("companion retained snapshots", retained, owned),
		Samples: []migrationCapacitySample{
			migrationSample(caseID, "Companions.records", "actor_snapshots", 1, selectionBytes),
			migrationSample(caseID, "Companions.jobs", "actor_snapshots", queuedCount, queueBytes),
			migrationSample(caseID, "Companions.inFlightJob", "actor_snapshots", 1, inFlightBytes),
			migrationSample(caseID, "companion retained snapshots", "actor_snapshots", retained, owned),
			migrationSample(caseID, "CompanionStore.SaveCompanions", "actor_snapshots", 1, heldBytes),
			migrationSample(caseID, "Companions.inFlightJob held", "actor_snapshots", 1, liveInFlightBytes),
			migrationSample(caseID, "saveCompletions on enqueue", "actor_snapshots", enqueuedCompletions, queueBytes),
			migrationSample(caseID, "saveCompletions before drain", "actor_snapshots", enqueuedCompletions, queueBytes),
			migrationAbsent(caseID, "saveCompletions after consume"),
			migrationSample(caseID, "Companions.retry", "actor_snapshots", 1, retryBytes),
		},
		Families: migrationWithFamily(migrationClosedFamilies(), family),
	})
	return family, item
}

func migrationHostileCapacityCase(
	t *testing.T,
	row migrationInventoryRow,
) (migrationFamilyReport, migrationCapacityCase) {
	t.Helper()
	const caseID = "save.hostile"
	if row.ID != caseID {
		t.Fatal("inventory missing save.hostile")
	}
	options := hostilePersistenceTestOptions()
	ctx, cancel := context.WithCancel(context.Background())
	owner := &Hostiles{
		store:       newControllableHostileStore(),
		options:     options,
		jobs:        make(chan hostileSaveJob, 1),
		completions: make(chan hostileSaveCompletion, 1),
		ctx:         ctx,
		cancel:      cancel,
	}
	t.Cleanup(owner.Close)
	owner.Observe([]contract.HostileMob{hostileObserveFixture(1, 4)})
	owner.mu.Lock()
	selection := owner.latestJobLocked()
	recordCount := len(owner.records)
	owner.mu.Unlock()
	selectionBytes := migrationHostileBytes(t, selection.Save)
	if err := owner.Poll(options.AutosaveTicks); err != nil {
		t.Fatal(err)
	}
	owner.mu.Lock()
	if !owner.inFlight {
		t.Fatal("hostile dispatch did not retain inFlightJob")
	}
	inFlightBytes := migrationHostileBytes(t, owner.inFlightJob.Save)
	queuedCount := len(owner.jobs)
	owner.mu.Unlock()
	if queuedCount != 1 || recordCount != 1 {
		t.Fatalf("hostile records=%d queued=%d", recordCount, queuedCount)
	}
	queued := <-owner.jobs
	queueBytes := migrationHostileBytes(t, queued.Save)
	owner.completions <- hostileSaveCompletion{Job: queued, Err: errors.New("hostile capacity retry")}
	enqueuedCompletions := len(owner.completions)
	if err := owner.Poll(options.AutosaveTicks); err == nil {
		t.Fatal("hostile retry completion was accepted")
	}
	owner.mu.Lock()
	if owner.retry == nil {
		t.Fatal("hostile retry ticket missing")
	}
	retryBytes := migrationHostileBytes(t, owner.retry.Save)
	owner.mu.Unlock()

	heldStore := newControllableHostileStore()
	live := NewHostiles(heldStore, storage.StoredHostileMobs{}, options)
	t.Cleanup(func() {
		migrationUnblock(heldStore.results)
		live.Close()
	})
	live.Observe([]contract.HostileMob{hostileObserveFixture(1, 6)})
	if err := live.Poll(options.AutosaveTicks); err != nil {
		t.Fatal(err)
	}
	held := receiveHostileSave(t, heldStore)
	heldBytes := migrationHostileBytes(t, held)
	live.mu.Lock()
	if !live.inFlight {
		t.Fatal("hostile worker lost inFlightJob while SaveHostileMobs was blocked")
	}
	liveInFlightBytes := migrationHostileBytes(t, live.inFlightJob.Save)
	live.mu.Unlock()
	heldStore.complete(nil)

	retained := 3
	owned := selectionBytes + queueBytes + inFlightBytes
	family := migrationFamilyReport{
		Family:            "hostile",
		Applicable:        true,
		Count:             recordCount,
		LogicalBound:      storage.MaxHostileMobs,
		EncodedEquivalent: selectionBytes,
		RetainedSnapshots: retained,
		OwnedBytes:        owned,
	}
	item := finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            caseID,
		Supported:         true,
		SourceSHA256:      row.SourceSHA256,
		InventoryID:       row.ID,
		Command:           migrationLaneAbsent(),
		ReadyChunkResults: migrationLaneAbsent(),
		Persistence:       migrationLaneApplicable("hostile retained snapshots", retained, owned),
		Samples: []migrationCapacitySample{
			migrationSample(caseID, "Hostiles.records", "actor_snapshots", 1, selectionBytes),
			migrationSample(caseID, "Hostiles.jobs", "actor_snapshots", queuedCount, queueBytes),
			migrationSample(caseID, "Hostiles.inFlightJob", "actor_snapshots", 1, inFlightBytes),
			migrationSample(caseID, "hostile retained snapshots", "actor_snapshots", retained, owned),
			migrationSample(caseID, "HostileMobStore.SaveHostileMobs", "actor_snapshots", 1, heldBytes),
			migrationSample(caseID, "Hostiles.inFlightJob held", "actor_snapshots", 1, liveInFlightBytes),
			migrationSample(caseID, "saveCompletions on enqueue", "actor_snapshots", enqueuedCompletions, queueBytes),
			migrationSample(caseID, "saveCompletions before drain", "actor_snapshots", enqueuedCompletions, queueBytes),
			migrationAbsent(caseID, "saveCompletions after consume"),
			migrationSample(caseID, "Hostiles.retry", "actor_snapshots", 1, retryBytes),
		},
		Families: migrationWithFamily(migrationClosedFamilies(), family),
	})
	return family, item
}

func migrationPassiveCapacityCase(
	t *testing.T,
	row migrationInventoryRow,
) (migrationFamilyReport, migrationCapacityCase) {
	t.Helper()
	const caseID = "save.passive"
	if row.ID != caseID {
		t.Fatal("inventory missing save.passive")
	}
	options := passivePersistenceTestOptions()
	ctx, cancel := context.WithCancel(context.Background())
	owner := &Passives{
		store:       newControllablePassiveStore(),
		options:     options,
		jobs:        make(chan passiveSaveJob, 1),
		completions: make(chan passiveSaveCompletion, 1),
		ctx:         ctx,
		cancel:      cancel,
	}
	t.Cleanup(owner.Close)
	owner.Observe([]contract.PassiveMob{passiveObserveFixture(1, 4)})
	owner.mu.Lock()
	selection := owner.latestJobLocked()
	recordCount := len(owner.records)
	owner.mu.Unlock()
	selectionBytes := migrationPassiveBytes(t, selection.Save)
	if err := owner.Poll(options.AutosaveTicks); err != nil {
		t.Fatal(err)
	}
	owner.mu.Lock()
	if !owner.inFlight {
		t.Fatal("passive dispatch did not retain inFlightJob")
	}
	inFlightBytes := migrationPassiveBytes(t, owner.inFlightJob.Save)
	queuedCount := len(owner.jobs)
	owner.mu.Unlock()
	if queuedCount != 1 || recordCount != 1 {
		t.Fatalf("passive records=%d queued=%d", recordCount, queuedCount)
	}
	queued := <-owner.jobs
	queueBytes := migrationPassiveBytes(t, queued.Save)
	owner.completions <- passiveSaveCompletion{Job: queued, Err: errors.New("passive capacity retry")}
	enqueuedCompletions := len(owner.completions)
	if err := owner.Poll(options.AutosaveTicks); err == nil {
		t.Fatal("passive retry completion was accepted")
	}
	owner.mu.Lock()
	if owner.retry == nil {
		t.Fatal("passive retry ticket missing")
	}
	retryBytes := migrationPassiveBytes(t, owner.retry.Save)
	owner.mu.Unlock()

	heldStore := newControllablePassiveStore()
	live := NewPassives(heldStore, storage.StoredPassiveMobs{}, options)
	t.Cleanup(func() {
		migrationUnblock(heldStore.results)
		live.Close()
	})
	live.Observe([]contract.PassiveMob{passiveObserveFixture(1, 6)})
	if err := live.Poll(options.AutosaveTicks); err != nil {
		t.Fatal(err)
	}
	held := receivePassiveSave(t, heldStore)
	heldBytes := migrationPassiveBytes(t, held)
	live.mu.Lock()
	if !live.inFlight {
		t.Fatal("passive worker lost inFlightJob while SavePassiveMobs was blocked")
	}
	liveInFlightBytes := migrationPassiveBytes(t, live.inFlightJob.Save)
	live.mu.Unlock()
	heldStore.complete(nil)

	retained := 3
	owned := selectionBytes + queueBytes + inFlightBytes
	family := migrationFamilyReport{
		Family:            "passive",
		Applicable:        true,
		Count:             recordCount,
		LogicalBound:      storage.MaxPassiveMobs,
		EncodedEquivalent: selectionBytes,
		RetainedSnapshots: retained,
		OwnedBytes:        owned,
	}
	item := finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            caseID,
		Supported:         true,
		SourceSHA256:      row.SourceSHA256,
		InventoryID:       row.ID,
		Command:           migrationLaneAbsent(),
		ReadyChunkResults: migrationLaneAbsent(),
		Persistence:       migrationLaneApplicable("passive retained snapshots", retained, owned),
		Samples: []migrationCapacitySample{
			migrationSample(caseID, "Passives.records", "actor_snapshots", 1, selectionBytes),
			migrationSample(caseID, "Passives.jobs", "actor_snapshots", queuedCount, queueBytes),
			migrationSample(caseID, "Passives.inFlightJob", "actor_snapshots", 1, inFlightBytes),
			migrationSample(caseID, "passive retained snapshots", "actor_snapshots", retained, owned),
			migrationSample(caseID, "PassiveMobStore.SavePassiveMobs", "actor_snapshots", 1, heldBytes),
			migrationSample(caseID, "Passives.inFlightJob held", "actor_snapshots", 1, liveInFlightBytes),
			migrationSample(caseID, "saveCompletions on enqueue", "actor_snapshots", enqueuedCompletions, queueBytes),
			migrationSample(caseID, "saveCompletions before drain", "actor_snapshots", enqueuedCompletions, queueBytes),
			migrationAbsent(caseID, "saveCompletions after consume"),
			migrationSample(caseID, "Passives.retry", "actor_snapshots", 1, retryBytes),
		},
		Families: migrationWithFamily(migrationClosedFamilies(), family),
	})
	return family, item
}

func migrationActorMetadataCase(
	t *testing.T,
	row migrationInventoryRow,
) (migrationFamilyReport, migrationCapacityCase) {
	t.Helper()
	const caseID = "save.world-metadata"
	if row.ID != caseID {
		t.Fatal("inventory missing save.world-metadata")
	}
	options := persistenceTestOptions()
	options.AutosaveTicks = 1
	engine := simruntime.NewEngine(0, 0, 42)
	world := migrationWorkerlessWorld(newPersistenceTestStore(), engine, options)
	world.Observe(options.AutosaveTicks, 7000)
	if len(world.saveJobs) != 1 {
		t.Fatalf("actor metadata jobs=%d, want 1", len(world.saveJobs))
	}
	job := <-world.saveJobs
	if job.Kind != saveKindMetadata {
		t.Fatalf("actor metadata kind=%d", job.Kind)
	}
	owned := migrationMetadataEncodedLength(t, job.Metadata)
	family := migrationFamilyReport{
		Family:            "metadata",
		Applicable:        true,
		Count:             1,
		LogicalBound:      1,
		EncodedEquivalent: owned,
		RetainedSnapshots: 1,
		OwnedBytes:        owned,
	}
	item := finalizeMigrationCase(t, migrationCapacityCase{
		CaseID:            caseID,
		Supported:         true,
		SourceSHA256:      row.SourceSHA256,
		InventoryID:       row.ID,
		Command:           migrationLaneAbsent(),
		ReadyChunkResults: migrationLaneAbsent(),
		Persistence:       migrationLaneApplicable("World.scheduleMetadataSaveLocked", 1, owned),
		Samples: []migrationCapacitySample{
			migrationAbsent(caseID, "Players.inFlightJob"),
			migrationSample(caseID, "World.scheduleMetadataSaveLocked", "metadata", 1, owned),
		},
		Families: migrationWithFamily(migrationClosedFamilies(), family),
	})
	return family, item
}
