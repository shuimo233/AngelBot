//! Durable tool-result identity. This is journal metadata, not model authority,
//! a second receipt store, or a replacement for the original handler output.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::Value;
use sha2::{Digest, Sha256};

const SEAL: &str = "_angelbot_source";
pub(crate) const MAX_SOURCE_BYTES: usize = 1024 * 1024;

pub(crate) struct SourceRecord<'a> {
    pub id: &'a str,
    pub run_id: &'a str,
    pub session_id: &'a str,
    pub sequence: i64,
    pub event_type: &'a str,
    pub payload: &'a str,
    pub created_at: i64,
}

pub(crate) struct ToolSource {
    pub call_id: String,
    pub tool_name: String,
    pub reported_success: bool,
    pub output: String,
}

fn revision(record: &SourceRecord<'_>, payload: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(
        record.id,
        record.run_id,
        record.session_id,
        record.sequence,
        record.event_type,
        record.created_at,
        payload,
    ))
    .map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Add an internal seal only to eligible newly appended results. Legacy and
/// oversized results keep their original representation, without fake refs.
pub(crate) fn seal(record: &SourceRecord<'_>) -> Result<String, String> {
    if record.event_type != "ToolExecutionEnd" {
        return Ok(record.payload.into());
    }
    let mut payload: Value = serde_json::from_str(record.payload).map_err(|e| e.to_string())?;
    if payload["type"] != "ToolExecutionEnd" {
        return Err("Tool source event identity disagrees".into());
    }
    let Some(output) = payload["data"]["output"].as_str() else {
        return Err("Tool source has no output".into());
    };
    if output.len() > MAX_SOURCE_BYTES {
        return Ok(record.payload.into());
    }
    if payload.get(SEAL).is_some() {
        return Err("Tool source is already sealed".into());
    }
    let digest = revision(record, &payload)?;
    payload[SEAL] = serde_json::json!({"version":1,"sha256":digest});
    Ok(payload.to_string())
}

/// Verify the complete stored record before exposing its handler-returned
/// text. Private provider events and unhashed legacy results are ineligible.
pub(crate) fn open(record: &SourceRecord<'_>) -> Result<ToolSource, String> {
    if record.event_type != "ToolExecutionEnd" {
        return Err("Not a tool-result source".into());
    }
    if record.payload.len() > 8 * MAX_SOURCE_BYTES {
        return Err("Tool source exceeds read budget".into());
    }
    let mut payload: Value =
        serde_json::from_str(record.payload).map_err(|_| "Tool source is malformed")?;
    let metadata = payload
        .as_object_mut()
        .ok_or("Tool source is malformed")?
        .remove(SEAL)
        .ok_or("Tool source has no durable version binding")?;
    if metadata["version"] != 1
        || metadata["sha256"].as_str() != Some(revision(record, &payload)?.as_str())
    {
        return Err("Tool source version or scope changed".into());
    }
    let event: super::event::AgentEvent =
        serde_json::from_value(payload).map_err(|_| "Tool source event is malformed")?;
    let super::event::AgentEvent::ToolExecutionEnd {
        call_id,
        tool_name,
        success,
        output,
        ..
    } = event
    else {
        return Err("Not a tool-result source".into());
    };
    if call_id.is_empty() || tool_name.is_empty() || output.len() > MAX_SOURCE_BYTES {
        return Err("Tool source has invalid identity or size".into());
    }
    Ok(ToolSource {
        call_id,
        tool_name,
        reported_success: success,
        output,
    })
}

pub(crate) fn reference(id: &str) -> Result<String, String> {
    let id = uuid::Uuid::parse_str(id).map_err(|_| "Invalid tool source identity")?;
    Ok(format!("r-{}", URL_SAFE_NO_PAD.encode(id.as_bytes())))
}

pub(crate) fn record_id(reference: &str) -> Result<String, String> {
    if reference.len() != 24 {
        return Err("Invalid tool source reference".into());
    }
    let encoded = reference
        .strip_prefix("r-")
        .ok_or("Invalid tool source reference")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "Invalid tool source reference")?;
    let id = uuid::Uuid::from_slice(&bytes)
        .map_err(|_| "Invalid tool source reference")?
        .to_string();
    if self::reference(&id)? != reference {
        return Err("Noncanonical tool source reference".into());
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record<'a>(payload: &'a str) -> SourceRecord<'a> {
        SourceRecord {
            id: "10101010-2020-4030-8040-505050505050",
            run_id: "run",
            session_id: "session",
            sequence: 4,
            event_type: "ToolExecutionEnd",
            payload,
            created_at: 17,
        }
    }
    #[test]
    fn seal_keeps_original_event_and_binds_every_owner_field() {
        let event = super::super::event::AgentEvent::ToolExecutionEnd {
            call_id: "call".into(),
            tool_name: "inspect".into(),
            success: true,
            output: "中🪶\n\"".into(),
            error: None,
        };
        let original = serde_json::to_string(&event).unwrap();
        let sealed = seal(&record(&original)).unwrap();
        let decoded: super::super::event::AgentEvent = serde_json::from_str(&sealed).unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::to_value(event).unwrap()
        );
        let mut row = record(&sealed);
        assert_eq!(open(&row).unwrap().output, "中🪶\n\"");
        for field in 0..6 {
            row = record(&sealed);
            match field {
                0 => row.id = "other",
                1 => row.run_id = "other",
                2 => row.session_id = "other",
                3 => row.sequence += 1,
                4 => row.created_at += 1,
                _ => row.event_type = "MessageDelta",
            }
            assert!(open(&row).is_err());
        }
        let corrupted = sealed.replace("inspect", "changed");
        assert!(open(&record(&corrupted)).is_err());
        assert!(open(&record(&original)).is_err());
        let token = reference(record("").id).unwrap();
        assert_eq!(token.len(), 24);
        assert_eq!(record_id(&token).unwrap(), record("").id);
        assert!(record_id("r-forged").is_err());
    }
}
