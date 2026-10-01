package storage

import (
	"context"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"github.com/channing771/mornlea/packages/server/storage/chunk"
	"github.com/channing771/mornlea/packages/server/storage/companion"
	"github.com/channing771/mornlea/packages/server/storage/hostile"
	"github.com/channing771/mornlea/packages/server/storage/player"
	"github.com/channing771/mornlea/packages/server/storage/region"
	"github.com/channing771/mornlea/packages/shared/core"
	"github.com/gofrs/flock"
	"hash/crc32"
	"io"
	"io/fs"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"testing"

	"github.com/channing771/mornlea/packages/server/storage/passive"
)

func TestRuntimeMigrationStartupOnlyIsInsufficient(t *testing.T) {
	root := t.TempDir()
	store, err := OpenDisk(context.Background(), root, OpenOptions{Create: Metadata{FormatVersion: currentMetadataVersion, Seed: 42}})
	if err != nil {
		t.Fatal(err)
	}
	if err := store.Close(); err != nil {
		t.Fatal(err)
	}
	data, err := passive.Encode(PassiveMobsSave{Revision: 1, Records: fixturePassiveRecords()})
	if err != nil {
		t.Fatal(err)
	}
	binary.LittleEndian.PutUint32(data[8:], passive.CurrentSchema+1)
	runtimeMigrationChecksum(data, "passive_mobs.bin")
	if err := os.WriteFile(filepath.Join(root, "passive_mobs.bin"), data, 0600); err != nil {
		t.Fatal(err)
	}
	// Startup only validates metadata and therefore misses this durable future schema.
	startup, err := OpenDisk(context.Background(), root, OpenOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if err := startup.Close(); err != nil {
		t.Fatal(err)
	}
	runtimeMigrationAssert(t, root, false, "passive_mobs.bin")
}

func TestRuntimeMigrationAllFamilies(t *testing.T) {
	root := t.TempDir()
	runtimeMigrationFixture(t, root)
	report := runtimeMigrationAssert(t, root, true, "")
	if report.ReadFiles != 7 {
		t.Fatalf("read_files=%d, want 7", report.ReadFiles)
	}
}

func runtimeMigrationFixture(t *testing.T, root string) {
	t.Helper()
	store, err := OpenDisk(context.Background(), root, OpenOptions{Create: Metadata{FormatVersion: currentMetadataVersion, Seed: 42}})
	if err != nil {
		t.Fatal(err)
	}
	// Fixture writes finish before the measured verifier operation begins.
	defer func() {
		if err := store.Close(); err != nil {
			t.Error(err)
		}
	}()
	for _, id := range []core.PlayerID{fixturePlayerID(), fixtureHostileTargetPlayerID()} {
		if _, err := store.SavePlayer(context.Background(), fixturePlayerSave(id, 1)); err != nil {
			t.Fatal(err)
		}
	}
	if err := store.SaveCompanions(context.Background(), fixtureCompanionV5Save(CompanionSave{Revision: 1, Records: fixtureCompanionBodies()})); err != nil {
		t.Fatal(err)
	}
	if err := store.SaveHostileMobs(context.Background(), HostileMobsSave{Revision: 1, Records: fixtureHostileRecords()}); err != nil {
		t.Fatal(err)
	}
	if err := store.SavePassiveMobs(context.Background(), PassiveMobsSave{Revision: 1, Records: fixturePassiveRecords()}); err != nil {
		t.Fatal(err)
	}
	keys := []core.ChunkKey{{Dimension: core.Overworld, Pos: core.ChunkPos{X: -3, Z: 7}}, {Dimension: core.Overworld, Pos: core.ChunkPos{X: -2, Z: 8}}}
	if _, err := store.SaveBatch(context.Background(), diskSavesFor(keys, 1)); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "opaque.bin"), []byte("immutable opaque data"), 0400); err != nil {
		t.Fatal(err)
	}
}

func runtimeMigrationAssert(t *testing.T, root string, compatible bool, offending string) runtimeMigrationReport {
	t.Helper()
	before := snapshotWorldBackupSource(t, root)
	output := filepath.Join(t.TempDir(), "report.json")
	report := runtimeMigrationVerifyWorld(root, output)
	after := snapshotWorldBackupSource(t, root)
	if !reflect.DeepEqual(before, after) {
		t.Fatal("verifier changed durable paths, modes or contents")
	}
	guard := flock.New(filepath.Join(root, "world.lock"), flock.SetFlag(os.O_RDONLY))
	if _, ok := before["world.lock"]; ok {
		held, err := guard.TryLock()
		if err != nil {
			t.Fatal(err)
		}
		if held {
			if err := guard.Unlock(); err != nil {
				t.Fatal(err)
			}
		}
		if !held {
			t.Fatal("verifier retained world lease")
		}
	}
	data, err := os.ReadFile(output)
	if err != nil {
		t.Fatal(err)
	}
	var emitted runtimeMigrationReport
	if err := json.Unmarshal(data, &emitted); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(report, emitted) {
		t.Fatalf("returned and emitted reports differ: %+v %+v", report, emitted)
	}
	if report.Compatible != compatible {
		t.Fatalf("compatible=%v want %v: %v", report.Compatible, compatible, report.Errors)
	}
	if offending != "" && !strings.Contains(strings.Join(report.Errors, "\n"), offending) {
		t.Fatalf("errors %v omit %q", report.Errors, offending)
	}
	if report.SchemaVersion != 1 || report.SourceSHA != runtimeMigrationSourceSHA || !runtimeMigrationHashValid(report.ExecutableSHA256) {
		t.Fatalf("invalid report identity %+v", report)
	}
	if compatible && (report.ReadFiles == 0 || len(report.Errors) != 0 || !runtimeMigrationHashValid(report.WorldTreeSHA256)) {
		t.Fatalf("invalid compatible report %+v", report)
	}
	if report.WorldTreeSHA256 != "" {
		paths := []string{}
		for path, item := range before {
			if item.Mode.IsRegular() && path != "world.lock" {
				paths = append(paths, filepath.ToSlash(path))
			}
		}
		sort.Strings(paths)
		canonical := []byte{}
		for _, path := range paths {
			canonical = append(canonical, []byte(path)...)
			canonical = append(canonical, 0)
			data := before[filepath.FromSlash(path)].Data
			canonical = binary.LittleEndian.AppendUint64(canonical, uint64(len(data)))
			canonical = append(canonical, []byte(data)...)
		}
		want := sha256.Sum256(canonical)
		if report.WorldTreeSHA256 != hex.EncodeToString(want[:]) {
			t.Fatalf("tree digest=%s want %x", report.WorldTreeSHA256, want)
		}
	}
	return report
}

