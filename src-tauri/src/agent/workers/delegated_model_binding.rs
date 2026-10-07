//! Durable, non-secret model identity for a delegated WorkPackage.
//!
//! This module deliberately knows neither foreground session configuration nor
//! Tauri. The Main Agent selects and validates opaque references at issuance;
//! a future application host injects a factory that resolves the credential
//! handle through the OS keychain when the scheduler starts an attempt.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::llm::LlmProvider;

use super::work_package::WorkPackageScope;

pub const DELEGATED_MODEL_BINDING_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedModelBindingRequest {
    pub provider_profile_ref: String,
    pub model_ref: String,
    pub credential_handle_ref: String,
    pub policy_version: u16,
}

impl DelegatedModelBindingRequest {
    pub fn new(
        provider_profile_ref: impl Into<String>,
        model_ref: impl Into<String>,
        credential_handle_ref: impl Into<String>,
        policy_version: u16,
    ) -> Result<Self, ModelBindingError> {
        let value = Self {
            provider_profile_ref: provider_profile_ref.into(),
            model_ref: model_ref.into(),
            credential_handle_ref: credential_handle_ref.into(),
            policy_version,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), ModelBindingError> {
        for value in [&self.provider_profile_ref, &self.model_ref] {
            if !safe_ref(value) {
                return Err(ModelBindingError::Invalid);
            }
        }
        // Accept only known opaque protected-store handles. In particular, a
        // model/API token cannot masquerade as a generic identifier.
        let known_handle = self.credential_handle_ref.starts_with("keychain:")
            || self
                .credential_handle_ref
                .strip_prefix("chatgpt_plan_v1_")
                .is_some_and(|reference| uuid::Uuid::parse_str(reference).is_ok());
        if !known_handle || !safe_ref(&self.credential_handle_ref) {
            return Err(ModelBindingError::Invalid);
        }
        if self.policy_version == 0 {
            return Err(ModelBindingError::Invalid);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedModelBinding {
    pub id: String,
    pub work_package_id: String,
    pub owner_profile_id: i64,
    pub workspace_key: String,
    pub provider_profile_ref: String,
    pub model_ref: String,
    pub credential_handle_ref: String,
    pub policy_version: u16,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelHostUnavailable {
    Unavailable,
    Revoked,
    PolicyMismatch,
}

impl ModelHostUnavailable {
    /// Safe, durable classification for a future scheduler/Main-Agent
    /// attention state. It intentionally contains no provider or keychain text.
    pub fn needs_attention_kind(self) -> &'static str {
        match self {
            Self::Unavailable => "delegated_model_unavailable",
            Self::Revoked => "delegated_model_revoked",
            Self::PolicyMismatch => "delegated_model_policy_mismatch",
        }
    }
}

/// Future scheduler port. Implementations may use the OS keychain internally,
/// but are forbidden from returning decrypted credentials or raw provider errors.
pub trait DelegatedModelHostFactory: Send + Sync {
    fn resolve_at_delegation(
        &self,
        request: &DelegatedModelBindingRequest,
    ) -> Result<(), ModelHostUnavailable>;

    fn create_model_host(
        &self,
        binding: &DelegatedModelBinding,
    ) -> Result<Arc<dyn LlmProvider>, ModelHostUnavailable>;
}

/// Main-Agent issuance guard for the currently resolved (but non-secret)
/// provider selection. It cannot construct a host itself: production startup
/// must replace it with a keychain-backed factory. Keeping this tiny guard
/// separate prevents `delegate_work` from reaching into foreground state.
#[derive(Debug, Clone)]
pub struct ExactModelBindingSelection {
    selected: DelegatedModelBindingRequest,
}

impl ExactModelBindingSelection {
    pub fn new(selected: DelegatedModelBindingRequest) -> Self {
        Self { selected }
    }
}

impl DelegatedModelHostFactory for ExactModelBindingSelection {
    fn resolve_at_delegation(
        &self,
        request: &DelegatedModelBindingRequest,
    ) -> Result<(), ModelHostUnavailable> {
        (request == &self.selected)
            .then_some(())
            .ok_or(ModelHostUnavailable::PolicyMismatch)
    }

    fn create_model_host(
        &self,
        _: &DelegatedModelBinding,
    ) -> Result<Arc<dyn LlmProvider>, ModelHostUnavailable> {
        Err(ModelHostUnavailable::Unavailable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelBindingError {
    Invalid,
    NotFound,
    NotActive,
    ScopeMismatch,
    AlreadyBound,
    Resolution(ModelHostUnavailable),
    Storage,
}

pub struct DelegatedModelBindingRepository;

impl DelegatedModelBindingRepository {
    /// Issue exactly one immutable binding for an active WorkPackage. The
    /// factory validates the non-secret references while the Main Agent still
    /// owns the delegation decision; no credential is stored or returned.
    pub fn create(
        conn: &Connection,
        scope: &WorkPackageScope,
        work_package_id: &str,
        request: &DelegatedModelBindingRequest,
        factory: &dyn DelegatedModelHostFactory,
        now: i64,
    ) -> Result<DelegatedModelBinding, ModelBindingError> {
        request.validate()?;
        factory
            .resolve_at_delegation(request)
            .map_err(ModelBindingError::Resolution)?;
        let package_status: Option<String> = conn
            .query_row(
                "SELECT status FROM work_packages WHERE id=?1 AND session_id=?2 AND owner_profile_id=?3 AND workspace_key=?4",
                params![work_package_id, scope.session_id, scope.owner_profile_id, scope.workspace_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| ModelBindingError::Storage)?;
        match package_status.as_deref() {
            None => return Err(ModelBindingError::NotFound),
            Some("active") => {}
            Some(_) => return Err(ModelBindingError::NotActive),
        }
        let id = format!("dmb_{}", Uuid::new_v4().simple());
        let inserted = conn.execute(
            "INSERT INTO delegated_model_bindings
             (id,work_package_id,owner_profile_id,workspace_key,provider_profile_ref,model_ref,credential_handle_ref,policy_version,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![id, work_package_id, scope.owner_profile_id, scope.workspace_key,
                request.provider_profile_ref, request.model_ref, request.credential_handle_ref,
                i64::from(request.policy_version), now],
        );
        match inserted {
            Ok(_) => Self::load(conn, scope, work_package_id),
            Err(error) if error.to_string().contains("UNIQUE constraint failed") => {
                Err(ModelBindingError::AlreadyBound)
            }
            Err(error) if error.to_string().contains("scope mismatch") => {
                Err(ModelBindingError::ScopeMismatch)
            }
            Err(_) => Err(ModelBindingError::Storage),
        }
    }

    pub fn load(
        conn: &Connection,
        scope: &WorkPackageScope,
        work_package_id: &str,
    ) -> Result<DelegatedModelBinding, ModelBindingError> {
        conn.query_row(
            "SELECT id,work_package_id,owner_profile_id,workspace_key,provider_profile_ref,model_ref,credential_handle_ref,policy_version,created_at
             FROM delegated_model_bindings WHERE work_package_id=?1 AND owner_profile_id=?2 AND workspace_key=?3",
            params![work_package_id, scope.owner_profile_id, scope.workspace_key],
            |row| {
                let policy_version: i64 = row.get(7)?;
                Ok(DelegatedModelBinding {
                    id: row.get(0)?, work_package_id: row.get(1)?, owner_profile_id: row.get(2)?, workspace_key: row.get(3)?,
                    provider_profile_ref: row.get(4)?, model_ref: row.get(5)?, credential_handle_ref: row.get(6)?,
                    policy_version: u16::try_from(policy_version).map_err(|_| rusqlite::Error::InvalidQuery)?, created_at: row.get(8)?,
                })
            },
        ).optional().map_err(|_| ModelBindingError::Storage)?.ok_or(ModelBindingError::NotFound)
    }
}

fn safe_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.' | b'/')
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn subscription_accepts_only_an_opaque_registration_handle() {
        let reference = format!("chatgpt_plan_v1_{}", uuid::Uuid::new_v4());
        assert!(super::DelegatedModelBindingRequest::new(
            "provider:openai",
            "model:fixture",
            reference,
            1
        )
        .is_ok());
        for invalid in [
            "chatgpt_plan_v1_bad",
            "chatgpt_plan_v1_sk-secret",
            "Bearer token",
        ] {
            assert!(super::DelegatedModelBindingRequest::new(
                "provider:openai",
                "model:fixture",
                invalid,
                1
            )
            .is_err());
        }
    }
    use super::*;
    use crate::agent::{
        work_package::{NewWorkPackage, WorkPackageRepository},
        worker_policy::{TaskShape, WorkerProfile},
    };

    struct Factory(ModelHostUnavailable);
    impl DelegatedModelHostFactory for Factory {
        fn resolve_at_delegation(
            &self,
            _: &DelegatedModelBindingRequest,
        ) -> Result<(), ModelHostUnavailable> {
            if self.0 == ModelHostUnavailable::Unavailable {
                Ok(())
            } else {
                Err(self.0)
            }
        }
        fn create_model_host(
            &self,
            _: &DelegatedModelBinding,
        ) -> Result<Arc<dyn LlmProvider>, ModelHostUnavailable> {
            Err(self.0)
        }
    }
    fn scope() -> WorkPackageScope {
        WorkPackageScope {
            session_id: "s".into(),
            owner_profile_id: 1,
            workspace_key: "workspace".into(),
        }
    }
    fn request() -> DelegatedModelBindingRequest {
        DelegatedModelBindingRequest::new(
            "provider:default",
            "model:example",
            "keychain:default",
            DELEGATED_MODEL_BINDING_VERSION,
        )
        .unwrap()
    }
    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate(&c).unwrap();
        c.execute(
            "INSERT INTO sessions (id,title,created_at,updated_at) VALUES ('s','s',1,1)",
            [],
        )
        .unwrap();
        c
    }
    fn package(c: &mut Connection) -> String {
        WorkPackageRepository::create(
            c,
            NewWorkPackage {
                scope: scope(),
                task_shape: TaskShape::Explore,
                worker_profile: WorkerProfile::Explorer,
                scope_digest: "digest".into(),
                capability_scope_ref: "source".into(),
                capability_expires_at: 100,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn durable_scope_and_immutability_are_enforced_without_secrets() {
        let mut c = db();
        let package = package(&mut c);
        let binding = DelegatedModelBindingRepository::create(
            &c,
            &scope(),
            &package,
            &request(),
            &Factory(ModelHostUnavailable::Unavailable),
            10,
        )
        .unwrap();
        assert_eq!(binding.credential_handle_ref, "keychain:default");
        let serialized = format!("{:?}", binding);
        assert!(!serialized.contains("sk-"));
        assert_eq!(
            DelegatedModelBindingRepository::create(
                &c,
                &scope(),
                &package,
                &request(),
                &Factory(ModelHostUnavailable::Unavailable),
                11
            ),
            Err(ModelBindingError::AlreadyBound)
        );
        assert!(c
            .execute(
                "UPDATE delegated_model_bindings SET model_ref='model:other' WHERE id=?1",
                [binding.id]
            )
            .is_err());
    }

    #[test]
    fn scope_and_provider_resolution_fail_closed() {
        let mut c = db();
        let package = package(&mut c);
        let wrong = WorkPackageScope {
            owner_profile_id: 2,
            ..scope()
        };
        assert_eq!(
            DelegatedModelBindingRepository::create(
                &c,
                &wrong,
                &package,
                &request(),
                &Factory(ModelHostUnavailable::Unavailable),
                10
            ),
            Err(ModelBindingError::NotFound)
        );
        let wrong_workspace = WorkPackageScope {
            workspace_key: "other".into(),
            ..scope()
        };
        assert_eq!(
            DelegatedModelBindingRepository::load(&c, &wrong_workspace, &package),
            Err(ModelBindingError::NotFound)
        );
        assert_eq!(
            DelegatedModelBindingRepository::create(
                &c,
                &scope(),
                &package,
                &request(),
                &Factory(ModelHostUnavailable::PolicyMismatch),
                10
            ),
            Err(ModelBindingError::Resolution(
                ModelHostUnavailable::PolicyMismatch
            ))
        );
        assert_eq!(
            ModelHostUnavailable::Unavailable.needs_attention_kind(),
            "delegated_model_unavailable"
        );
        assert_eq!(
            ModelHostUnavailable::Revoked.needs_attention_kind(),
            "delegated_model_revoked"
        );
        assert_eq!(
            ModelHostUnavailable::PolicyMismatch.needs_attention_kind(),
            "delegated_model_policy_mismatch"
        );
    }

    #[test]
    fn binding_does_not_store_secret_text() {
        let mut c = db();
        let package = package(&mut c);
        let bad =
            DelegatedModelBindingRequest::new("provider:default", "model:example", "sk-secret", 1);
        assert_eq!(bad, Err(ModelBindingError::Invalid));
        let binding = DelegatedModelBindingRepository::create(
            &c,
            &scope(),
            &package,
            &request(),
            &Factory(ModelHostUnavailable::Unavailable),
            10,
        )
        .unwrap();
        let dump:String=c.query_row("SELECT provider_profile_ref || '|' || model_ref || '|' || credential_handle_ref FROM delegated_model_bindings WHERE id=?1",[binding.id],|r|r.get(0)).unwrap();
        assert!(!dump.contains("secret"));
    }

    #[test]
    fn migration_recovers_when_v50_journal_entry_is_missing() {
        let c = db();
        c.execute("DELETE FROM migration_log WHERE version=50", [])
            .unwrap();
        c.execute(
            "UPDATE settings SET value='49' WHERE key='schema_version'",
            [],
        )
        .unwrap();
        crate::db::migrate(&c).unwrap();
        let applied: String = c
            .query_row(
                "SELECT status FROM migration_log WHERE version=50",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(applied, "applied");
    }
}
