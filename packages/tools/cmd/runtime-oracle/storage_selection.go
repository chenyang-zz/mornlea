package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

const (
	maxStorageArgumentsBytes = 1024
	regionOrderInputBytes    = 57344
)

var (
	playerIDPattern      = regexp.MustCompile(`^[0-9a-f]{32}$`)
	decimalStringPattern = regexp.MustCompile(`^-?(0|[1-9][0-9]*)$`)
	uDecimalPattern      = regexp.MustCompile(`^(0|[1-9][0-9]*)$`)
)

func validateStorageArguments(family, operation string, raw json.RawMessage) error {
	if operation == "order" && family != "save.region" {
		return fmt.Errorf("operation order is permitted only for save.region")
	}
	if len(raw) > maxStorageArgumentsBytes {
		return fmt.Errorf("arguments exceed %d byte budget", maxStorageArgumentsBytes)
	}
	payload := raw
	if len(payload) == 0 {
		payload = json.RawMessage(`{}`)
	}
	if err := validateNoDuplicateKeys(payload); err != nil {
		return fmt.Errorf("duplicate keys: %w", err)
	}
	var obj map[string]json.RawMessage
	dec := json.NewDecoder(bytes.NewReader(payload))
	dec.UseNumber()
	if err := dec.Decode(&obj); err != nil {
		return fmt.Errorf("arguments must be one JSON object: %w", err)
	}
	var extra any
	if err := dec.Decode(&extra); err != io.EOF {
		return fmt.Errorf("arguments must be one JSON object")
	}

	switch family {
	case "save.player":
		return validatePlayerArguments(operation, obj)
	case "save.chunk":
		return validateChunkArguments(operation, obj)
	case "save.region":
		return validateRegionArguments(operation, obj)
	case "save.world-metadata", "save.hostile", "save.passive", "save.companion":
		return validateMetadataFamilyArguments(operation, obj)
	default:
		if strings.HasPrefix(family, "save.") {
			return fmt.Errorf("unsupported save family %s", family)
		}
		return fmt.Errorf("not a save family")
	}
}

func validateSaveCaseInputConstraints(root string, c CaseSpec) error {
	if c.Family != "save.region" || c.Operation != "order" {
		return nil
	}
	fullPath := filepath.Join(root, filepath.FromSlash(c.Input.Path))
	info, err := os.Stat(fullPath)
	if err != nil {
		return fmt.Errorf("stat region order input: %w", err)
	}
	if info.Size() != int64(regionOrderInputBytes) {
		return fmt.Errorf("region order input must be %d bytes, got %d", regionOrderInputBytes, info.Size())
	}
	return nil
}

func validatePlayerArguments(operation string, obj map[string]json.RawMessage) error {
	allowed := map[string]bool{"requested_player_id": true, "capacity": true}
	if err := rejectUnknownKeys(obj, allowed); err != nil {
		return err
	}
	idRaw, ok := obj["requested_player_id"]
	if !ok {
		return fmt.Errorf("missing requested_player_id")
	}
	var id string
	if err := json.Unmarshal(idRaw, &id); err != nil || !playerIDPattern.MatchString(id) {
		return fmt.Errorf("requested_player_id must be 32 lowercase hex digits")
	}
	if raw, ok := obj["capacity"]; ok {
		if operation != "encode" {
			return fmt.Errorf("capacity is permitted only on encode")
		}
		if err := validateCapacityArgument(raw); err != nil {
			return err
		}
	}
	return nil
}