const runtimeMigrationSourceSHA = "d042982d33bb1694d768b75b01c297bd02534a08"

var errRuntimeMigrationSymlink = errors.New("symlink is forbidden")

type runtimeMigrationReport struct {
	SchemaVersion    uint32   `json:"schema_version"`
	SourceSHA        string   `json:"source_sha"`
	ExecutableSHA256 string   `json:"executable_sha256"`
	WorldTreeSHA256  string   `json:"world_tree_sha256"`
	ReadFiles        uint     `json:"read_files"`
	Errors           []string `json:"errors"`
	Compatible       bool     `json:"compatible"`
}

// This callable test is the offline oracle; it never starts a server or repairs a save.
func TestRuntimeMigrationVerifyWorld(t *testing.T) {
	root, worldSet := os.LookupEnv("MORNLEA_VERIFY_WORLD")
	output, outputSet := os.LookupEnv("MORNLEA_VERIFY_OUTPUT")
	if !worldSet && !outputSet {
		data, err := encodeMetadata(Metadata{FormatVersion: currentMetadataVersion, Seed: 42})
		if err != nil {
			t.Fatal(err)
		}
		got, err := decodeMetadata(data)
		if err != nil || got.Seed != 42 {
			t.Fatalf("built-in metadata characterization: %+v %v", got, err)
		}
		return
	}
	if worldSet != outputSet {
		t.Fatal("MORNLEA_VERIFY_WORLD and MORNLEA_VERIFY_OUTPUT must both be set")
	}
	report := runtimeMigrationVerifyWorld(root, output)
	if !report.Compatible {
		t.Fatalf("world incompatible: %v", report.Errors)
	}
}

func runtimeMigrationHashValid(value string) bool {
	decoded, err := hex.DecodeString(value)
	return err == nil && len(decoded) == sha256.Size && strings.ToLower(value) == value
}

// `runtimeMigrationVerifyWorld` owns the sole lease and emits only outside the input tree.
func runtimeMigrationVerifyWorld(root, output string) runtimeMigrationReport {
	return runtimeMigrationVerifyWorldWithWrite(root, output, os.WriteFile)
}

// Normal qualification uses `os.WriteFile`; the private operation seam exercises actual output I/O failures.
func runtimeMigrationVerifyWorldWithWrite(root, output string, writeFile func(string, []byte, os.FileMode) error) runtimeMigrationReport {
	report := runtimeMigrationReport{SchemaVersion: 1, SourceSHA: runtimeMigrationSourceSHA, Errors: []string{}}
	add := func(path, operation string, err error) {
		report.Errors = append(report.Errors, fmt.Sprintf("%s: %s: %v", path, operation, err))
	}
	executable, err := os.Executable()
	if err == nil {
		report.ExecutableSHA256, err = runtimeMigrationFileHash(executable)
	}
	if err != nil {
		add("executable", "hash", err)
	}
	world, err := filepath.Abs(root)
	if root == "" {
		err = errors.New("world path is empty")
	}
	if err != nil {
		add(".", "absolute world path", err)
	}
	if err == nil {
		err = runtimeMigrationRealPath(world, false)
		if err != nil {
			add(".", "validate world", err)
		}
	}
	var outputErr error
	if errors.Is(err, errRuntimeMigrationSymlink) {
		// Rejected input aliases make lexical output containment unsafe; never follow them to emit a report.
		outputErr = fmt.Errorf("unsafe output context: rejected world alias: %w", err)
	} else {
		outputErr = runtimeMigrationOutputPath(world, output)
	}
	if outputErr != nil {
		add("output", "validate", outputErr)
	}
	if err == nil && outputErr == nil {
		info, statErr := os.Lstat(world)
		if statErr != nil {
			add(".", "stat world", statErr)
		} else if !info.IsDir() {
			add(".", "validate world", errors.New("not a real directory"))
		} else {
			lockPath := filepath.Join(world, "world.lock")
			lockInfo, lockErr := os.Lstat(lockPath)
			if lockErr != nil {
				add("world.lock", "stat", lockErr)
			} else if !lockInfo.Mode().IsRegular() {
				add("world.lock", "validate", errors.New("not a regular file"))
			} else {
				guard := flock.New(lockPath, flock.SetFlag(os.O_RDONLY))
				held, lockErr := guard.TryLock()
				if lockErr != nil {
					add("world.lock", "lock", lockErr)
				} else if !held {
					add("world.lock", "lock", ErrWorldLocked)
				} else {
					// Hashing and decoding occur under the exclusive lease; all descriptors close before the final hash.
					before, paths, modes, treeErr := runtimeMigrationTree(world)
					if treeErr != nil {
						add(".", "inventory/hash before", treeErr)
					} else {
						foundMetadata := false
						for _, rel := range paths {
							if rel == "world.meta" {
								foundMetadata = true
							}
							supported, decodeErr := runtimeMigrationDecode(world, rel)
							if decodeErr != nil {
								add(rel, "read/decode", decodeErr)
							} else if supported {
								report.ReadFiles++
							}
						}
						if !foundMetadata {
							add("world.meta", "required metadata", os.ErrNotExist)
						}
					}
					after, _, afterModes, afterErr := runtimeMigrationTree(world)
					if afterErr != nil {
						add(".", "inventory/hash after", afterErr)
					} else {
						report.WorldTreeSHA256 = after
						if treeErr == nil && (before != after || !reflect.DeepEqual(modes, afterModes)) {
							add(".", "immutability", errors.New("durable tree contents, paths or modes changed"))
						}
					}
				}
				if unlockErr := guard.Unlock(); unlockErr != nil {
					add("world.lock", "unlock", unlockErr)
				}
			}
		}
	}
	sort.Strings(report.Errors)
	report.Compatible = report.ReadFiles > 0 && len(report.Errors) == 0
	if outputErr == nil {
		data, marshalErr := json.MarshalIndent(report, "", "  ")
		if marshalErr == nil {
			marshalErr = writeFile(output, append(data, '\n'), 0600)
		}
		if marshalErr != nil {
			add("output", "write report", marshalErr)
			sort.Strings(report.Errors)
			report.Compatible = false
		}
	}
	return report
}

