//! JSON document model with Go `encoding/json` lookup semantics.
//!
//! `serde_json::Value` collapses duplicate object keys and erases whether a
//! number was written as an integer literal, but Go's config decoder observes
//! both: struct targets (`logging`) see every duplicate key, `int` targets
//! reject `1.0` and `1e0`, and maps keep the last duplicate. This model keeps
//! every entry in source order plus a last-wins index, and records the number
//! form serde_json reported.
//!
//! Parsing still goes through serde_json, so its stricter input rules (invalid
//! UTF-8, lone surrogate escapes, 128 or more nested containers, float
//! literals that overflow `f64`, `-0` as an integer) reject files Go accepts. Those are documented known
//! differences, not parity bugs.

use std::collections::HashMap;
use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// A number as serde_json reported it. Integer literals that fit `u64`/`i64`
/// arrive as integers; every other literal (fraction, exponent, or an integer
/// too large for 64 bits) arrives as `F64`, which Go's `strconv.ParseInt` also
/// refuses. The one integer literal serde_json reports as a float is `-0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum JsonNumber {
    U64(u64),
    I64(i64),
    F64(f64),
}

impl JsonNumber {
    /// Go `json.Unmarshal` into `int`: integer literal within `int64`.
    pub(crate) fn as_go_int(self) -> Option<i64> {
        match self {
            Self::U64(value) => i64::try_from(value).ok(),
            Self::I64(value) => Some(value),
            Self::F64(_) => None,
        }
    }

    /// Go `json.Unmarshal` into `float64`. serde_json already refused literals
    /// outside the `f64` range, so every reported number converts.
    pub(crate) fn as_go_f64(self) -> f64 {
        match self {
            Self::U64(value) => value as f64,
            Self::I64(value) => value as f64,
            Self::F64(value) => value,
        }
    }

    /// Go `json.Unmarshal` into `float32` (`strconv.ParseFloat(s, 32)`):
    /// values that round to infinity are refused. Fractional literals are
    /// rounded through `f64` first; see the design's known differences.
    pub(crate) fn as_go_f32(self) -> Option<f32> {
        let value = match self {
            Self::U64(value) => value as f32,
            Self::I64(value) => value as f32,
            Self::F64(value) => value as f32,
        };
        value.is_finite().then_some(value)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum JsonValue {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(JsonObject),
}

impl JsonValue {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::Array(_) => "array",
            Self::Object(_) => "object",
        }
    }
}

/// Object entries in source order, duplicates included.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct JsonObject {
    entries: Vec<(String, JsonValue)>,
    last: HashMap<String, usize>,
}

impl JsonObject {
    fn push(&mut self, key: String, value: JsonValue) {
        self.last.insert(key.clone(), self.entries.len());
        self.entries.push((key, value));
    }

    /// Every entry including duplicates, as Go struct decoding sees them.
    pub(crate) fn all_entries(&self) -> impl Iterator<Item = (&str, &JsonValue)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }

    /// Distinct keys with their last value, as a Go `map` holds them.
    pub(crate) fn map_entries(&self) -> impl Iterator<Item = (&str, &JsonValue)> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(index, (key, _))| self.last.get(key) == Some(index))
            .map(|(_, (key, value))| (key.as_str(), value))
    }

    /// Go map index `m[key]`: exact key, last duplicate wins.
    pub(crate) fn get_exact(&self, key: &str) -> Option<&JsonValue> {
        self.last.get(key).map(|&index| &self.entries[index].1)
    }
}

/// Go `strings.EqualFold` restricted to an ASCII `key`. Simple case folding
/// maps only two non-ASCII runes onto ASCII letters: KELVIN SIGN onto `k` and
/// LATIN SMALL LETTER LONG S onto `s`.
pub(crate) fn go_equal_fold(candidate: &str, key: &str) -> bool {
    let mut chars = candidate.chars();
    for expected in key.chars() {
        let Some(actual) = chars.next() else {
            return false;
        };
        let matches = actual.eq_ignore_ascii_case(&expected)
            || (actual == '\u{212A}' && expected.eq_ignore_ascii_case(&'k'))
            || (actual == '\u{17F}' && expected.eq_ignore_ascii_case(&'s'));
        if !matches {
            return false;
        }
    }
    chars.next().is_none()
}