func validateChunkArguments(operation string, obj map[string]json.RawMessage) error {
	allowed := map[string]bool{"dimension": true, "x": true, "z": true, "revision": true}
	if err := rejectUnknownKeys(obj, allowed); err != nil {
		return err
	}
	if _, ok := obj["capacity"]; ok {
		return fmt.Errorf("chunk arguments forbid capacity")
	}
	for _, key := range []string{"dimension", "x", "z"} {
		if err := requireSignedInt32Field(obj, key); err != nil {
			return err
		}
	}
	revRaw, ok := obj["revision"]
	if !ok {
		return fmt.Errorf("missing revision")
	}
	var revString string
	if err := json.Unmarshal(revRaw, &revString); err != nil || !uDecimalPattern.MatchString(revString) {
		return fmt.Errorf("revision must be an unsigned decimal string")
	}
	return nil
}

func validateRegionArguments(operation string, obj map[string]json.RawMessage) error {
	allowed := map[string]bool{"dimension": true, "x": true, "z": true, "file_size": true, "component": true, "capacity": true}
	if err := rejectUnknownKeys(obj, allowed); err != nil {
		return err
	}
	for _, key := range []string{"dimension", "x", "z"} {
		if err := requireSignedInt32Field(obj, key); err != nil {
			return err
		}
	}
	sizeRaw, ok := obj["file_size"]
	if !ok {
		return fmt.Errorf("missing file_size")
	}
	var sizeString string
	if err := json.Unmarshal(sizeRaw, &sizeString); err != nil || !decimalStringPattern.MatchString(sizeString) {
		return fmt.Errorf("file_size must be a signed decimal string")
	}
	componentRaw, ok := obj["component"]
	if !ok {
		return fmt.Errorf("missing component")
	}
	var component string
	if err := json.Unmarshal(componentRaw, &component); err != nil {
		return fmt.Errorf("component must be a string")
	}
	switch operation {
	case "order":
		if component != "banks" {
			return fmt.Errorf("region order requires component \"banks\"")
		}
	case "decode", "encode":
		if component != "bank" && component != "superblock" {
			return fmt.Errorf("region %s requires component \"bank\" or \"superblock\"", operation)
		}
	default:
		return fmt.Errorf("invalid operation %q", operation)
	}
	if raw, ok := obj["capacity"]; ok {
		if operation != "encode" {
			return fmt.Errorf("capacity is permitted only on encode")
		}
		if err := validateCapacityArgument(raw); err != nil {
			return err
		}
	}
	return nil
}

func validateMetadataFamilyArguments(operation string, obj map[string]json.RawMessage) error {
	allowed := map[string]bool{"capacity": true}
	if err := rejectUnknownKeys(obj, allowed); err != nil {
		return err
	}
	if raw, ok := obj["capacity"]; ok {
		if operation != "encode" {
			return fmt.Errorf("capacity is permitted only on encode")
		}
		if err := validateCapacityArgument(raw); err != nil {
			return err
		}
	}
	return nil
}

func validateCapacityArgument(raw json.RawMessage) error {
	var number json.Number
	if err := json.Unmarshal(raw, &number); err != nil {
		return fmt.Errorf("capacity must be an unsigned integer")
	}
	value, err := strconv.ParseUint(number.String(), 10, 64)
	if err != nil {
		return fmt.Errorf("capacity must be an unsigned integer")
	}
	if value > uint64(math.MaxUint32) {
		return fmt.Errorf("capacity exceeds u32 range")
	}
	return nil
}

func rejectUnknownKeys(obj map[string]json.RawMessage, allowed map[string]bool) error {
	for key := range obj {
		if !allowed[key] {
			return fmt.Errorf("unknown argument key %q", key)
		}
	}
	return nil
}

func requireSignedInt32Field(obj map[string]json.RawMessage, key string) error {
	raw, ok := obj[key]
	if !ok {
		return fmt.Errorf("missing %s", key)
	}
	var number json.Number
	if err := json.Unmarshal(raw, &number); err != nil {
		return fmt.Errorf("%s must be a signed 32-bit integer", key)
	}
	value, err := number.Int64()
	if err != nil || value < math.MinInt32 || value > math.MaxInt32 {
		return fmt.Errorf("%s must be a signed 32-bit integer", key)
	}
	return nil
}