func runtimeMigrationFileHash(path string) (string, error) {
	file, err := os.Open(path)
	if err != nil {
		return "", err
	}
	hash := sha256.New()
	_, readErr := io.Copy(hash, file)
	if err := errors.Join(readErr, file.Close()); err != nil {
		return "", err
	}
	return hex.EncodeToString(hash.Sum(nil)), nil
}

// Ancestor checks reject aliases without resolving or following symlinks.
func runtimeMigrationRealPath(path string, missingLeaf bool) error {
	path = filepath.Clean(path)
	var pathErrors []error
	for current := path; ; current = filepath.Dir(current) {
		info, err := os.Lstat(current)
		if err != nil {
			if !(missingLeaf && current == path && errors.Is(err, os.ErrNotExist)) {
				pathErrors = append(pathErrors, fmt.Errorf("%s: %w", current, err))
			}
		} else if info.Mode()&os.ModeSymlink != 0 {
			pathErrors = append(pathErrors, fmt.Errorf("%s: %w", current, errRuntimeMigrationSymlink))
		} else if current != path && !info.IsDir() {
			pathErrors = append(pathErrors, fmt.Errorf("%s: ancestor is not a directory", current))
		}
		if filepath.Dir(current) == current {
			break
		}
	}
	return errors.Join(pathErrors...)
}

func runtimeMigrationOutputPath(world, output string) error {
	if !filepath.IsAbs(output) || filepath.Clean(output) != output {
		return errors.New("report path must be absolute and clean")
	}
	rel, err := filepath.Rel(world, output)
	if err != nil {
		return err
	}
	if rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return errors.New("report path must be outside world")
	}
	if err := runtimeMigrationRealPath(output, true); err != nil {
		return err
	}
	info, err := os.Lstat(output)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() {
		return errors.New("report leaf must be absent or regular")
	}
	// A lexically external report must not share a durable world inode, including the lock.
	return filepath.WalkDir(world, func(path string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			if path == world && errors.Is(walkErr, os.ErrNotExist) {
				return nil
			}
			return fmt.Errorf("output identity inventory %s: %w", path, walkErr)
		}
		candidate, err := entry.Info()
		if err != nil {
			return err
		}
		if candidate.Mode().IsRegular() && os.SameFile(info, candidate) {
			rel, err := filepath.Rel(world, path)
			if err != nil {
				return err
			}
			return fmt.Errorf("output aliases world file %s", filepath.ToSlash(rel))
		}
		return nil
	})
}

// The digest matches activation's path/NUL/LE-length/content convention; modes are checked separately.
func runtimeMigrationTree(root string) (string, []string, map[string]os.FileMode, error) {
	paths := []string{}
	modes := map[string]os.FileMode{}
	err := filepath.WalkDir(root, func(path string, entry fs.DirEntry, walkErr error) error {
		rel, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		rel = filepath.ToSlash(rel)
		if walkErr != nil {
			return fmt.Errorf("%s: walk: %w", rel, walkErr)
		}
		info, err := entry.Info()
		if err != nil {
			return fmt.Errorf("%s: stat: %w", rel, err)
		}
		modes[rel] = info.Mode()
		if info.IsDir() {
			return nil
		}
		if !info.Mode().IsRegular() {
			return fmt.Errorf("%s: not a regular file or directory", rel)
		}
		if rel != "world.lock" {
			paths = append(paths, rel)
		}
		return nil
	})
	if err != nil {
		return "", nil, modes, err
	}
	sort.Strings(paths)
	hash := sha256.New()
	for _, rel := range paths {
		file, err := os.Open(filepath.Join(root, filepath.FromSlash(rel)))
		if err != nil {
			return "", nil, modes, fmt.Errorf("%s: open: %w", rel, err)
		}
		info, statErr := file.Stat()
		if statErr != nil {
			return "", nil, modes, fmt.Errorf("%s: stat/close: %w", rel, errors.Join(statErr, file.Close()))
		}
		_, _ = hash.Write([]byte(rel))
		_, _ = hash.Write([]byte{0})
		var length [8]byte
		binary.LittleEndian.PutUint64(length[:], uint64(info.Size()))
		_, _ = hash.Write(length[:])
		read, readErr := io.Copy(hash, file)
		closeErr := file.Close()
		if readErr != nil || closeErr != nil {
			return "", nil, modes, fmt.Errorf("%s: hash/close: %w", rel, errors.Join(readErr, closeErr))
		}
		if read != info.Size() {
			return "", nil, modes, fmt.Errorf("%s: length changed while hashing", rel)
		}
	}
	return hex.EncodeToString(hash.Sum(nil)), paths, modes, nil
}

func runtimeMigrationDecode(root, rel string) (bool, error) {
	path := filepath.Join(root, filepath.FromSlash(rel))
	switch rel {
	case "world.meta":
		file, err := os.Open(path)
		if err != nil {
			return true, err
		}
		data, readErr := io.ReadAll(io.LimitReader(file, 4097))
		if err := errors.Join(readErr, file.Close()); err != nil {
			return true, err
		}
		if len(data) > 4096 {
			return true, errors.New("metadata exceeds bounded read")
		}
		_, err = decodeMetadata(data)
		return true, err
	case "companions.ai":
		data, err := readCompanionFile(path)
		if err == nil {
			_, err = companion.Decode(data)
		}
		return true, err
	case "hostile_mobs.bin":
		data, err := readHostileFile(path)
		if err == nil {
			_, err = hostile.Decode(data)
		}
		return true, err
	case "passive_mobs.bin":
		data, err := readPassiveFile(path)
		if err == nil {
			_, err = passive.Decode(data)
		}
		return true, err
	}
	if strings.HasPrefix(rel, "players/") && strings.HasSuffix(rel, ".player") {
		name := strings.TrimSuffix(strings.TrimPrefix(rel, "players/"), ".player")
		id, err := core.ParsePlayerID(name)
		if err != nil {
			return true, fmt.Errorf("noncanonical player path: %w", err)
		}
		if id.String() != name {
			return true, errors.New("noncanonical player path")
		}
		data, err := readPlayerFile(path)
		if err == nil {
			_, err = player.Decode(id, data)
		}
		return true, err
	}
	if strings.HasPrefix(rel, "dimensions/") && strings.HasSuffix(rel, ".region") {
		key, err := runtimeMigrationRegionKey(rel)
		if err != nil {
			return true, err
		}
		return true, runtimeMigrationRegion(path, key)
	}
	return false, nil
}

