//! Small model-facing views over durable originals. No database, permissions,
//! execution, or changes to the raw audit result live in this module.
use crate::agent::tool::ToolResult;
use serde_json::{json, Value};

pub(crate) const MAX_VIEW_BYTES: usize = 5000;
pub(crate) trait ToolContextProjector: Send + Sync {
    /// None means the original must remain visible: no durable source, an
    /// ineligible result, or no useful reduction. Errors also retain the raw
    /// result and must be reported by the caller, never as a source success.
    fn project(
        &self,
        call_id: &str,
        tool_name: &str,
        result: &ToolResult,
    ) -> Result<Option<String>, String>;
}

pub(crate) fn view(
    source_ref: &str,
    tool_name: &str,
    reported_success: bool,
    output: &str,
    path: &str,
    offset: usize,
    limit: usize,
    raw: bool,
    tail: bool,
) -> Result<String, String> {
    if !(1..=2000).contains(&limit) || path.len() > 512 || (tail && offset != 0) {
        return Err("Invalid bounded source window".into());
    }
    let mut result = json!({"source_ref":source_ref,"tool_name":tool_name,"reported_success":reported_success,"historical_data":true,"path":path});
    if raw {
        if !path.is_empty() {
            return Err("Raw text windows require an empty path".into());
        }
        add_text_window(&mut result, output, "text", offset, limit, tail)?;
    } else if let Ok(value) = serde_json::from_str::<Value>(output) {
        let value = if path.is_empty() {
            &value
        } else {
            value
                .pointer(path)
                .ok_or("Tool source JSON path is unavailable")?
        };
        match value {
            Value::Object(map) => {
                if tail {
                    return Err("Tail windows require a text source or string path".into());
                }
                if offset > map.len() {
                    return Err("Source entry offset is outside the object".into());
                }
                result["kind"] = json!("object");
                result["unit"] = json!("entries");
                result["offset"] = json!(offset);
                result["total_entries"] = json!(map.len());
                result["values"] = json!({});
                result["omitted_paths"] = json!([]);
                let mut taken = 0;
                for (key, value) in map.iter().skip(offset).take(limit.min(16)) {
                    let pointer = format!("{}/{}", path, key.replace('~', "~0").replace('/', "~1"));
                    let previous = result.clone();
                    if pointer.len() > 512 {
                        result["unaddressable_entries"] = json!(true);
                    } else if value.to_string().len() <= 512 {
                        result["values"][key] = value.clone();
                    } else {
                        result["omitted_paths"]
                            .as_array_mut()
                            .unwrap()
                            .push(json!(pointer));
                    }
                    if result.to_string().len() > MAX_VIEW_BYTES - 150 {
                        result = previous;
                        break;
                    }
                    taken += 1;
                }
                if taken == 0 && offset < map.len() {
                    return Err("Source entry metadata exceeds view budget".into());
                }
                let next = offset + taken;
                result["has_more"] = json!(next < map.len());
                result["next_offset"] = json!(if next < map.len() { Some(next) } else { None });
                result["complete"] = json!(
                    offset == 0
                        && next == map.len()
                        && result["omitted_paths"].as_array().unwrap().is_empty()
                        && result.get("unaddressable_entries").is_none()
                );
            }
            Value::Array(items) => {
                if tail {
                    return Err("Tail windows require a text source or string path".into());
                }
                if offset > items.len() {
                    return Err("Source entry offset is outside the array".into());
                }
                result["kind"] = json!("array");
                result["unit"] = json!("entries");
                result["offset"] = json!(offset);
                result["total_entries"] = json!(items.len());
                result["values"] = json!({});
                result["omitted_paths"] = json!([]);
                let mut taken = 0;
                for (index, value) in items.iter().enumerate().skip(offset).take(limit.min(16)) {
                    let previous = result.clone();
                    let pointer = format!("{path}/{index}");
                    if value.to_string().len() <= 512 {
                        result["values"][index.to_string()] = value.clone();
                    } else if pointer.len() > 512 {
                        result["unaddressable_entries"] = json!(true);
                    } else {
                        result["omitted_paths"]
                            .as_array_mut()
                            .unwrap()
                            .push(json!(pointer));
                    }
                    if result.to_string().len() > MAX_VIEW_BYTES - 150 {
                        result = previous;
                        break;
                    }
                    taken += 1;
                }
                if taken == 0 && offset < items.len() {
                    return Err("Source entry metadata exceeds view budget".into());
                }
                let next = offset + taken;
                result["has_more"] = json!(next < items.len());
                result["next_offset"] = json!(if next < items.len() { Some(next) } else { None });
                result["complete"] = json!(
                    offset == 0
                        && next == items.len()
                        && result["omitted_paths"].as_array().unwrap().is_empty()
                        && result.get("unaddressable_entries").is_none()
                );
            }
            Value::String(text) => add_text_window(&mut result, text, "text", offset, limit, tail)?,
            scalar => {
                if tail {
                    return Err("Tail windows require a text source or string path".into());
                }
                if offset != 0 {
                    return Err("Scalar sources have no entry offset".into());
                }
                result["kind"] = json!("scalar");
                result["value"] = scalar.clone();
                result["complete"] = json!(true);
            }
        }
    } else {
        if !path.is_empty() {
            return Err("Text sources have no JSON paths".into());
        }
        add_text_window(&mut result, output, "text", offset, limit, tail)?;
    }
    let output = result.to_string();
    if output.len() > MAX_VIEW_BYTES {
        return Err("Tool source view exceeds byte budget".into());
    }
    Ok(output)
}

