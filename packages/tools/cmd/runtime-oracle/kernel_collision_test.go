package main

import (
	"encoding/binary"
	"math"
	"os"
	"path/filepath"
	"testing"

	"github.com/channing771/mornlea/packages/shared/nativeabi"
)

func TestKernelCollision(t *testing.T) {
	// 1. Valid case
	input := make([]byte, 64+196*27) // 3x3x3 = 27 cells
	copy(input[0:4], "MGC1")
	binary.LittleEndian.PutUint32(input[4:8], 1)
	binary.LittleEndian.PutUint32(input[8:12], math.Float32bits(1.5))
	binary.LittleEndian.PutUint32(input[12:16], math.Float32bits(0.5))
	binary.LittleEndian.PutUint32(input[16:20], math.Float32bits(1.5))
	binary.LittleEndian.PutUint32(input[20:24], math.Float32bits(-0.5))
	input[32] = 1
	binary.LittleEndian.PutUint32(input[36:40], math.Float32bits(0.6))
	binary.LittleEndian.PutUint32(input[40:44], 0)
	binary.LittleEndian.PutUint32(input[44:48], 0)
	binary.LittleEndian.PutUint32(input[48:52], 0)
	binary.LittleEndian.PutUint32(input[52:56], 3)
	binary.LittleEndian.PutUint32(input[56:60], 3)
	binary.LittleEndian.PutUint32(input[60:64], 3)
	output := make([]byte, 16)
	nativeabi.CollisionResolve(input, output)

	// 2. Short input -> panic
	func() {
		defer func() { recover() }()
		nativeabi.CollisionResolve(make([]byte, 63), output)
		t.Errorf("expected panic on short input")
	}()

	// 3. Short output -> panic
	func() {
		defer func() { recover() }()
		nativeabi.CollisionResolve(input, make([]byte, 15))
		t.Errorf("expected panic on short output")
	}()

	// 4. Invalid length -> panic
	func() {
		defer func() { recover() }()
		nativeabi.CollisionResolve(make([]byte, 65), output) // 65 is not 64 + N*196
		t.Errorf("expected panic on invalid length")
	}()

	// 5. Invalid magic -> panic
	func() {
		defer func() { recover() }()
		badMagic := make([]byte, len(input))
		copy(badMagic, input)
		copy(badMagic[0:4], "BAD0")
		nativeabi.CollisionResolve(badMagic, output)
		t.Errorf("expected panic on bad magic")
	}()

	exportDir := os.Getenv("RUNTIME_ORACLE_EXPORT_DIR")
	if exportDir != "" {
		if err := os.MkdirAll(exportDir, 0755); err != nil {
			t.Fatalf("MkdirAll: %v", err)
		}
		if err := os.WriteFile(filepath.Join(exportDir, "collision_input.bin"), input, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
		if err := os.WriteFile(filepath.Join(exportDir, "collision_output.bin"), output, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
	}
}