func runtimeMigrationInt32(text string) (int32, error) {
	number, err := strconv.ParseInt(text, 10, 32)
	if err != nil {
		return 0, err
	}
	if strconv.FormatInt(number, 10) != text {
		return 0, errors.New("noncanonical signed decimal")
	}
	return int32(number), nil
}

func runtimeMigrationRegionKey(rel string) (region.RegionKey, error) {
	parts := strings.Split(rel, "/")
	if len(parts) != 4 || parts[0] != "dimensions" || parts[2] != "regions" {
		return region.RegionKey{}, errors.New("noncanonical region layout")
	}
	name := strings.Split(parts[3], ".")
	if len(name) != 4 || name[0] != "r" || name[3] != "region" {
		return region.RegionKey{}, errors.New("noncanonical region filename")
	}
	dimension, errD := runtimeMigrationInt32(parts[1])
	x, errX := runtimeMigrationInt32(name[1])
	z, errZ := runtimeMigrationInt32(name[2])
	if err := errors.Join(errD, errX, errZ); err != nil {
		return region.RegionKey{}, fmt.Errorf("noncanonical region coordinates: %w", err)
	}
	return region.RegionKey{Dimension: core.DimensionID(dimension), X: x, Z: z}, nil
}

func runtimeMigrationRegion(path string, key region.RegionKey) error {
	file, err := os.Open(path)
	if err != nil {
		return err
	}
	info, statErr := file.Stat()
	header := make([]byte, region.DataStartSector*region.SectorSize)
	_, readErr := io.ReadFull(file, header)
	if err := errors.Join(statErr, readErr, file.Close()); err != nil {
		return fmt.Errorf("header read/stat/close: %w", err)
	}
	if err := region.DecodeSuperblock(key, header[:region.SectorSize]); err != nil {
		return err
	}
	a, errA := region.DecodeRegionBank(key, header[region.BankAStartSector*region.SectorSize:region.BankAStartSector*region.SectorSize+region.BankSize], info.Size())
	b, errB := region.DecodeRegionBank(key, header[region.BankBStartSector*region.SectorSize:region.BankBStartSector*region.SectorSize+region.BankSize], info.Size())
	// A future standby must refuse even when normal selection could ignore it.
	if errors.Is(errA, ErrFutureVersion) || errors.Is(errB, ErrFutureVersion) {
		return fmt.Errorf("future region bank: %w", errors.Join(errA, errB))
	}
	if _, _, err := region.SelectRegionBank(a, errA, b, errB); err != nil {
		return err
	}
	owner, err := chunk.OpenRegion(context.Background(), path, key)
	if err != nil {
		return err
	}
	var loadErrors []error
	for slot, entry := range owner.Bank().Entries {
		if entry.OffsetSector == 0 {
			continue
		}
		x := int64(key.X)*32 + int64(slot%32)
		z := int64(key.Z)*32 + int64(slot/32)
		if x < math.MinInt32 || x > math.MaxInt32 || z < math.MinInt32 || z > math.MaxInt32 {
			loadErrors = append(loadErrors, fmt.Errorf("slot %d: chunk coordinate overflow", slot))
			continue
		}
		_, err := owner.Load(context.Background(), core.ChunkKey{Dimension: key.Dimension, Pos: core.ChunkPos{X: int32(x), Z: int32(z)}})
		if err != nil {
			loadErrors = append(loadErrors, fmt.Errorf("slot %d: load: %w", slot, err))
		}
	}
	loadErrors = append(loadErrors, owner.Close())
	return errors.Join(loadErrors...)
}

func TestRuntimeMigrationFamilyRefusals(t *testing.T) {
	families := []struct {
		path   string
		schema uint32
		offset int
	}{
		{"world.meta", currentMetadataVersion, 4},
		{"players/" + fixturePlayerID().String() + ".player", player.CurrentSchema, 8},
		{"companions.ai", companion.CurrentSchema, 8},
		{"hostile_mobs.bin", hostile.CurrentSchema, 8},
		{"passive_mobs.bin", passive.CurrentSchema, 8},
		{"dimensions/0/regions/r.-1.0.region", 0, 4},
	}
	for _, family := range families {
		for _, future := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/future=%v", family.path, future), func(t *testing.T) {
				root := t.TempDir()
				runtimeMigrationFixture(t, root)
				path := filepath.Join(root, filepath.FromSlash(family.path))
				data := runtimeMigrationRead(t, path)
				if future {
					schema := family.schema
					if schema == 0 {
						schema = binary.LittleEndian.Uint32(data[family.offset:])
					}
					binary.LittleEndian.PutUint32(data[family.offset:], schema+1)
					runtimeMigrationChecksum(data, family.path)
				} else {
					data[0] ^= 0xff
				}
				runtimeMigrationWrite(t, path, data)
				supported, err := runtimeMigrationDecode(root, family.path)
				want := ErrCorrupt
				if future {
					want = ErrFutureVersion
				}
				if !supported || !errors.Is(err, want) {
					t.Fatalf("source codec error %v, want %v", err, want)
				}
				runtimeMigrationAssert(t, root, false, family.path)
			})
		}
	}
}

func runtimeMigrationChecksum(data []byte, rel string) {
	switch {
	case rel == "world.meta":
		binary.LittleEndian.PutUint32(data[len(data)-4:], crc32.Checksum(data[:len(data)-4], region.CRCTable))
	case strings.HasSuffix(rel, ".region"):
		binary.LittleEndian.PutUint32(data[region.SectorSize-4:], crc32.Checksum(data[:region.SectorSize-4], region.CRCTable))
	default:
		offset, payload := 28, 32
		if strings.HasSuffix(rel, ".player") {
			offset, payload = 40, player.EnvelopeLength
		}
		hash := crc32.New(region.CRCTable)
		_, _ = hash.Write(data[8:offset])
		_, _ = hash.Write(data[payload:])
		binary.LittleEndian.PutUint32(data[offset:], hash.Sum32())
	}
}

func runtimeMigrationRead(t *testing.T, path string) []byte {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return data
}
func runtimeMigrationWrite(t *testing.T, path string, data []byte) {
	t.Helper()
	if err := os.WriteFile(path, data, 0600); err != nil {
		t.Fatal(err)
	}
}

