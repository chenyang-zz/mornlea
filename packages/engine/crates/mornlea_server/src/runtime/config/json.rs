//! JSON document model with Go `encoding/json` lookup semantics.
//!
//! `serde_json::Value` collapses duplicate object keys and loses each number's
//! source literal, but Go's config decoder observes both: struct targets
//! (`logging`) see every duplicate key, maps keep the last duplicate, and Go
//! parses each number literal straight into its target type (`int` accepts
//! `-0` and refuses `-0.0`, `float32` rounds the literal once). This model
//! keeps every entry in source order plus a last-wins index, and keeps every
//! number's exact source literal.
//!
//! Parsing still goes through serde_json, so its stricter input rules (invalid
//! UTF-8, lone surrogate escapes, 128 or more nested containers, float
//! literals that overflow `f64`) reject files Go accepts. Those are documented
//! known differences, not parity bugs. serde_json's number values are not
//! used: after it accepts a document, `parse` scans the bytes once for number
//! literals and pairs them with the parsed numbers in source order.

use std::collections::HashMap;
use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// A JSON number kept as its exact source literal, converted the way Go's
/// `encoding/json` converts it for each target type.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JsonNumber {
    literal: Box<str>,
}

impl JsonNumber {
    /// Go `json.Unmarshal` into `int`: `strconv.ParseInt(literal, 10, 64)`.
    /// Accepts `-0` as 0; refuses any fraction or exponent and `int64`
    /// overflow.
    pub(crate) fn as_go_int(&self) -> Option<i64> {
        self.literal.parse().ok()
    }

    /// Go `json.Unmarshal` into `float64`: `strconv.ParseFloat(literal, 64)`,
    /// correctly rounded once; values that round to infinity are refused.
    pub(crate) fn as_go_f64(&self) -> Option<f64> {
        self.literal
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
    }

    /// Go `json.Unmarshal` into `float32`: `strconv.ParseFloat(literal, 32)`,
    /// rounded once from the literal, never through `f64`; values that round
    /// to infinity are refused.
    pub(crate) fn as_go_f32(&self) -> Option<f32> {
        self.literal
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
    }