/// Shared Unicode window for source reads and direct file reads. Existing host
/// metadata counts toward the same encoded byte budget as the selected text.
pub(crate) fn add_text_window(
    result: &mut Value,
    text: &str,
    content_key: &str,
    offset: usize,
    limit: usize,
    tail: bool,
) -> Result<(), String> {
    if !(1..=2000).contains(&limit) || (tail && offset != 0) {
        return Err("Invalid bounded text window".into());
    }
    let total = text.chars().count();
    if offset > total {
        return Err("Source text offset is outside the original".into());
    }
    let offset = if tail {
        total.saturating_sub(limit)
    } else {
        offset
    };
    let chars: Vec<char> = text.chars().skip(offset).take(limit).collect();
    result["kind"] = json!("text");
    result["unit"] = json!("characters");
    result["offset"] = json!(offset);
    result["total_chars"] = json!(total);
    let make = |count: usize| {
        let mut value = result.clone();
        // Byte-budget trimming must preserve the END of a requested tail,
        // including escaped/non-ASCII text, not the beginning of that window.
        let offset = if tail { total - count } else { offset };
        let next = offset + count;
        value["offset"] = json!(offset);
        let selected = if tail {
            &chars[chars.len() - count..]
        } else {
            &chars[..count]
        };
        value[content_key] = json!(selected.iter().collect::<String>());
        value["omitted_before"] = json!(offset > 0);
        value["has_more"] = json!(next < total);
        value["next_offset"] = json!(if next < total { Some(next) } else { None });
        value["complete"] = json!(offset == 0 && next == total);
        value
    };
    let mut low = 0;
    let mut high = chars.len();
    while low < high {
        let mid = (low + high + 1) / 2;
        if make(mid).to_string().len() <= MAX_VIEW_BYTES {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    if low == 0 && offset < total {
        return Err("Source text metadata exceeds budget".into());
    }
    let selected = make(low);
    if selected.to_string().len() > MAX_VIEW_BYTES {
        return Err("Source text metadata exceeds budget".into());
    }
    *result = selected;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_views_keep_omissions_and_read_original_unicode_without_retrying() {
        let original =
            json!({"state":"failed","details":{"reason":"occupied","trace":"x".repeat(9000)}})
                .to_string();
        let root: Value = serde_json::from_str(
            &view("ref", "inspect", true, &original, "", 0, 1000, false, false).unwrap(),
        )
        .unwrap();
        assert_eq!(root["values"]["state"], "failed");
        assert_eq!(root["omitted_paths"], json!(["/details"]));
        assert_eq!(root["complete"], false);
        let leaf: Value = serde_json::from_str(
            &view(
                "ref", "inspect", true, &original, "/details", 0, 1000, false, false,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(leaf["values"]["reason"], "occupied");
        let text = "中🪶\n\"\\\u{0000}".repeat(1500);
        let mut read = String::new();
        let mut offset = 0;
        loop {
            let output =
                view("ref", "inspect", true, &text, "", offset, 2000, true, false).unwrap();
            assert!(output.len() <= MAX_VIEW_BYTES);
            let output: Value = serde_json::from_str(&output).unwrap();
            read.push_str(output["text"].as_str().unwrap());
            match output["next_offset"].as_u64() {
                None => break,
                Some(next) => {
                    assert!(next > offset as u64);
                    offset = next as usize;
                }
            }
        }
        assert_eq!(read, text);
        assert!(view("ref", "inspect", true, &original, "/absent", 0, 1, false, false).is_err());
        let key = "k".repeat(511);
        let nested = json!({key.clone():["v".repeat(900)]}).to_string();
        let page: Value = serde_json::from_str(
            &view(
                "ref",
                "inspect",
                true,
                &nested,
                &format!("/{key}"),
                0,
                16,
                false,
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page["unaddressable_entries"], true);
        assert_eq!(page["complete"], false);
        assert_eq!(
            page["omitted_paths"],
            json!([]),
            "do not advertise paths that cannot be requested; root text remains readable"
        );
        let mut oversized = json!({"resolvedPath":"p".repeat(MAX_VIEW_BYTES)});
        assert!(add_text_window(&mut oversized, "", "content", 0, 1200, false).is_err());
    }
}