func TestRuntimeMigrationOptionalAndMissing(t *testing.T) {
	for _, missing := range []string{"optional", "world.meta", "world.lock"} {
		t.Run(missing, func(t *testing.T) {
			root := t.TempDir()
			runtimeMigrationFixture(t, root)
			if missing == "optional" {
				for _, path := range []string{"players", "dimensions", "companions.ai", "hostile_mobs.bin", "passive_mobs.bin"} {
					if err := os.RemoveAll(filepath.Join(root, path)); err != nil {
						t.Fatal(err)
					}
				}
				report := runtimeMigrationAssert(t, root, true, "")
				if report.ReadFiles != 1 {
					t.Fatalf("read_files %d, want 1", report.ReadFiles)
				}
			} else {
				if err := os.Remove(filepath.Join(root, missing)); err != nil {
					t.Fatal(err)
				}
				runtimeMigrationAssert(t, root, false, missing)
			}
		})
	}
	t.Run("missing world", func(t *testing.T) {
		root := filepath.Join(t.TempDir(), "missing")
		output := filepath.Join(t.TempDir(), "report.json")
		report := runtimeMigrationVerifyWorld(root, output)
		if report.Compatible || report.ReadFiles != 0 {
			t.Fatalf("missing world report %+v", report)
		}
		if _, err := os.Lstat(root); !errors.Is(err, os.ErrNotExist) {
			t.Fatalf("created missing world: %v", err)
		}
		var emitted runtimeMigrationReport
		if err := json.Unmarshal(runtimeMigrationRead(t, output), &emitted); err != nil || !reflect.DeepEqual(report, emitted) {
			t.Fatalf("missing world report emission %v %+v", err, emitted)
		}
	})
}

func TestRuntimeMigrationLeaseContention(t *testing.T) {
	root := t.TempDir()
	runtimeMigrationFixture(t, root)
	before := snapshotWorldBackupSource(t, root)
	guard := flock.New(filepath.Join(root, "world.lock"), flock.SetFlag(os.O_RDONLY))
	held, err := guard.TryLock()
	if err != nil || !held {
		t.Fatalf("setup lease %v %v", held, err)
	}
	output := filepath.Join(t.TempDir(), "report.json")
	report := runtimeMigrationVerifyWorld(root, output)
	unlockErr := guard.Unlock()
	if unlockErr != nil {
		t.Fatal(unlockErr)
	}
	var emitted runtimeMigrationReport
	if err := json.Unmarshal(runtimeMigrationRead(t, output), &emitted); err != nil || !reflect.DeepEqual(report, emitted) {
		t.Fatalf("contention report emission %v %+v", err, emitted)
	}
	if report.Compatible || report.ReadFiles != 0 || !strings.Contains(strings.Join(report.Errors, "\n"), ErrWorldLocked.Error()) {
		t.Fatalf("contention report %+v", report)
	}
	if !reflect.DeepEqual(before, snapshotWorldBackupSource(t, root)) {
		t.Fatal("contention changed world")
	}
	runtimeMigrationAssert(t, root, true, "")
}

func TestRuntimeMigrationCanonicalPaths(t *testing.T) {
	aliases := []string{
		"players/" + strings.ToUpper(fixturePlayerID().String()) + ".player",
		"players/nested/" + fixturePlayerID().String() + ".player",
		"players/00000000-0000-1000-8000-000000000000.player",
		"dimensions/+0/regions/r.-1.0.region",
		"dimensions/00/regions/r.-1.0.region",
		"dimensions/0/regions/r.-01.0.region",
		"dimensions/0/regions/nested/r.-1.0.region",
	}
	for _, alias := range aliases {
		t.Run(alias, func(t *testing.T) {
			root := t.TempDir()
			runtimeMigrationFixture(t, root)
			path := filepath.Join(root, filepath.FromSlash(alias))
			if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
				t.Fatal(err)
			}
			runtimeMigrationWrite(t, path, []byte("alias"))
			runtimeMigrationAssert(t, root, false, alias)
		})
	}
}

func TestRuntimeMigrationPathSafety(t *testing.T) {
	for _, kind := range []string{"world symlink", "world ancestor symlink", "tree symlink", "nonregular leaf", "lock symlink", "output symlink", "output ancestor symlink", "output inside world", "relative output", "unclean output", "output directory", "missing output parent", "output hardlink metadata", "output hardlink opaque", "output hardlink lock"} {
		t.Run(kind, func(t *testing.T) {
			root := filepath.Join(t.TempDir(), "world")
			runtimeMigrationFixture(t, root)
			actualRoot := root
			output := filepath.Join(t.TempDir(), "report.json")
			switch kind {
			case "world symlink":
				alias := filepath.Join(t.TempDir(), "alias")
				if err := os.Symlink(root, alias); err != nil {
					t.Fatal(err)
				}
				root = alias
			case "world ancestor symlink":
				alias := filepath.Join(t.TempDir(), "alias")
				if err := os.Symlink(filepath.Dir(root), alias); err != nil {
					t.Fatal(err)
				}
				root = filepath.Join(alias, "world")
			case "tree symlink":
				if err := os.Symlink("world.meta", filepath.Join(root, "alias")); err != nil {
					t.Fatal(err)
				}
			case "nonregular leaf":
				if err := syscall.Mkfifo(filepath.Join(root, "fifo"), 0600); err != nil {
					t.Fatal(err)
				}
			case "lock symlink":
				if err := os.Remove(filepath.Join(root, "world.lock")); err != nil {
					t.Fatal(err)
				}
				if err := os.Symlink("world.meta", filepath.Join(root, "world.lock")); err != nil {
					t.Fatal(err)
				}
			case "output symlink":
				target := filepath.Join(t.TempDir(), "target")
				runtimeMigrationWrite(t, target, []byte("keep"))
				if err := os.Symlink(target, output); err != nil {
					t.Fatal(err)
				}
			case "output ancestor symlink":
				parent := t.TempDir()
				alias := filepath.Join(t.TempDir(), "alias")
				if err := os.Symlink(parent, alias); err != nil {
					t.Fatal(err)
				}
				output = filepath.Join(alias, "report.json")
			case "output inside world":
				output = filepath.Join(root, "report.json")
			case "relative output":
				output = "report.json"
			case "unclean output":
				output = filepath.Dir(output) + "/./report.json"
			case "output directory":
				if err := os.Mkdir(output, 0700); err != nil {
					t.Fatal(err)
				}
			case "missing output parent":
				output = filepath.Join(t.TempDir(), "absent", "report.json")
			case "output hardlink metadata", "output hardlink opaque", "output hardlink lock":
				source := "world.meta"
				if kind == "output hardlink opaque" {
					source = "opaque.bin"
				}
				if kind == "output hardlink lock" {
					source = "world.lock"
				}
				if err := os.Link(filepath.Join(root, source), output); err != nil {
					t.Fatal(err)
				}
			}
			before := snapshotWorldBackupSource(t, actualRoot)
			report := runtimeMigrationVerifyWorld(root, output)
			if report.Compatible || len(report.Errors) == 0 {
				t.Fatalf("unsafe path report %+v", report)
			}
			if strings.HasPrefix(kind, "output hardlink") && (report.ReadFiles != 0 || !strings.Contains(strings.Join(report.Errors, "\n"), "output aliases world file")) {
				t.Fatalf("hardlink was not refused before reads: %+v", report)
			}
			if !reflect.DeepEqual(before, snapshotWorldBackupSource(t, actualRoot)) {
				t.Fatal("unsafe path changed world")
			}
			if info, err := os.Lstat(filepath.Join(actualRoot, "world.lock")); err == nil && info.Mode().IsRegular() {
				guard := flock.New(filepath.Join(actualRoot, "world.lock"), flock.SetFlag(os.O_RDONLY))
				held, err := guard.TryLock()
				if held {
					if err := guard.Unlock(); err != nil {
						t.Fatal(err)
					}
				}
				if err != nil || !held {
					t.Fatalf("retained lock: %v", err)
				}
			}
		})
	}
}

