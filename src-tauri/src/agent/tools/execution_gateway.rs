//! Capability-based policy for delegated attempts.
//!
//! This module deliberately *does not execute tools*.  A future delegation
//! runtime must ask this gateway for a decision before it crosses the normal
//! tool/permission/confirmation boundary.  Keeping the policy here pure and
//! narrow prevents a second, ungoverned execution path for sub-agents.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::delegation::LeaseStatus;

/// A lease after its grants have been validated and canonicalized.  It is a
/// runtime view of the durable lease metadata, not a filesystem capability by
/// itself: every execution intent must still be authorized by this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityLease {
    pub id: String,
    pub attempt_id: String,
    pub status: LeaseStatus,
    /// Epoch milliseconds. A lease is expired at this instant, not after it.
    pub expires_at_ms: i64,
    read_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
    tool_allowlist: BTreeSet<String>,
    network_hosts: BTreeSet<String>,
}

/// Raw, narrow data needed to construct a [`CapabilityLease`].  A persistence
/// adapter can deserialize this directly from `delegation_capability_leases`
/// without granting the caller any ability to execute an intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGrant {
    pub id: String,
    pub attempt_id: String,
    pub status: LeaseStatus,
    pub expires_at_ms: i64,
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    pub tool_allowlist: Vec<String>,
    pub network_hosts: Vec<String>,
}

impl CapabilityLease {
    /// Validates the grant with deny-by-default semantics. Roots must already
    /// exist, which is intentional: a main agent allocates a sandbox before
    /// issuing a lease instead of letting a child choose an arbitrary root.
    pub fn from_grant(grant: CapabilityGrant) -> Result<Self, CapabilityGrantError> {
        if grant.id.trim().is_empty() || grant.attempt_id.trim().is_empty() {
            return Err(CapabilityGrantError::MissingIdentity);
        }
        if grant.expires_at_ms <= 0 {
            return Err(CapabilityGrantError::InvalidExpiry);
        }

        let read_roots = canonical_roots(grant.read_roots)?;
        let write_roots = canonical_roots(grant.write_roots)?;
        let tool_allowlist = normalized_identifiers(grant.tool_allowlist)
            .map_err(CapabilityGrantError::InvalidToolIdentifier)?;
        let network_hosts = grant
            .network_hosts
            .into_iter()
            .map(|host| {
                normalize_host(&host).ok_or_else(|| CapabilityGrantError::InvalidNetworkHost(host))
            })
            .collect::<Result<BTreeSet<_>, _>>()?;

        Ok(Self {
            id: grant.id,
            attempt_id: grant.attempt_id,
            status: grant.status,
            expires_at_ms: grant.expires_at_ms,
            read_roots,
            write_roots,
            tool_allowlist,
            network_hosts,
        })
    }

