//! Cross-language `StorageValueV1` tree encoding used for corpus `value_sha256`.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const TAG_NULL: u8 = 0x00;
const TAG_FALSE: u8 = 0x01;
const TAG_TRUE: u8 = 0x02;
const TAG_SIGNED: u8 = 0x03;
const TAG_UNSIGNED: u8 = 0x04;
const TAG_F32: u8 = 0x05;
const TAG_UTF8: u8 = 0x06;
const TAG_BYTES: u8 = 0x07;
const TAG_ARRAY: u8 = 0x08;
const TAG_OBJECT: u8 = 0x09;

/// One node in the typed value tree hashed by corpus evidence.
#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)]
pub enum Value {
    Null,
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    F32(f32),
    Utf8(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

/// SHA-256 digest of the tagged `StorageValueV1` encoding for `value`.
pub fn value_sha256(value: &Value) -> String {
    let encoded = encode_value_v1(value);
    let mut hasher = Sha256::new();
    hasher.update(&encoded);
    format!("sha256:{:x}", hasher.finalize())
}

/// Encodes one value using the frozen `StorageValueV1` tag layout.
pub fn encode_value_v1(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(value, &mut out);
    out
}

fn encode_into(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.push(TAG_NULL),
        Value::Bool(false) => out.push(TAG_FALSE),
        Value::Bool(true) => out.push(TAG_TRUE),
        Value::Signed(v) => {
            out.push(TAG_SIGNED);
            out.extend_from_slice(&v.to_le_bytes());
        }
        Value::Unsigned(v) => {
            out.push(TAG_UNSIGNED);
            out.extend_from_slice(&v.to_le_bytes());
        }
        Value::F32(v) => {
            out.push(TAG_F32);
            out.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        Value::Utf8(text) => {
            out.push(TAG_UTF8);
            push_u32_le(out, text.len() as u32);
            out.extend_from_slice(text.as_bytes());
        }
        Value::Bytes(bytes) => {
            out.push(TAG_BYTES);
            push_u32_le(out, bytes.len() as u32);
            out.extend_from_slice(bytes);
        }
        Value::Array(items) => {
            out.push(TAG_ARRAY);
            push_u32_le(out, items.len() as u32);
            for item in items {
                encode_into(item, out);
            }
        }
        Value::Object(fields) => {
            out.push(TAG_OBJECT);
            push_u32_le(out, fields.len() as u32);
            for (key, child) in fields {
                push_u32_le(out, key.len() as u32);
                out.extend_from_slice(key.as_bytes());
                encode_into(child, out);
            }
        }
    }
}

fn push_u32_le(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Independent cross-language golden tree from the frozen corpus contract.
pub fn cross_language_golden_tree() -> Value {
    let mut nested = BTreeMap::new();
    nested.insert("nested_key".to_string(), Value::Signed(-7));
    nested.insert("wide".to_string(), Value::Unsigned(9));

    Value::Array(vec![
        Value::Null,
        Value::Signed(-1_001),
        Value::Unsigned(1_001),
        Value::F32(f32::from_bits(0x8000_0000)),
        Value::Utf8("storage".to_string()),
        Value::Bytes(vec![0x0a, 0x0b, 0x0c]),
        Value::Object(nested),
        Value::Array(vec![Value::Signed(1), Value::Signed(2), Value::Signed(3)]),
    ])
}

/// Digest produced independently by the Go `TestStorageValueV1CrossLanguageGoldenTree`
/// oracle in `storage_manifest_test.go`.
pub const CROSS_LANGUAGE_GOLDEN_TREE_SHA256: &str =
    "sha256:9afe6b3bc14a7b3daa74b6be41bc34d2357ac15a874c92a20a8f84c3bf66206b";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_value_v1_object_fields_are_sorted_on_the_wire() {
        let mut fields = BTreeMap::new();
        fields.insert("z_last".to_string(), Value::Signed(1));
        fields.insert("a_first".to_string(), Value::Signed(2));
        let encoded = encode_value_v1(&Value::Object(fields));
        let z_pos = encoded
            .windows(6)
            .position(|window| window == b"z_last")
            .expect("z_last key bytes");
        let a_pos = encoded
            .windows(7)
            .position(|window| window == b"a_first")
            .expect("a_first key bytes");
        assert!(
            a_pos < z_pos,
            "object keys must encode in ascending UTF-8 order"
        );
    }

    #[test]
    fn storage_value_v1_cross_language_golden_tree_digest_matches_go() {
        let digest = value_sha256(&cross_language_golden_tree());
        assert_eq!(
            digest, CROSS_LANGUAGE_GOLDEN_TREE_SHA256,
            "golden tree digest must match the Go node 1.1 oracle"
        );
    }
}