/// Go `strings.ToLower`: per-rune simple lowercase mapping. Only U+0130 has a
/// multi-rune full mapping, and its simple mapping is `i`.
pub(crate) fn go_to_lower(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == '\u{130}' {
                'i'
            } else {
                c.to_lowercase().next().unwrap_or(c)
            }
        })
        .collect()
}

pub(crate) fn parse(bytes: &[u8]) -> Result<JsonValue, serde_json::Error> {
    serde_json::from_slice(bytes)
}

impl<'de> Deserialize<'de> for JsonValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonValueVisitor)
    }
}

struct JsonValueVisitor;

impl<'de> Visitor<'de> for JsonValueVisitor {
    type Value = JsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<JsonValue, E> {
        Ok(JsonValue::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<JsonValue, E> {
        Ok(JsonValue::Number(JsonNumber::I64(value)))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<JsonValue, E> {
        Ok(JsonValue::Number(JsonNumber::U64(value)))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<JsonValue, E> {
        Ok(JsonValue::Number(JsonNumber::F64(value)))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<JsonValue, E> {
        Ok(JsonValue::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<JsonValue, E> {
        Ok(JsonValue::String(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<JsonValue, E> {
        Ok(JsonValue::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<JsonValue, E> {
        Ok(JsonValue::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<JsonValue, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(JsonValue::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<JsonValue, A::Error> {
        let mut object = JsonObject::default();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value()?;
            object.push(key, value);
        }
        Ok(JsonValue::Object(object))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_are_kept_in_order_and_map_view_is_last_wins() {
        let JsonValue::Object(object) = parse(br#"{"a":1,"b":2,"a":3}"#).unwrap() else {
            panic!("object");
        };
        assert_eq!(object.all_entries().count(), 3);
        let map: Vec<_> = object.map_entries().map(|(key, _)| key).collect();
        assert_eq!(map, ["b", "a"]);
        assert_eq!(
            object.get_exact("a"),
            Some(&JsonValue::Number(JsonNumber::U64(3)))
        );
    }

    #[test]
    fn number_forms_follow_go_int_rules() {
        let int = |text: &str| match parse(text.as_bytes()).unwrap() {
            JsonValue::Number(number) => number.as_go_int(),
            other => panic!("{other:?}"),
        };
        assert_eq!(int("1"), Some(1));
        // serde_json reports `-0` as a float, so Rust refuses an integer
        // literal Go accepts (documented known difference).
        assert_eq!(int("-0"), None);
        assert_eq!(int("1.0"), None);
        assert_eq!(int("1e0"), None);
        assert_eq!(int("9223372036854775807"), Some(i64::MAX));
        assert_eq!(int("9223372036854775808"), None);
        assert_eq!(int("-9223372036854775809"), None);
    }

    #[test]
    fn float32_conversion_rounds_and_refuses_overflow() {
        let f32_of = |text: &str| match parse(text.as_bytes()).unwrap() {
            JsonValue::Number(number) => number.as_go_f32(),
            other => panic!("{other:?}"),
        };
        assert_eq!(f32_of("1.00000001"), Some(1.0));
        assert!(f32_of("1.0000001").unwrap() > 1.0);
        assert_eq!(f32_of("1e39"), None);
    }

    #[test]
    fn equal_fold_matches_go_special_runes() {
        assert!(go_equal_fold("SpawnRadius", "spawnRadius"));
        assert!(go_equal_fold("\u{17F}im", "sim"));
        assert!(go_equal_fold("\u{212A}EY", "key"));
        assert!(!go_equal_fold("sims", "sim"));
        assert!(!go_equal_fold("si", "sim"));
        assert_eq!(go_to_lower("\u{130}D"), "id");
        assert_eq!(go_to_lower("\u{212A}"), "k");
    }
}