    fn pending() -> Self {
        Self {
            literal: Box::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn literal(&self) -> &str {
        &self.literal
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
    let mut value: JsonValue = serde_json::from_slice(bytes)?;
    let mut literals = number_literals(bytes).into_iter();
    let aligned = attach_literals(&mut value, &mut literals) && literals.next().is_none();
    if !aligned {
        return Err(de::Error::custom(
            "number literals do not line up with the parsed document",
        ));
    }
    Ok(value)
}

/// Every number literal in source order. Only called on bytes serde_json
/// accepted, so outside strings a number starts with `-` or a digit and runs
/// until the first byte that cannot continue a JSON number.
fn number_literals(bytes: &[u8]) -> Vec<Box<str>> {
    let mut literals = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'"' {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
                index += 1;
            }
            b'-' | b'0'..=b'9' => {
                let start = index;
                while index < bytes.len()
                    && matches!(bytes[index], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                {
                    index += 1;
                }
                // JSON number bytes are ASCII.
                literals.push(String::from_utf8_lossy(&bytes[start..index]).into());
            }
            _ => index += 1,
        }
    }
    literals
}

/// Pairs numbers with literals in source order: arrays in order, object
/// entries in source order with every duplicate. False if literals run out.
fn attach_literals(value: &mut JsonValue, literals: &mut impl Iterator<Item = Box<str>>) -> bool {
    match value {
        JsonValue::Number(number) => match literals.next() {
            Some(literal) => {
                number.literal = literal;
                true
            }
            None => false,
        },
        JsonValue::Array(items) => items.iter_mut().all(|item| attach_literals(item, literals)),
        JsonValue::Object(object) => object
            .entries
            .iter_mut()
            .all(|(_, item)| attach_literals(item, literals)),
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::String(_) => true,
    }
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

    // The literal is attached by `parse` after serde_json accepts the file.
    fn visit_i64<E: de::Error>(self, _value: i64) -> Result<JsonValue, E> {
        Ok(JsonValue::Number(JsonNumber::pending()))
    }

    fn visit_u64<E: de::Error>(self, _value: u64) -> Result<JsonValue, E> {
        Ok(JsonValue::Number(JsonNumber::pending()))
    }

    fn visit_f64<E: de::Error>(self, _value: f64) -> Result<JsonValue, E> {
        Ok(JsonValue::Number(JsonNumber::pending()))
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
        let Some(JsonValue::Number(number)) = object.get_exact("a") else {
            panic!("number");
        };
        assert_eq!(number.literal(), "3");
    }

    #[test]
    fn number_forms_follow_go_int_rules() {
        let int = |text: &str| match parse(text.as_bytes()).unwrap() {
            JsonValue::Number(number) => number.as_go_int(),
            other => panic!("{other:?}"),
        };
        assert_eq!(int("1"), Some(1));
        // Go `strconv.ParseInt` accepts `-0` as 0 but refuses every zero
        // written with a fraction or exponent.
        assert_eq!(int("-0"), Some(0));
        assert_eq!(int("0"), Some(0));
        for text in ["-0.0", "-0e0", "-0E0", "-0.0e0", "-0e-0", "0.0", "0e0"] {
            assert_eq!(int(text), None, "{text}");
        }
        assert_eq!(int("1.0"), None);
        assert_eq!(int("1e0"), None);
        assert_eq!(int("9223372036854775807"), Some(i64::MAX));
        assert_eq!(int("9223372036854775808"), None);
        assert_eq!(int("-9223372036854775809"), None);
    }

    #[test]
    fn negative_zero_literal_is_tracked_in_document_order() {
        let JsonValue::Object(object) =
            parse(br#"{"s":"-0","a":[-0.0,-0,{"-0":-0e0}],"b":-0,"b":-0E0,"c":-0 }"#).unwrap()
        else {
            panic!("object");
        };
        let ints: Vec<Option<i64>> = numbers(&JsonValue::Object(object))
            .into_iter()
            .map(|number| number.as_go_int())
            .collect();
        assert_eq!(
            ints,
            [None, Some(0), None, Some(0), None, Some(0)],
            "only the bare `-0` literals are integers"
        );
    }

    #[test]
    fn literal_scan_skips_strings_with_escapes() {
        let value = parse(br#"{"k\"-1":"2\\","e":"\u0031","n":[-0, 7e1]}"#).unwrap();
        let literals: Vec<String> = numbers(&value)
            .iter()
            .map(|number| number.literal().to_owned())
            .collect();
        assert_eq!(literals, ["-0", "7e1"]);
    }

    #[test]
    fn literals_stay_aligned_across_duplicate_keys() {
        let JsonValue::Object(object) =
            parse(br#"{"a":-0,"logging":{"default":"info","default":"warn"},"a":-0.0,"b":[1,-0]}"#)
                .unwrap()
        else {
            panic!("object");
        };
        let int_of = |value: Option<&JsonValue>| match value {
            Some(JsonValue::Number(number)) => number.as_go_int(),
            other => panic!("{other:?}"),
        };
        // Map view: the last duplicate wins and keeps its own literal.
        assert_eq!(int_of(object.get_exact("a")), None);
        // Struct view: every duplicate keeps its own literal, in order.
        let all: Vec<Option<i64>> = object
            .all_entries()
            .filter(|(key, _)| *key == "a")
            .map(|(_, value)| int_of(Some(value)))
            .collect();
        assert_eq!(all, [Some(0), None]);
        let Some(JsonValue::Array(items)) = object.get_exact("b") else {
            panic!("array");
        };
        assert_eq!(int_of(items.get(1)), Some(0));
    }

    #[test]
    fn negative_zero_literal_keeps_go_float_sign() {
        let number = |text: &str| match parse(text.as_bytes()).unwrap() {
            JsonValue::Number(number) => number,
            other => panic!("{other:?}"),
        };
        // Go `strconv.ParseFloat("-0")` is negative zero.
        let f = number("-0").as_go_f64().unwrap();
        assert!(f == 0.0 && f.is_sign_negative());
        let f = number("-0").as_go_f32().unwrap();
        assert!(f == 0.0 && f.is_sign_negative());
        assert!(number("0").as_go_f64().unwrap().is_sign_positive());
    }

    fn numbers(value: &JsonValue) -> Vec<JsonNumber> {
        match value {
            JsonValue::Number(number) => vec![number.clone()],
            JsonValue::Array(items) => items.iter().flat_map(numbers).collect(),
            JsonValue::Object(object) => object
                .all_entries()
                .flat_map(|(_, value)| numbers(value))
                .collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn float32_conversion_rounds_and_refuses_overflow() {
        let f32_of = |text: &str| match parse(text.as_bytes()).unwrap() {
            JsonValue::Number(number) => number.as_go_f32(),
            other => panic!("{other:?}"),
        };
        assert_eq!(f32_of("1.00000001"), Some(1.0));
        // Go `strconv.ParseFloat(s, 32)` rounds the literal once. Rounding to
        // `f64` first lands exactly on the float32 midpoint and then ties to
        // 1.0, which would accept an `audioVolume` Go refuses.
        assert!(f32_of("1.00000005960464477539062500000000001").unwrap() > 1.0);
        assert_eq!(f32_of("1.000000059604644775390625"), Some(1.0));
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