const runtimeMigrationRegionRelative = "dimensions/0/regions/r.-1.0.region"

func runtimeMigrationBanks(t *testing.T, data []byte, key region.RegionKey) ([2]region.Bank, int) {
	t.Helper()
	var banks [2]region.Bank
	var bankErrors [2]error
	for index, start := range []int{region.BankAStartSector, region.BankBStartSector} {
		offset := start * region.SectorSize
		banks[index], bankErrors[index] = region.DecodeRegionBank(key, data[offset:offset+region.BankSize], int64(len(data)))
	}
	_, active, err := region.SelectRegionBank(banks[0], bankErrors[0], banks[1], bankErrors[1])
	if err != nil {
		t.Fatal(err)
	}
	return banks, active
}

func runtimeMigrationPutBank(t *testing.T, data []byte, key region.RegionKey, index int, bank region.Bank) {
	t.Helper()
	encoded, err := region.EncodeRegionBank(key, bank)
	if err != nil {
		t.Fatal(err)
	}
	start := region.BankAStartSector
	if index == 1 {
		start = region.BankBStartSector
	}
	copy(data[start*region.SectorSize:], encoded[:])
}

func TestRuntimeMigrationRegionRecovery(t *testing.T) {
	for _, kind := range []string{"corrupt active payload recovery", "corrupt active decode recovery", "future active payload", "corrupt standby fallback", "corrupt active bank fallback", "future standby", "future active bank", "both banks corrupt", "payload without standby", "corrupt payload with malformed standby", "coordinate overflow"} {
		t.Run(kind, func(t *testing.T) {
			root := t.TempDir()
			runtimeMigrationFixture(t, root)
			chunkKey := core.ChunkKey{Dimension: core.Overworld, Pos: core.ChunkPos{X: -3, Z: 7}}
			key, slot := region.RegionFor(chunkKey)
			if kind != "payload without standby" {
				store, err := OpenDisk(context.Background(), root, OpenOptions{})
				if err != nil {
					t.Fatal(err)
				}
				_, saveErr := store.SaveBatch(context.Background(), diskSavesFor([]core.ChunkKey{chunkKey}, 2))
				closeErr := store.Close()
				if err := errors.Join(saveErr, closeErr); err != nil {
					t.Fatal(err)
				}
			}
			path := filepath.Join(root, filepath.FromSlash(runtimeMigrationRegionRelative))
			data := runtimeMigrationRead(t, path)
			banks, active := runtimeMigrationBanks(t, data, key)
			standby := 1 - active
			bankOffset := func(index int) int {
				if index == 0 {
					return region.BankAStartSector * region.SectorSize
				}
				return region.BankBStartSector * region.SectorSize
			}
			compatible := false
			switch kind {
			case "corrupt active payload recovery", "corrupt active decode recovery", "future active payload", "payload without standby", "corrupt payload with malformed standby":
				entry := banks[active].Entries[slot]
				offset := int(entry.OffsetSector) * region.SectorSize
				payload := data[offset : offset+int(entry.PayloadLength)]
				if kind == "future active payload" {
					schema := binary.LittleEndian.Uint32(payload[8:])
					binary.LittleEndian.PutUint32(payload[8:], schema+1)
				} else {
					payload[0] ^= 0xff
				}
				if kind == "corrupt active decode recovery" || kind == "future active payload" {
					banks[active].Entries[slot].PayloadCRC32C = crc32.Checksum(payload, region.CRCTable)
					runtimeMigrationPutBank(t, data, key, active, banks[active])
				}
				if kind == "corrupt payload with malformed standby" {
					data[bankOffset(standby)] ^= 0xff
				}
				compatible = kind == "corrupt active payload recovery" || kind == "corrupt active decode recovery"
			case "corrupt standby fallback":
				data[bankOffset(standby)] ^= 0xff
				compatible = true
			case "corrupt active bank fallback":
				data[bankOffset(active)] ^= 0xff
				compatible = true
			case "future standby", "future active bank":
				index := standby
				if kind == "future active bank" {
					index = active
				}
				// Start with a real encoded bank and maintain its checksum after the future version mutation.
				runtimeMigrationPutBank(t, data, key, index, banks[index])
				offset := bankOffset(index)
				bank := data[offset : offset+region.BankSize]
				binary.LittleEndian.PutUint32(bank[4:], binary.LittleEndian.Uint32(bank[4:])+1)
				binary.LittleEndian.PutUint32(bank[60:], 0)
				binary.LittleEndian.PutUint32(bank[60:], crc32.Checksum(bank, region.CRCTable))
				if _, err := region.DecodeRegionBank(key, bank, int64(len(data))); !errors.Is(err, ErrFutureVersion) {
					t.Fatalf("future bank error identity: %v", err)
				}
			case "both banks corrupt":
				data[bankOffset(0)] ^= 0xff
				data[bankOffset(1)] ^= 0xff
			case "coordinate overflow":
				key.X = math.MaxInt32
				super := region.EncodeSuperblock(key)
				copy(data, super[:])
				for index, bank := range banks {
					runtimeMigrationPutBank(t, data, key, index, bank)
				}
				target := filepath.Join(root, "dimensions/0/regions/r.2147483647.0.region")
				if err := os.Rename(path, target); err != nil {
					t.Fatal(err)
				}
				path = target
			}
			runtimeMigrationWrite(t, path, data)
			rel, err := filepath.Rel(root, path)
			if err != nil {
				t.Fatal(err)
			}
			rel = filepath.ToSlash(rel)
			supported, decodeErr := runtimeMigrationDecode(root, rel)
			if !supported {
				t.Fatal("region not decoded")
			}
			if strings.HasPrefix(kind, "future") && !errors.Is(decodeErr, ErrFutureVersion) {
				t.Fatalf("future region error identity: %v", decodeErr)
			}
			if compatible && decodeErr != nil {
				t.Fatalf("source recovery failed: %v", decodeErr)
			}
			if kind == "coordinate overflow" && !strings.Contains(fmt.Sprint(decodeErr), "coordinate overflow") {
				t.Fatalf("overflow error: %v", decodeErr)
			}
			offending := rel
			if compatible {
				offending = ""
			}
			runtimeMigrationAssert(t, root, compatible, offending)
		})
	}
}

