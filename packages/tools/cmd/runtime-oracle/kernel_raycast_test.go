package main

import (
	"testing"

	"github.com/channing771/mornlea/packages/shared/nativeabi"
)

func TestKernelRaycast(t *testing.T) {
	input := make([]byte, 40)
	copy(input[0:4], "MGR1")
	input[4] = 1

	// origin = [0, 0, 0]
	// direction = [1, 0, 0]
	input[20] = 0
	input[21] = 0
	input[22] = 0x80
	input[23] = 0x3f // 1.0f

	// max = 10.0
	input[32] = 0
	input[33] = 0
	input[34] = 0x20
	input[35] = 0x41 // 10.0f

	cursor := make([]byte, 64)
	copy(cursor[0:4], "MRC1")
	cursor[4] = 1

	output := make([]byte, 1280)

	count, done := nativeabi.RaycastBatch(input, cursor, output)

	if count != 11 {
		t.Errorf("expected 11 records, got %d", count)
	}

	if !done {
		t.Errorf("expected done to be true")
	}

	// test error conditions (panics)
	func() {
		defer func() {
			if r := recover(); r == nil {
				t.Errorf("expected panic on short input")
			}
		}()
		nativeabi.RaycastBatch(make([]byte, 39), cursor, output)
	}()

	func() {
		defer func() {
			if r := recover(); r == nil {
				t.Errorf("expected panic on short output")
			}
		}()
		nativeabi.RaycastBatch(input, cursor, make([]byte, 1279))
	}()
}
