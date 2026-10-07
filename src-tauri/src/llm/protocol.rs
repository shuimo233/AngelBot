//! Model transport, authentication and private continuation contracts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) const MAX_CONTINUATION_ITEMS: usize = 128;
pub(super) const MAX_CONTINUATION_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProtocol {
    OpenaiChatCompletions,
    OpenaiResponses,
    AnthropicMessages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelAuthMode {
    #[default]
    ApiKey,
    None,
    ChatgptPlan,
}

/// Complete output items from one completed model request. This data is private
/// to backend history, never a UI activity or a new source of tool authority.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolContinuation {
    pub protocol: ModelProtocol,
    pub model: String,
    pub credential_ref: String,
    pub output_items: Vec<Value>,
}

impl std::fmt::Debug for ProtocolContinuation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtocolContinuation")
            .field("protocol", &self.protocol)
            .field("items", &self.output_items.len())
            .finish_non_exhaustive()
    }
}

impl ProtocolContinuation {
    pub fn validate(&self) -> Result<(), String> {
        if self.protocol != ModelProtocol::OpenaiResponses
            || self.model.trim().is_empty()
            || self.model.len() > 200
            || self.credential_ref.trim().is_empty()
            || self.credential_ref.len() > 256
        {
            return Err("Invalid private model continuation: binding".into());
        }
        if self.output_items.is_empty() {
            return Err("Invalid private model continuation: empty output".into());
        }
        if self.output_items.len() > MAX_CONTINUATION_ITEMS {
            return Err("Invalid private model continuation: output count".into());
        }
        let encoded = serde_json::to_vec(&self.output_items)
            .map_err(|_| "Invalid private model continuation: encoding")?;
        if encoded.len() > MAX_CONTINUATION_BYTES {
            return Err("Invalid private model continuation: output size".into());
        }
        if self.output_items.iter().any(|item| {
            !matches!(
                item.get("type").and_then(Value::as_str),
                Some("message" | "reasoning" | "function_call" | "custom_tool_call")
            )
        }) {
            return Err("Invalid private model continuation: unknown output type".into());
        }
        Ok(())
    }

    pub fn matches(&self, model: &str, credential_ref: &str) -> bool {
        self.model == model && self.credential_ref == credential_ref
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn private_continuation_failures_report_fixed_reasons_without_private_values() {
        let valid = ProtocolContinuation {
            protocol: ModelProtocol::OpenaiResponses,
            model: "fixture-private-model".into(),
            credential_ref: "fixture-private-credential".into(),
            output_items: vec![json!({"type":"message","private":"fixture-private-output"})],
        };
        let mut cases = Vec::new();
        let mut wrong_protocol = valid.clone();
        wrong_protocol.protocol = ModelProtocol::AnthropicMessages;
        cases.push((wrong_protocol, "binding"));
        let mut empty_model = valid.clone();
        empty_model.model = " ".into();
        cases.push((empty_model, "binding"));
        let mut long_model = valid.clone();
        long_model.model = "fixture-private".repeat(14);
        cases.push((long_model, "binding"));
        let mut empty_credential = valid.clone();
        empty_credential.credential_ref = " ".into();
        cases.push((empty_credential, "binding"));
        let mut long_credential = valid.clone();
        long_credential.credential_ref = "fixture-private".repeat(18);
        cases.push((long_credential, "binding"));
        let mut empty_output = valid.clone();
        empty_output.output_items.clear();
        cases.push((empty_output, "empty output"));
        let mut too_many_outputs = valid.clone();
        too_many_outputs.output_items = vec![valid.output_items[0].clone(); 129];
        cases.push((too_many_outputs, "output count"));
        let mut oversized_output = valid.clone();
        oversized_output.output_items = vec![json!({
            "type":"message","private":"fixture-private".repeat(40_000)
        })];
        cases.push((oversized_output, "output size"));
        let mut unknown_output = valid.clone();
        unknown_output.output_items = vec![json!({
            "type":"fixture-private-kind","private":"fixture-private-output"
        })];
        cases.push((unknown_output, "unknown output type"));
        let mut missing_output_type = valid.clone();
        missing_output_type.output_items = vec![json!({"private":"fixture-private-output"})];
        cases.push((missing_output_type, "unknown output type"));

        for (continuation, reason) in cases {
            let error = continuation.validate().unwrap_err();
            assert_eq!(
                error,
                format!("Invalid private model continuation: {reason}")
            );
            assert!(!error.contains("fixture-private"));
        }
        assert!(valid.validate().is_ok());
    }
}