func TestRuntimeMigrationOlderSchemas(t *testing.T) {
	root := t.TempDir()
	runtimeMigrationFixture(t, root)
	playerPath := filepath.Join(root, "players", fixturePlayerID().String()+".player")
	runtimeMigrationWrite(t, playerPath, runtimeMigrationRead(t, "player/testdata/player-v1.bin"))
	key := core.ChunkKey{Dimension: core.Overworld, Pos: core.ChunkPos{X: -3, Z: 7}}
	regionKey, slot := region.RegionFor(key)
	payload := runtimeMigrationRead(t, "chunk/testdata/chunk-v1.bin")
	decoded, err := chunk.Decode(key, 19, payload)
	if err != nil || !decoded.Migrated {
		t.Fatalf("older chunk fixture decode: %+v %v", decoded, err)
	}
	sectors := (len(payload) + region.SectorSize - 1) / region.SectorSize
	data := make([]byte, (region.DataStartSector+sectors)*region.SectorSize)
	super := region.EncodeSuperblock(regionKey)
	copy(data, super[:])
	bank := region.Bank{Generation: 1}
	bank.Entries[slot] = region.Entry{OffsetSector: region.DataStartSector, SectorCount: uint32(sectors), PayloadLength: uint32(len(payload)), Revision: 19, PayloadCRC32C: crc32.Checksum(payload, region.CRCTable)}
	runtimeMigrationPutBank(t, data, regionKey, 0, bank)
	runtimeMigrationPutBank(t, data, regionKey, 1, region.Bank{})
	copy(data[region.DataStartSector*region.SectorSize:], payload)
	runtimeMigrationWrite(t, filepath.Join(root, filepath.FromSlash(runtimeMigrationRegionRelative)), data)
	report := runtimeMigrationAssert(t, root, true, "")
	if report.ReadFiles != 7 {
		t.Fatalf("older read_files=%d", report.ReadFiles)
	}
}

func TestRuntimeMigrationCallableEnvironment(t *testing.T) {
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"MORNLEA_VERIFY_WORLD", "MORNLEA_VERIFY_OUTPUT"} {
		t.Run(name, func(t *testing.T) {
			command := exec.Command(executable, "-test.run=^TestRuntimeMigrationVerifyWorld$", "-test.count=1")
			for _, value := range os.Environ() {
				if !strings.HasPrefix(value, "MORNLEA_VERIFY_WORLD=") && !strings.HasPrefix(value, "MORNLEA_VERIFY_OUTPUT=") {
					command.Env = append(command.Env, value)
				}
			}
			command.Env = append(command.Env, name+"="+t.TempDir())
			output, err := command.CombinedOutput()
			if err == nil || !strings.Contains(string(output), "must both be set") {
				t.Fatalf("one-env callable outcome %v %s", err, output)
			}
		})
	}
}

// Explicit scratch fixtures support standalone test-binary qualification without a product command.
func TestRuntimeMigrationPrepareCallableFixture(t *testing.T) {
	root := os.Getenv("MORNLEA_VERIFY_FIXTURE")
	if root == "" {
		return
	}
	if !filepath.IsAbs(root) || filepath.Clean(root) != root {
		t.Fatal("fixture path must be absolute and clean")
	}
	if _, err := os.Lstat(root); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("fixture destination must be absent: %v", err)
	}
	runtimeMigrationFixture(t, root)
	if os.Getenv("MORNLEA_VERIFY_FIXTURE_FUTURE") == "1" {
		path := filepath.Join(root, "passive_mobs.bin")
		data := runtimeMigrationRead(t, path)
		binary.LittleEndian.PutUint32(data[8:], passive.CurrentSchema+1)
		runtimeMigrationChecksum(data, "passive_mobs.bin")
		runtimeMigrationWrite(t, path, data)
	}
}

func TestRuntimeMigrationAccumulatedErrors(t *testing.T) {
	root := t.TempDir()
	runtimeMigrationFixture(t, root)
	paths := []string{"companions.ai", "hostile_mobs.bin", "passive_mobs.bin"}
	for _, rel := range paths {
		path := filepath.Join(root, rel)
		data := runtimeMigrationRead(t, path)
		data[0] ^= 0xff
		runtimeMigrationWrite(t, path, data)
	}
	report := runtimeMigrationAssert(t, root, false, paths[0])
	if len(report.Errors) != len(paths) || report.ReadFiles != 4 {
		t.Fatalf("accumulated report %+v", report)
	}
	for index, path := range paths {
		if !strings.HasPrefix(report.Errors[index], path+":") {
			t.Fatalf("errors not in path order: %v", report.Errors)
		}
	}
}

func TestRuntimeMigrationBoundedMetadata(t *testing.T) {
	root := t.TempDir()
	runtimeMigrationFixture(t, root)
	runtimeMigrationWrite(t, filepath.Join(root, "world.meta"), make([]byte, 4097))
	runtimeMigrationAssert(t, root, false, "metadata exceeds bounded read")
}

func TestRuntimeMigrationExistingReport(t *testing.T) {
	root := t.TempDir()
	runtimeMigrationFixture(t, root)
	before := snapshotWorldBackupSource(t, root)
	output := filepath.Join(t.TempDir(), "report.json")
	runtimeMigrationWrite(t, output, []byte("previous external report"))
	report := runtimeMigrationVerifyWorld(root, output)
	if !report.Compatible {
		t.Fatalf("existing report refused: %+v", report)
	}
	var emitted runtimeMigrationReport
	if err := json.Unmarshal(runtimeMigrationRead(t, output), &emitted); err != nil || !reflect.DeepEqual(report, emitted) {
		t.Fatalf("existing report emission %v %+v", err, emitted)
	}
	if !reflect.DeepEqual(before, snapshotWorldBackupSource(t, root)) {
		t.Fatal("existing report changed world")
	}
}

