package config

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"
)

// The parity fixtures are shared byte-for-byte with the Rust server's
// `runtime::config` loader (packages/engine/crates/mornlea_server). A config
// file must roll back and forth between the Go and Rust servers, so both sides
// must accept and reject exactly the same files:
//
//   - bad/: `decodeConfig` must reject every file.
//   - good/: `decodeConfig` must accept every file, and the server-owned
//     effective values (physics, sim, fluidEnabled, logging) must equal
//     expected/<name>.json, which every good fixture must have; the Rust test
//     checks the same expectation.
//   - known_difference/: Go accepts and Rust deliberately rejects; the list is
//     documented in the rust-authoritative-server design. Pinning Go acceptance
//     here keeps each documented difference real.
//
// Companion fixtures name parityAPIKeyEnv (set) or parityUnsetEnv (empty) so
// the environment check in `applyAI` is deterministic.
const (
	parityFixtureDir = "testdata/parity"
	parityAPIKeyEnv  = "MORNLEA_CONFIG_PARITY_API_KEY"
	parityUnsetEnv   = "MORNLEA_CONFIG_PARITY_UNSET"
	// parityUpdateEnv regenerates expected/ from the Go decoder when set to 1.
	parityUpdateEnv = "MORNLEA_CONFIG_PARITY_UPDATE"
)

// parityGroups lists every top-level surface `decodeConfig` type-checks plus
// the syntax and top-level shape cases; each needs at least one bad fixture.
var parityGroups = []string{
	"syntax", "top_level", "version", "logging", "ai", "texturePackPath",
	"audioVolume", "windowSize", "fluidEnabled", "cameraMode", "physics", "sim", "render",
}

// parityKnownDifferences lists the known_difference/ fixtures; it must match
// the design's known-differences list and the Rust test's pinned errors.
var parityKnownDifferences = []string{
	"ambiguous_case_only_keys",
	"nesting_at_go_limit",
	"nesting_over_serde_limit",
	"unknown_key_huge_number",
	"unknown_key_invalid_utf8_bytes",
	"unknown_key_lone_surrogate_escape",
}

func parityFixtures(t *testing.T, kind string) []string {
	t.Helper()
	paths, err := filepath.Glob(filepath.Join(parityFixtureDir, kind, "*.json"))
	if err != nil {
		t.Fatalf("glob %s fixtures: %v", kind, err)
	}
	if len(paths) == 0 {
		t.Fatalf("no %s fixtures under %s", kind, parityFixtureDir)
	}
	sort.Strings(paths)
	return paths
}

func decodeParityFixture(t *testing.T, path string) (Config, error) {
	t.Helper()
	contents, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read fixture %s: %v", path, err)
	}
	// A temp config path keeps persona lookup away from the fixture tree.
	return decodeConfig(filepath.Join(t.TempDir(), "config.json"), contents)
}

func TestDecodeConfigParityFixtures(t *testing.T) {
	t.Setenv(parityAPIKeyEnv, "parity-key")
	t.Setenv(parityUnsetEnv, "")

	covered := make(map[string]bool)
	for _, path := range parityFixtures(t, "bad") {
		name := strings.TrimSuffix(filepath.Base(path), ".json")
		for _, group := range parityGroups {
			if strings.HasPrefix(name, group+"_") {
				covered[group] = true
			}
		}
		t.Run("bad/"+name, func(t *testing.T) {
			if _, err := decodeParityFixture(t, path); err == nil {
				t.Fatalf("decodeConfig accepted %s; the fixture must be rejected", path)
			}
		})
	}
	for _, group := range parityGroups {
		if !covered[group] {
			t.Errorf("no bad fixture covers %q", group)
		}
	}

	update := os.Getenv(parityUpdateEnv) == "1"
	for _, path := range parityFixtures(t, "good") {
		name := strings.TrimSuffix(filepath.Base(path), ".json")
		t.Run("good/"+name, func(t *testing.T) {
			cfg, err := decodeParityFixture(t, path)
			if err != nil {
				t.Fatalf("decodeConfig rejected %s: %v", path, err)
			}
			checkParityExpectation(t, name, cfg, update)
		})
	}
	var knownNames []string
	for _, path := range parityFixtures(t, "known_difference") {
		name := strings.TrimSuffix(filepath.Base(path), ".json")
		knownNames = append(knownNames, name)
		t.Run("known_difference/"+name, func(t *testing.T) {
			if _, err := decodeParityFixture(t, path); err != nil {
				t.Fatalf("decodeConfig rejected %s (%v); a documented difference needs Go acceptance", path, err)
			}
		})
	}
	if !reflect.DeepEqual(knownNames, parityKnownDifferences) {
		t.Errorf("known_difference fixtures = %v, want the documented %v", knownNames, parityKnownDifferences)
	}
}

// parityValues flattens the effective values the Rust server freezes. Numbers
// are float64 so integer and float32 fields compare exactly on both sides.
func parityValues(cfg Config) map[string]any {
	values := map[string]any{
		"fluidEnabled":    cfg.FluidEnabled,
		"logging.default": float64(cfg.Logging.Default),
	}
	modules := make(map[string]any, len(cfg.Logging.Modules))
	for name, level := range cfg.Logging.Modules {
		modules[name] = float64(level)
	}
	values["logging.modules"] = modules
	for _, field := range Fields() {
		if field.Group != "physics" && field.Group != "sim" {
			continue
		}
		target := groupValue(&cfg, field.Group).FieldByName(exportedFieldName(field.Name))
		var number float64
		switch target.Kind() {
		case reflect.Float32, reflect.Float64:
			number = target.Float()
		case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
			number = float64(target.Int())
		default:
			number = float64(target.Uint())
		}
		values[field.Group+"."+field.Name] = number
	}
	return values
}

func checkParityExpectation(t *testing.T, name string, cfg Config, update bool) {
	t.Helper()
	path := filepath.Join(parityFixtureDir, "expected", name+".json")
	got := parityValues(cfg)
	if update {
		body, err := json.MarshalIndent(got, "", "  ")
		if err != nil {
			t.Fatalf("marshal expectation: %v", err)
		}
		if err := os.WriteFile(path, append(body, '\n'), 0o644); err != nil {
			t.Fatalf("write expectation %s: %v", path, err)
		}
		return
	}
	contents, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read expectation %s: %v; every good fixture needs one (regenerate with %s=1)", path, err, parityUpdateEnv)
	}
	var want map[string]any
	decoder := json.NewDecoder(bytes.NewReader(contents))
	if err := decoder.Decode(&want); err != nil {
		t.Fatalf("decode expectation %s: %v", path, err)
	}
	// Round-trip through JSON so both sides use the same number representation.
	body, err := json.Marshal(got)
	if err != nil {
		t.Fatalf("marshal values: %v", err)
	}
	var normalized map[string]any
	if err := json.Unmarshal(body, &normalized); err != nil {
		t.Fatalf("normalize values: %v", err)
	}
	if !reflect.DeepEqual(normalized, want) {
		t.Fatalf("effective values for %s differ from %s:\n got %v\nwant %v", name, path, normalized, want)
	}
}
