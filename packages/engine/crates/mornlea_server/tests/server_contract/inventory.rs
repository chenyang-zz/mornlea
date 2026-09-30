//! The capability inventory is a source fixture. This module checks that its
//! rows are present, unique, and fully named. It does not execute those rows.

use std::collections::BTreeSet;

#[test]
fn capability_inventory_rows_are_unique_and_complete() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../testdata/runtime-migration/server/capability-inventory.json"
    );
    let text = std::fs::read_to_string(path).expect("capability inventory");
    let rows = rows_array(&text);
    let mut ids = Vec::new();
    let mut rest = rows;
    while let Some(index) = rest.find("\"id\": \"") {
        let tail = &rest[index + "\"id\": \"".len()..];
        let end = tail.find('"').expect("id end");
        let id = &tail[..end];
        assert!(!id.is_empty(), "empty capability id");
        ids.push(id.to_owned());
        rest = &tail[end..];
    }
    assert_eq!(ids.len(), 78);
    let mut seen = BTreeSet::new();
    for id in &ids {
        assert!(seen.insert(id.clone()), "duplicate {id}");
    }
    for field in [
        "source",
        "source_sha256",
        "fixture",
        "rust_node",
        "rust_test",
    ] {
        let needle = format!("\"{field}\":");
        let count = rows.matches(&needle).count();
        assert_eq!(count, 78, "{field}");
    }
}

fn rows_array(text: &str) -> &str {
    let rows_at = text.find("\"rows\"").expect("rows");
    let start = text[rows_at..].find('[').expect("rows array") + rows_at;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return &text[start..=start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unclosed rows array");
}
