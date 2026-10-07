//! Credential acquisition is independent of the model wire protocol.
//! Secrets stay behind this seam; callers retain only a non-secret reference.

use std::sync::Arc;

#[async_trait::async_trait]
pub trait ModelCredentialSource: Send + Sync {
    async fn bearer_token(&self) -> Result<String, String>;
    fn credential_ref(&self) -> &str;
    /// Re-check revocation before accepting completed inference or tool calls.
    fn validate_session(&self) -> Result<(), String> {
        Ok(())
    }
}

pub struct StaticCredentialSource {
    token: String,
    reference: String,
}

impl StaticCredentialSource {
    pub fn new(token: String, reference: String) -> Arc<dyn ModelCredentialSource> {
        Arc::new(Self { token, reference })
    }
}

#[async_trait::async_trait]
impl ModelCredentialSource for StaticCredentialSource {
    async fn bearer_token(&self) -> Result<String, String> {
        Ok(self.token.clone())
    }

    fn credential_ref(&self) -> &str {
        &self.reference
    }
}
