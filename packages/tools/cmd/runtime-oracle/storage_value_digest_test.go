package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"fmt"
	"math"
	"sort"
	"testing"
)

const (
	storageTagNull     = 0x00
	storageTagFalse    = 0x01
	storageTagTrue     = 0x02
	storageTagSigned   = 0x03
	storageTagUnsigned = 0x04
	storageTagF32      = 0x05
	storageTagUTF8     = 0x06
	storageTagBytes    = 0x07
	storageTagArray    = 0x08
	storageTagObject   = 0x09
)

type storageValueNode struct {
	null     bool
	boolean  *bool
	signed   *int64
	unsigned *uint64
	f32bits  *uint32
	utf8     *string
	bytes    []byte
	array    []storageValueNode
	object   map[string]storageValueNode
}

func storageValueNull() storageValueNode {
	return storageValueNode{null: true}
}

func storageValueBool(v bool) storageValueNode {
	return storageValueNode{boolean: &v}
}

func storageValueSigned(v int64) storageValueNode {
	return storageValueNode{signed: &v}
}

func storageValueUnsigned(v uint64) storageValueNode {
	return storageValueNode{unsigned: &v}
}

func storageValueF32(v float32) storageValueNode {
	bits := math.Float32bits(v)
	return storageValueNode{f32bits: &bits}
}

func storageValueUTF8(v string) storageValueNode {
	return storageValueNode{utf8: &v}
}

func storageValueBytes(v []byte) storageValueNode {
	return storageValueNode{bytes: v}
}

func storageValueArray(items []storageValueNode) storageValueNode {
	return storageValueNode{array: items}
}

func storageValueObject(fields map[string]storageValueNode) storageValueNode {
	return storageValueNode{object: fields}
}

func encodeStorageValueV1(node storageValueNode) []byte {
	var out bytes.Buffer
	encodeStorageValueNode(&out, node)
	return out.Bytes()
}

func encodeStorageValueNode(out *bytes.Buffer, node storageValueNode) {
	switch {
	case node.null:
		out.WriteByte(storageTagNull)
	case node.boolean != nil:
		if *node.boolean {
			out.WriteByte(storageTagTrue)
		} else {
			out.WriteByte(storageTagFalse)
		}
	case node.signed != nil:
		out.WriteByte(storageTagSigned)
		_ = binary.Write(out, binary.LittleEndian, *node.signed)
	case node.unsigned != nil:
		out.WriteByte(storageTagUnsigned)
		_ = binary.Write(out, binary.LittleEndian, *node.unsigned)
	case node.f32bits != nil:
		out.WriteByte(storageTagF32)
		_ = binary.Write(out, binary.LittleEndian, *node.f32bits)
	case node.utf8 != nil:
		out.WriteByte(storageTagUTF8)
		text := []byte(*node.utf8)
		_ = binary.Write(out, binary.LittleEndian, uint32(len(text)))
		out.Write(text)
	case node.bytes != nil:
		out.WriteByte(storageTagBytes)
		_ = binary.Write(out, binary.LittleEndian, uint32(len(node.bytes)))
		out.Write(node.bytes)
	case node.array != nil:
		out.WriteByte(storageTagArray)
		_ = binary.Write(out, binary.LittleEndian, uint32(len(node.array)))
		for _, item := range node.array {
			encodeStorageValueNode(out, item)
		}
	case node.object != nil:
		out.WriteByte(storageTagObject)
		keys := make([]string, 0, len(node.object))
		for key := range node.object {
			keys = append(keys, key)
		}
		sort.Strings(keys)
		_ = binary.Write(out, binary.LittleEndian, uint32(len(keys)))
		for _, key := range keys {
			keyBytes := []byte(key)
			_ = binary.Write(out, binary.LittleEndian, uint32(len(keyBytes)))
			out.Write(keyBytes)
			encodeStorageValueNode(out, node.object[key])
		}
	default:
		panic("empty storage value node")
	}
}

func storageValueSHA256(node storageValueNode) string {
	sum := sha256.Sum256(encodeStorageValueV1(node))
	return fmt.Sprintf("sha256:%x", sum)
}

func crossLanguageGoldenStorageTree() storageValueNode {
	return storageValueArray([]storageValueNode{
		storageValueNull(),
		storageValueSigned(-1001),
		storageValueUnsigned(1001),
		storageValueF32(math.Float32frombits(0x80000000)),
		storageValueUTF8("storage"),
		storageValueBytes([]byte{0x0a, 0x0b, 0x0c}),
		storageValueObject(map[string]storageValueNode{
			"nested_key": storageValueSigned(-7),
			"wide":       storageValueUnsigned(9),
		}),
		storageValueArray([]storageValueNode{
			storageValueSigned(1),
			storageValueSigned(2),
			storageValueSigned(3),
		}),
	})
}

// Cross-language golden tree pinned by the frozen storage corpus contract.
const crossLanguageGoldenStorageTreeSHA256 =
	"sha256:9afe6b3bc14a7b3daa74b6be41bc34d2357ac15a874c92a20a8f84c3bf66206b"

func TestStorageValueV1CrossLanguageGoldenTree(t *testing.T) {
	digest := storageValueSHA256(crossLanguageGoldenStorageTree())
	if digest != crossLanguageGoldenStorageTreeSHA256 {
		t.Fatalf("golden tree digest mismatch: got %s want %s", digest, crossLanguageGoldenStorageTreeSHA256)
	}
}