func TestRuntimeMigrationRejectedWorldAliasOutput(t *testing.T) {
	for _, ancestor := range []bool{false, true} {
		for _, existing := range []bool{false, true} {
			t.Run(fmt.Sprintf("ancestor=%v/existing=%v", ancestor, existing), func(t *testing.T) {
				realWorld := filepath.Join(t.TempDir(), "world")
				runtimeMigrationFixture(t, realWorld)
				alias := filepath.Join(t.TempDir(), "alias")
				target := realWorld
				if ancestor {
					target = filepath.Dir(realWorld)
				}
				if err := os.Symlink(target, alias); err != nil {
					t.Fatal(err)
				}
				world := alias
				if ancestor {
					world = filepath.Join(alias, "world")
				}
				output := filepath.Join(realWorld, "report.json")
				if existing {
					if err := os.Link(filepath.Join(realWorld, "world.meta"), output); err != nil {
						t.Fatal(err)
					}
				}
				before := snapshotWorldBackupSource(t, realWorld)
				report := runtimeMigrationVerifyWorld(world, output)
				if !reflect.DeepEqual(before, snapshotWorldBackupSource(t, realWorld)) {
					t.Fatal("rejected world alias report mutated its referent")
				}
				if report.Compatible || report.ReadFiles != 0 || !strings.Contains(strings.Join(report.Errors, "\n"), "unsafe output context") {
					t.Fatalf("alias report %+v", report)
				}
				if !existing {
					if _, err := os.Lstat(output); !errors.Is(err, os.ErrNotExist) {
						t.Fatalf("created report in rejected alias referent: %v", err)
					}
				}
				guard := flock.New(filepath.Join(realWorld, "world.lock"), flock.SetFlag(os.O_RDONLY))
				held, err := guard.TryLock()
				if held {
					if err := guard.Unlock(); err != nil {
						t.Fatal(err)
					}
				}
				if err != nil || !held {
					t.Fatalf("alias retained lease %v", err)
				}
			})
		}
	}
}

func TestRuntimeMigrationReportWriteFailure(t *testing.T) {
	root := t.TempDir()
	runtimeMigrationFixture(t, root)
	before := snapshotWorldBackupSource(t, root)
	parent := filepath.Join(t.TempDir(), "output")
	if err := os.Mkdir(parent, 0700); err != nil {
		t.Fatal(err)
	}
	output := filepath.Join(parent, "report.json")
	calls := 0
	var actualWriteErr error
	report := runtimeMigrationVerifyWorldWithWrite(root, output, func(path string, data []byte, mode os.FileMode) error {
		calls++
		// Remove only the disposable output parent after validation, then delegate the real write.
		if err := os.Rename(parent, parent+"-moved"); err != nil {
			t.Fatal(err)
		}
		actualWriteErr = os.WriteFile(path, data, mode)
		return actualWriteErr
	})
	if calls != 1 || !errors.Is(actualWriteErr, os.ErrNotExist) {
		t.Fatalf("actual output write calls=%d err=%v", calls, actualWriteErr)
	}
	if report.Compatible || report.ReadFiles != 7 || len(report.Errors) != 1 || !strings.Contains(report.Errors[0], "output: write report:") {
		t.Fatalf("write failure report %+v", report)
	}
	if _, err := os.Lstat(output); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("claimed emission after failed write: %v", err)
	}
	if !reflect.DeepEqual(before, snapshotWorldBackupSource(t, root)) {
		t.Fatal("output I/O failure changed world")
	}
	guard := flock.New(filepath.Join(root, "world.lock"), flock.SetFlag(os.O_RDONLY))
	held, err := guard.TryLock()
	if held {
		if err := guard.Unlock(); err != nil {
			t.Fatal(err)
		}
	}
	if err != nil || !held {
		t.Fatalf("write failure retained lease %v", err)
	}
}

func TestRuntimeMigrationCallableEmptyEnvironment(t *testing.T) {
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	for _, names := range [][]string{{"MORNLEA_VERIFY_WORLD"}, {"MORNLEA_VERIFY_OUTPUT"}, {"MORNLEA_VERIFY_WORLD", "MORNLEA_VERIFY_OUTPUT"}} {
		t.Run(strings.Join(names, "+"), func(t *testing.T) {
			command := exec.Command(executable, "-test.run=^TestRuntimeMigrationVerifyWorld$", "-test.count=1")
			for _, value := range os.Environ() {
				if !strings.HasPrefix(value, "MORNLEA_VERIFY_WORLD=") && !strings.HasPrefix(value, "MORNLEA_VERIFY_OUTPUT=") {
					command.Env = append(command.Env, value)
				}
			}
			for _, name := range names {
				command.Env = append(command.Env, name+"=")
			}
			output, err := command.CombinedOutput()
			if err == nil {
				t.Fatalf("empty supplied environment falsely accepted: %s", output)
			}
		})
	}
}

func TestRuntimeMigrationMissingWorldAliasAncestor(t *testing.T) {
	referent := t.TempDir()
	alias := filepath.Join(t.TempDir(), "alias")
	if err := os.Symlink(referent, alias); err != nil {
		t.Fatal(err)
	}
	world := filepath.Join(alias, "missing-world")
	output := filepath.Join(referent, "report.json")
	before := snapshotWorldBackupSource(t, referent)
	report := runtimeMigrationVerifyWorld(world, output)
	if !reflect.DeepEqual(before, snapshotWorldBackupSource(t, referent)) {
		t.Fatal("missing world alias emitted into its ancestor referent")
	}
	if report.Compatible || report.ReadFiles != 0 || !strings.Contains(strings.Join(report.Errors, "\n"), "unsafe output context") {
		t.Fatalf("missing alias report %+v", report)
	}
	if !errors.Is(runtimeMigrationRealPath(world, false), errRuntimeMigrationSymlink) {
		t.Fatal("missing leaf hid symlink ancestor identity")
	}
	if _, err := os.Lstat(output); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("created alias report: %v", err)
	}
	if _, err := os.Lstat(world); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("created missing aliased world: %v", err)
	}
}