    /// Introspection only.  Execution still has to pass through
    /// [`ExecutionGateway::authorize`]; this cannot grant a capability.
    pub fn permits_tool(&self, tool_id: &str) -> bool {
        self.tool_allowlist.contains(tool_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityGrantError {
    MissingIdentity,
    InvalidExpiry,
    InvalidRoot(PathBuf),
    InvalidToolIdentifier(String),
    InvalidNetworkHost(String),
}

/// An operation requested by a delegated attempt.  `request_fingerprint` is a
/// stable, caller-supplied digest/identifier of the requested effect; never
/// put secret request bodies in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionIntent {
    pub tool_id: String,
    pub action: ExecutionAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionAction {
    ReadFile {
        path: PathBuf,
    },
    WriteFile {
        path: PathBuf,
        request_fingerprint: String,
    },
    Network {
        host: String,
        request_fingerprint: String,
    },
    /// For a tool with no filesystem/network target.  Marking an action as
    /// effectful requires an idempotency key just like writes and network.
    Tool {
        operation: String,
        effectful: bool,
        request_fingerprint: String,
    },
}

impl ExecutionAction {
    fn is_effectful(&self) -> bool {
        matches!(
            self,
            Self::WriteFile { .. }
                | Self::Network { .. }
                | Self::Tool {
                    effectful: true,
                    ..
                }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionDecision {
    Allow(AllowedExecution),
    Deny(ExecutionDenied),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedExecution {
    /// Present only for effects. Pass it to the eventual execution gateway as
    /// the durable idempotency/effect key.
    pub effect_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionDenied {
    LeaseRevoked,
    LeaseExpired,
    ToolNotAllowed(String),
    ReadPathNotAllowed(PathBuf),
    WritePathNotAllowed(PathBuf),
    NetworkHostNotAllowed(String),
    InvalidIntent(String),
}

/// Stateless authorization policy. `now_ms` is supplied by the caller, which
/// keeps decisions deterministic in tests and makes expiry behavior explicit.
pub struct ExecutionGateway;

impl ExecutionGateway {
    pub fn authorize(
        lease: &CapabilityLease,
        intent: &ExecutionIntent,
        now_ms: i64,
    ) -> ExecutionDecision {
        if lease.status != LeaseStatus::Active {
            return ExecutionDecision::Deny(ExecutionDenied::LeaseRevoked);
        }
        if now_ms >= lease.expires_at_ms {
            return ExecutionDecision::Deny(ExecutionDenied::LeaseExpired);
        }
        if !lease.tool_allowlist.contains(&intent.tool_id) {
            return ExecutionDecision::Deny(ExecutionDenied::ToolNotAllowed(
                intent.tool_id.clone(),
            ));
        }

        let authorization = match &intent.action {
            ExecutionAction::ReadFile { path } => {
                authorize_path(path, &lease.read_roots).map_err(ExecutionDenied::ReadPathNotAllowed)
            }
            ExecutionAction::WriteFile { path, .. } => authorize_path(path, &lease.write_roots)
                .map_err(ExecutionDenied::WritePathNotAllowed),
            ExecutionAction::Network { host, .. } => match normalize_host(host) {
                Some(normalized) if lease.network_hosts.contains(&normalized) => Ok(()),
                Some(normalized) => Err(ExecutionDenied::NetworkHostNotAllowed(normalized)),
                None => Err(ExecutionDenied::InvalidIntent(
                    "network host must be a bare DNS name".into(),
                )),
            },
            ExecutionAction::Tool {
                operation,
                effectful,
                request_fingerprint,
            } => {
                if operation.trim().is_empty()
                    || (*effectful && request_fingerprint.trim().is_empty())
                {
                    Err(ExecutionDenied::InvalidIntent(
                        "tool operation and effect fingerprint are required".into(),
                    ))
                } else {
                    Ok(())
                }
            }
        };
        if let Err(reason) = authorization {
            return ExecutionDecision::Deny(reason);
        }

        let effect_key = if intent.action.is_effectful() {
            match effect_key(lease, intent) {
                Ok(key) => Some(key),
                Err(reason) => return ExecutionDecision::Deny(reason),
            }
        } else {
            None
        };
        ExecutionDecision::Allow(AllowedExecution { effect_key })
    }
}

/// Deterministically derives an effect key only after an intent has been
/// authorized.  The attempt and lease identities deliberately scope keys so a
/// retry in a fresh sandbox cannot accidentally deduplicate an old attempt.
pub fn effect_key(
    lease: &CapabilityLease,
    intent: &ExecutionIntent,
) -> Result<String, ExecutionDenied> {
    if !intent.action.is_effectful() {
        return Err(ExecutionDenied::InvalidIntent(
            "read-only intents have no effect key".into(),
        ));
    }
    let material = effect_material(intent)?;
    let mut hasher = Sha256::new();
    for field in [&lease.id, &lease.attempt_id, &intent.tool_id, &material] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    Ok(format!("effect:{}", hex::encode(hasher.finalize())))
}

fn effect_material(intent: &ExecutionIntent) -> Result<String, ExecutionDenied> {
    match &intent.action {
        ExecutionAction::WriteFile {
            path,
            request_fingerprint,
        } => {
            if request_fingerprint.trim().is_empty() {
                return Err(ExecutionDenied::InvalidIntent(
                    "write effect fingerprint is required".into(),
                ));
            }
            Ok(format!("write:{}:{}", path.display(), request_fingerprint))
        }
        ExecutionAction::Network {
            host,
            request_fingerprint,
        } => {
            let host = normalize_host(host).ok_or_else(|| {
                ExecutionDenied::InvalidIntent("network host must be a bare DNS name".into())
            })?;
            if request_fingerprint.trim().is_empty() {
                return Err(ExecutionDenied::InvalidIntent(
                    "network effect fingerprint is required".into(),
                ));
            }
            Ok(format!("network:{host}:{request_fingerprint}"))
        }
        ExecutionAction::Tool {
            operation,
            effectful: true,
            request_fingerprint,
        } => {
            if operation.trim().is_empty() || request_fingerprint.trim().is_empty() {
                return Err(ExecutionDenied::InvalidIntent(
                    "tool operation and effect fingerprint are required".into(),
                ));
            }
            Ok(format!("tool:{operation}:{request_fingerprint}"))
        }
        _ => Err(ExecutionDenied::InvalidIntent(
            "read-only intents have no effect key".into(),
        )),
    }
}

fn canonical_roots(roots: Vec<PathBuf>) -> Result<Vec<PathBuf>, CapabilityGrantError> {
    roots
        .into_iter()
        .map(|root| {
            let canonical = fs::canonicalize(&root)
                .map_err(|_| CapabilityGrantError::InvalidRoot(root.clone()))?;
            if canonical.is_dir() {
                Ok(canonical)
            } else {
                Err(CapabilityGrantError::InvalidRoot(root))
            }
        })
        .collect()
}

fn normalized_identifiers(values: Vec<String>) -> Result<BTreeSet<String>, String> {
    values
        .into_iter()
        .map(|value| {
            let normalized = value.trim().to_owned();
            if normalized.is_empty() || normalized.chars().any(char::is_whitespace) {
                Err(value)
            } else {
                Ok(normalized)
            }
        })
        .collect()
}

/// Only exact, lower-case DNS names are accepted.  Schemes, ports, paths and
/// wildcard suffixes are intentionally not interpreted by this policy.
fn normalize_host(host: &str) -> Option<String> {
    let normalized = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.len() > 253
        || normalized.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
        || normalized.split('.').any(|label| {
            label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-')
        })
    {
        return None;
    }
    Some(normalized)
}

fn authorize_path(path: &Path, roots: &[PathBuf]) -> Result<(), PathBuf> {
    if roots.is_empty() || !path.is_absolute() || has_parent_component(path) {
        return Err(path.to_path_buf());
    }
    let resolved = canonicalize_for_authorization(path).map_err(|_| path.to_path_buf())?;
    if roots.iter().any(|root| resolved.starts_with(root)) {
        Ok(())
    } else {
        Err(path.to_path_buf())
    }
}

fn has_parent_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

/// Canonicalizes a path even when the final file does not exist.  The deepest
/// existing ancestor is canonicalized first, so symlinks cannot escape an
/// authorized root through a newly-created child path.
fn canonicalize_for_authorization(path: &Path) -> std::io::Result<PathBuf> {
    let mut cursor = path;
    let mut missing = Vec::new();
    while !cursor.exists() {
        let name = cursor
            .file_name()
            .ok_or_else(|| std::io::Error::other("path has no existing ancestor"))?;
        missing.push(name.to_os_string());
        cursor = cursor
            .parent()
            .ok_or_else(|| std::io::Error::other("path has no existing ancestor"))?;
    }
    let mut canonical = fs::canonicalize(cursor)?;
    for component in missing.iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn lease(root: &Path) -> CapabilityLease {
        CapabilityLease::from_grant(CapabilityGrant {
            id: "lease-1".into(),
            attempt_id: "attempt-1".into(),
            status: LeaseStatus::Active,
            expires_at_ms: 100,
            read_roots: vec![root.to_path_buf()],
            write_roots: vec![root.to_path_buf()],
            tool_allowlist: vec![
                "files.read".into(),
                "files.write".into(),
                "web.fetch".into(),
            ],
            network_hosts: vec!["api.example.test".into()],
        })
        .unwrap()
    }

    #[test]
    fn denies_path_traversal_even_when_lexically_under_root() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("safe").join("..").join("outside.txt");
        let intent = ExecutionIntent {
            tool_id: "files.read".into(),
            action: ExecutionAction::ReadFile { path: target },
        };
        assert!(matches!(
            ExecutionGateway::authorize(&lease(temp.path()), &intent, 1),
            ExecutionDecision::Deny(ExecutionDenied::ReadPathNotAllowed(_))
        ));
    }

    #[test]
    fn denies_sibling_prefix_path() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("sandbox");
        let sibling = temp.path().join("sandbox-other");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        let intent = ExecutionIntent {
            tool_id: "files.write".into(),
            action: ExecutionAction::WriteFile {
                path: sibling.join("result.txt"),
                request_fingerprint: "body-v1".into(),
            },
        };
        assert!(matches!(
            ExecutionGateway::authorize(&lease(&root), &intent, 1),
            ExecutionDecision::Deny(ExecutionDenied::WritePathNotAllowed(_))
        ));
    }

    #[test]
    fn rejects_expired_and_revoked_leases() {
        let temp = tempdir().unwrap();
        let intent = ExecutionIntent {
            tool_id: "files.read".into(),
            action: ExecutionAction::ReadFile {
                path: temp.path().join("input.txt"),
            },
        };
        assert_eq!(
            ExecutionGateway::authorize(&lease(temp.path()), &intent, 100),
            ExecutionDecision::Deny(ExecutionDenied::LeaseExpired)
        );
        let mut revoked = lease(temp.path());
        revoked.status = LeaseStatus::Revoked;
        assert_eq!(
            ExecutionGateway::authorize(&revoked, &intent, 1),
            ExecutionDecision::Deny(ExecutionDenied::LeaseRevoked)
        );
    }

    #[test]
    fn enforces_tool_and_network_allowlists() {
        let temp = tempdir().unwrap();
        let forbidden_tool = ExecutionIntent {
            tool_id: "shell.exec".into(),
            action: ExecutionAction::Tool {
                operation: "run".into(),
                effectful: false,
                request_fingerprint: String::new(),
            },
        };
        assert_eq!(
            ExecutionGateway::authorize(&lease(temp.path()), &forbidden_tool, 1),
            ExecutionDecision::Deny(ExecutionDenied::ToolNotAllowed("shell.exec".into()))
        );
        let forbidden_host = ExecutionIntent {
            tool_id: "web.fetch".into(),
            action: ExecutionAction::Network {
                host: "evil.example.test".into(),
                request_fingerprint: "request-v1".into(),
            },
        };
        assert_eq!(
            ExecutionGateway::authorize(&lease(temp.path()), &forbidden_host, 1),
            ExecutionDecision::Deny(ExecutionDenied::NetworkHostNotAllowed(
                "evil.example.test".into()
            ))
        );
    }

    #[test]
    fn separates_read_and_write_roots() {
        let temp = tempdir().unwrap();
        let read = temp.path().join("read");
        let write = temp.path().join("write");
        fs::create_dir_all(&read).unwrap();
        fs::create_dir_all(&write).unwrap();
        let lease = CapabilityLease::from_grant(CapabilityGrant {
            read_roots: vec![read.clone()],
            write_roots: vec![write.clone()],
            ..grant_for(&read)
        })
        .unwrap();
        let write_to_read = ExecutionIntent {
            tool_id: "files.write".into(),
            action: ExecutionAction::WriteFile {
                path: read.join("no.txt"),
                request_fingerprint: "v1".into(),
            },
        };
        let read_from_write = ExecutionIntent {
            tool_id: "files.read".into(),
            action: ExecutionAction::ReadFile {
                path: write.join("no.txt"),
            },
        };
        assert!(matches!(
            ExecutionGateway::authorize(&lease, &write_to_read, 1),
            ExecutionDecision::Deny(ExecutionDenied::WritePathNotAllowed(_))
        ));
        assert!(matches!(
            ExecutionGateway::authorize(&lease, &read_from_write, 1),
            ExecutionDecision::Deny(ExecutionDenied::ReadPathNotAllowed(_))
        ));
    }

    #[test]
    fn effect_keys_are_stable_and_scoped_to_attempt() {
        let temp = tempdir().unwrap();
        let intent = ExecutionIntent {
            tool_id: "files.write".into(),
            action: ExecutionAction::WriteFile {
                path: temp.path().join("result.txt"),
                request_fingerprint: "content-sha256".into(),
            },
        };
        let first = ExecutionGateway::authorize(&lease(temp.path()), &intent, 1);
        let second = ExecutionGateway::authorize(&lease(temp.path()), &intent, 1);
        assert_eq!(first, second);
        let key = match first {
            ExecutionDecision::Allow(allowed) => allowed.effect_key.unwrap(),
            other => panic!("unexpected decision: {other:?}"),
        };
        let mut next_attempt = lease(temp.path());
        next_attempt.attempt_id = "attempt-2".into();
        assert_ne!(key, effect_key(&next_attempt, &intent).unwrap());
    }

    fn grant_for(root: &Path) -> CapabilityGrant {
        CapabilityGrant {
            id: "lease-1".into(),
            attempt_id: "attempt-1".into(),
            status: LeaseStatus::Active,
            expires_at_ms: 100,
            read_roots: vec![root.to_path_buf()],
            write_roots: vec![root.to_path_buf()],
            tool_allowlist: vec![
                "files.read".into(),
                "files.write".into(),
                "web.fetch".into(),
            ],
            network_hosts: vec!["api.example.test".into()],
        }
    }
}
