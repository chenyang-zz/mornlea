package main

import (
	"bytes"
	"encoding/binary"
	"math"
	"testing"

	"github.com/channing771/mornlea/packages/shared/nativeabi"
)

func TestKernelPhysics(t *testing.T) {
	validInput := make([]byte, 196160)
	copy(validInput[0:4], "MGP1")
	binary.LittleEndian.PutUint32(validInput[4:8], 4)
	
	binary.LittleEndian.PutUint32(validInput[8:12], math.Float32bits(0.5))
	binary.LittleEndian.PutUint32(validInput[12:16], math.Float32bits(1.0))
	binary.LittleEndian.PutUint32(validInput[16:20], math.Float32bits(0.5))
	validInput[32] = 0
	validInput[33] = 0
	validInput[34] = 0
	validInput[35] = 0
	binary.LittleEndian.PutUint32(validInput[40:44], math.Float32bits(1.0))
	binary.LittleEndian.PutUint32(validInput[44:48], math.Float32bits(0.05))

	binary.LittleEndian.PutUint32(validInput[80:84], math.Float32bits(-1.0))
	binary.LittleEndian.PutUint32(validInput[84:88], math.Float32bits(1.0))
	binary.LittleEndian.PutUint32(validInput[88:92], math.Float32bits(-1.0))
	binary.LittleEndian.PutUint32(validInput[92:96], math.Float32bits(1.0))
	binary.LittleEndian.PutUint32(validInput[96:100], math.Float32bits(-1.0))
	binary.LittleEndian.PutUint32(validInput[100:104], math.Float32bits(1.0))

	binary.LittleEndian.PutUint32(validInput[104:108], 0xFFFFFFFB)
	binary.LittleEndian.PutUint32(validInput[108:112], 0xFFFFFFFB)
	binary.LittleEndian.PutUint32(validInput[112:116], 0xFFFFFFFB)
	binary.LittleEndian.PutUint32(validInput[116:120], 10)
	binary.LittleEndian.PutUint32(validInput[120:124], 10)
	binary.LittleEndian.PutUint32(validInput[124:128], 10)

	validInput[160 + 555*196] = 1
	validOutput := make([]byte, 32)

	assertPanic := func(t *testing.T, f func()) {
		defer func() {
			if r := recover(); r == nil {
				t.Fatalf("expected panic")
			}
		}()
		f()
	}

	t.Run("Valid Input", func(t *testing.T) {
		nativeabi.PhysicsStep(validInput, validOutput)
	})

	t.Run("Short Input", func(t *testing.T) {
		assertPanic(t, func() {
			nativeabi.PhysicsStep(validInput[:150], validOutput)
		})
	})

	t.Run("Short Output", func(t *testing.T) {
		assertPanic(t, func() {
			nativeabi.PhysicsStep(validInput, validOutput[:31])
		})
	})

	t.Run("Invalid Arguments", func(t *testing.T) {
		badInput := bytes.Clone(validInput)
		binary.LittleEndian.PutUint32(badInput[8:12], 0x7FC00000)
		assertPanic(t, func() {
			nativeabi.PhysicsStep(badInput, validOutput)
		})
	})

	t.Run("Displacement out of bounds", func(t *testing.T) {
		badInput := bytes.Clone(validInput)
		binary.LittleEndian.PutUint32(badInput[88:92], math.Float32bits(0.0))
		binary.LittleEndian.PutUint32(badInput[92:96], math.Float32bits(0.0))
		binary.LittleEndian.PutUint32(badInput[72:76], math.Float32bits(32.0))
		binary.LittleEndian.PutUint32(badInput[76:80], math.Float32bits(78.4))
		assertPanic(t, func() {
			nativeabi.PhysicsStep(badInput, validOutput)
		})
	})
}
