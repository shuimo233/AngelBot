//! Official Sign in with ChatGPT public-client flow.
//!
//! Only opaque registration references and display state cross this interface.
//! Tokens stay behind the credential-source seam, in an AES-GCM encrypted record
//! whose key is protected by the existing OS vault. No Codex auth.json is read.
//! Contracts: developers.openai.com/siwc/token-sharing-open-source/sign-in and
//! profiles-and-sessions. All network/browser/store adapters are private.

use super::auth::ModelCredentialSource;
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use fs2::FileExt;
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use oauth2::{CsrfToken, PkceCodeChallenge, PkceCodeVerifier};
use rand::{rngs::OsRng, RngCore};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tauri_plugin_keyring_store::KeyringExt;
use tauri_plugin_shell::ShellExt;
use tokio::sync::{Mutex as AsyncMutex, Notify};
use uuid::Uuid;

const ISSUER: &str = "https://auth.openai.com";
const DISCOVERY: &str = "https://auth.openai.com/.well-known/openid-configuration";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const REFERENCE_PREFIX: &str = "chatgpt_plan_v1_";
const ASSOCIATED_DATA: &[u8] = b"angelbot.chatgpt-plan.credentials:v1";
const MAX_RECORD_BYTES: usize = 256 * 1024;
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const NETWORK_ERROR: &str = "无法连接 OpenAI 认证服务，请检查网络与 Windows 系统代理后重试。";
const STORE_ERROR: &str = "无法访问 ChatGPT 的受保护凭据，请检查系统凭据库。";
const SIGN_IN_REQUIRED: &str = "ChatGPT 连接已失效，请重新登录。";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptPlanStatus {
    pub connected: bool,
    pub plan_enabled: bool,
    pub needs_sign_in: bool,
    pub account_label: Option<String>,
    pub credential_ref: Option<String>,
}

// Intentionally no Debug: assertions and diagnostics must never print tokens.
#[derive(Clone, Serialize, Deserialize)]
struct Tokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: String,
    expires_at: u64,
    scopes: Vec<String>,
    nonce: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Registration {
    credential_ref: String,
    client_id: String,
    subject: String,
    email: Option<String>,
    tokens: Option<Tokens>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    version: u8,
    host_id: String,
    registration: Option<Registration>,
    // Refresh does not change this epoch; sign-in and disconnect do. It keeps
    // another process's pending browser callback from reviving a signed-out
    // session even though the stable account/client mapping is retained.
    #[serde(default)]
    authorization_epoch: u64,
    #[serde(default)]
    disconnecting: bool,
}

impl Record {
    fn new() -> Self {
        Self {
            version: 1,
            host_id: format!("urn:uuid:{}", Uuid::new_v4()),
            registration: None,
            authorization_epoch: 0,
            disconnecting: false,
        }
    }

    fn status(&self) -> ChatGptPlanStatus {
        let registration = self.registration.as_ref();
        let tokens = registration
            .and_then(|registration| registration.tokens.as_ref())
            .filter(|_| !self.disconnecting);
        ChatGptPlanStatus {
            connected: tokens.is_some(),
            plan_enabled: tokens.is_some_and(|tokens| {
                tokens.access_token.is_some()
                    && tokens.scopes.iter().any(|scope| scope == DIRECT_SCOPE)
            }),
            needs_sign_in: registration.is_some() && tokens.is_none(),
            account_label: registration.map(|registration| {
                let label = registration.email.as_deref().unwrap_or("ChatGPT 账号");
                let label: String = label
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(96)
                    .collect();
                // Email is display metadata, never identity or a workspace ID.
                format!(
                    "{label} · {}",
                    &registration.credential_ref[REFERENCE_PREFIX.len()..][..8]
                )
            }),
            credential_ref: registration.map(|registration| registration.credential_ref.clone()),
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self
                .host_id
                .strip_prefix("urn:uuid:")
                .and_then(|id| Uuid::parse_str(id).ok())
                .is_none()
        {
            return Err(STORE_ERROR.into());
        }
        if let Some(registration) = &self.registration {
            validate_reference(&registration.credential_ref)?;
            if !valid_client_id(&registration.client_id) || registration.subject.is_empty() {
                return Err(STORE_ERROR.into());
            }
        }
        Ok(())
    }
}

fn validate_reference(reference: &str) -> Result<(), String> {
    let suffix = reference
        .strip_prefix(REFERENCE_PREFIX)
        .ok_or(SIGN_IN_REQUIRED)?;
    let uuid = Uuid::parse_str(suffix).map_err(|_| SIGN_IN_REQUIRED.to_string())?;
    if suffix != uuid.to_string() {
        return Err(SIGN_IN_REQUIRED.into());
    }
    Ok(())
}

fn valid_client_id(client_id: &str) -> bool {
    client_id.starts_with("oaiapp_")
        && client_id.len() <= 256
        && client_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<Record>, String>;
    fn save(&self, record: &Record) -> Result<(), String>;
}

struct ProtectedStore {
    app: tauri::AppHandle,
    path: PathBuf,
    key_account: String,
}

impl ProtectedStore {
    fn new(app: &tauri::AppHandle) -> Self {
        let path = crate::app_data_dir().join("chatgpt-plan.credentials");
        let namespace = hex::encode(Sha256::digest(path.to_string_lossy().as_bytes()));
        Self {
            app: app.clone(),
            path,
            key_account: format!("chatgpt_plan_v1_key_{namespace}"),
        }
    }

    fn encryption_key(&self, create: bool) -> Result<Vec<u8>, String> {
        let vault = &self.app.keyring().store;
        match vault
            .get_password(&self.key_account)
            .map_err(|_| STORE_ERROR.to_string())?
        {
            Some(encoded) => {
                let key = STANDARD
                    .decode(encoded)
                    .map_err(|_| STORE_ERROR.to_string())?;
                if key.len() != 32 {
                    return Err(STORE_ERROR.into());
                }
                Ok(key)
            }
            None if create && !self.path.exists() => {
                let mut key = vec![0; 32];
                OsRng.fill_bytes(&mut key);
                vault
                    .set_password(&self.key_account, &STANDARD.encode(&key))
                    .map_err(|_| STORE_ERROR.to_string())?;
                Ok(key)
            }
            None => Err(STORE_ERROR.into()),
        }
    }
}

impl CredentialStore for ProtectedStore {
    fn load(&self) -> Result<Option<Record>, String> {
        if !self.path.exists() {
            return Ok(None);
        }
        let metadata = std::fs::metadata(&self.path).map_err(|_| STORE_ERROR.to_string())?;
        if !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES as u64 {
            return Err(STORE_ERROR.into());
        }
        let bytes = std::fs::read(&self.path).map_err(|_| STORE_ERROR.to_string())?;
        let plaintext = decrypt_record(&self.encryption_key(false)?, &bytes)?;
        let record: Record =
            serde_json::from_slice(&plaintext).map_err(|_| STORE_ERROR.to_string())?;
        record.validate()?;
        Ok(Some(record))
    }

    fn save(&self, record: &Record) -> Result<(), String> {
        record.validate()?;
        let parent = self.path.parent().ok_or(STORE_ERROR)?;
        std::fs::create_dir_all(parent).map_err(|_| STORE_ERROR.to_string())?;
        let plaintext = serde_json::to_vec(record).map_err(|_| STORE_ERROR.to_string())?;
        let bytes = encrypt_record(&self.encryption_key(true)?, &plaintext)?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| STORE_ERROR.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| STORE_ERROR.to_string())?;
        }
        temporary
            .write_all(&bytes)
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|_| STORE_ERROR.to_string())?;
        // tempfile persist atomically replaces an existing file on Windows too.
        temporary
            .persist(&self.path)
            .map_err(|_| STORE_ERROR.to_string())?;
        Ok(())
    }
}

fn encrypt_record(key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    if plaintext.len() > MAX_RECORD_BYTES - 32 {
        return Err(STORE_ERROR.into());
    }
    let mut nonce = [0; 12];
    OsRng.fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| STORE_ERROR.to_string())?;
    let encrypted = cipher
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: ASSOCIATED_DATA,
            },
        )
        .map_err(|_| STORE_ERROR.to_string())?;
    let mut bytes = nonce.to_vec();
    bytes.extend(encrypted);
    Ok(bytes)
}

fn decrypt_record(key: &[u8], bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() < 28 || bytes.len() > MAX_RECORD_BYTES {
        return Err(STORE_ERROR.into());
    }
    let nonce: [u8; 12] = bytes[..12]
        .try_into()
        .map_err(|_| STORE_ERROR.to_string())?;
    Aes256Gcm::new_from_slice(key)
        .map_err(|_| STORE_ERROR.to_string())?
        .decrypt(
            &Nonce::from(nonce),
            Payload {
                msg: &bytes[12..],
                aad: ASSOCIATED_DATA,
            },
        )
        .map_err(|_| STORE_ERROR.to_string())
}

fn process_gate() -> Arc<AsyncMutex<()>> {
    static GATE: OnceLock<Arc<AsyncMutex<()>>> = OnceLock::new();
    GATE.get_or_init(|| Arc::new(AsyncMutex::new(()))).clone()
}

struct CrossProcessGuard(File);
impl Drop for CrossProcessGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

async fn cross_process_lock(path: Option<PathBuf>) -> Result<Option<CrossProcessGuard>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    tokio::task::spawn_blocking(move || {
        let parent = path.parent().ok_or(STORE_ERROR)?;
        std::fs::create_dir_all(parent).map_err(|_| STORE_ERROR.to_string())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|_| STORE_ERROR.to_string())?;
        FileExt::lock_exclusive(&file).map_err(|_| STORE_ERROR.to_string())?;
        Ok(Some(CrossProcessGuard(file)))
    })
    .await
    .map_err(|_| STORE_ERROR.to_string())?
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    revocation_endpoint: Option<String>,
}

impl Discovery {
    fn validate(&self) -> Result<(), String> {
        if self.issuer != ISSUER
            || self.authorization_endpoint != AUTHORIZE
            || self.token_endpoint != TOKEN
        {
            return Err("OpenAI 认证元数据不匹配，已停止登录。".into());
        }
        trusted_endpoint(&self.jwks_uri)?;
        if let Some(endpoint) = &self.revocation_endpoint {
            trusted_endpoint(endpoint)?;
        }
        Ok(())
    }
}

fn trusted_endpoint(endpoint: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| NETWORK_ERROR.to_string())?;
    if url.scheme() != "https"
        || url.host_str() != Some("auth.openai.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("OpenAI 认证端点不受信任，已停止请求。".into());
    }
    Ok(())
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

enum AuthError {
    Network,
    Transport(TransportFailure),
    Protocol,
    Http(u16),
    OAuth(String),
}

enum TransportFailure {
    Timeout,
    Connect,
    Request,
    Response,
}

impl AuthError {
    fn transport(error: reqwest::Error) -> Self {
        // Classify only; raw errors can contain sensitive request URLs.
        let kind = if error.is_timeout() {
            TransportFailure::Timeout
        } else if error.is_connect() {
            TransportFailure::Connect
        } else {
            TransportFailure::Request
        };
        Self::Transport(kind)
    }

    fn transport_read(error: reqwest::Error) -> Self {
        Self::Transport(if error.is_timeout() {
            TransportFailure::Timeout
        } else {
            TransportFailure::Response
        })
    }

    fn terminal_refresh(&self) -> bool {
        matches!(self, Self::OAuth(code) if matches!(code.as_str(), "invalid_grant" | "invalid_refresh_token" | "token_expired" | "refresh_token_expired" | "refresh_token_invalidated" | "refresh_token_reused"))
    }

    fn message(&self) -> String {
        match self {
            Self::Network => NETWORK_ERROR.into(),
            Self::Transport(kind) => {
                let detail = match kind {
                    TransportFailure::Timeout => "请求超时",
                    TransportFailure::Connect => "连接建立失败（代理、网络或 TLS）",
                    TransportFailure::Request => "请求发送中断",
                    TransportFailure::Response => "响应读取中断",
                };
                format!("{detail}。请检查网络与 Windows 系统代理后重试。")
            }
            Self::Http(403) => "OpenAI 认证服务拒绝请求（HTTP 403）。请检查 Windows 系统代理是否可用，以及当前网络是否允许访问认证服务。".into(),
            Self::Http(429) => "OpenAI 认证服务请求过多（HTTP 429），请稍后重试。".into(),
            Self::Http(status) => format!("OpenAI 认证服务返回 HTTP {status}，已停止请求。请检查网络或稍后重试。"),
            Self::OAuth(code) if code == "invalid_client" => {
                "OpenAI 客户端注册无效，请检查客户端配置。".into()
            }
            Self::OAuth(code) if code == "access_denied" => "ChatGPT 授权已取消。".into(),
            Self::OAuth(code) if code == "invalid_grant" => {
                "ChatGPT 授权码已失效，请重新登录。".into()
            }
            _ => "OpenAI 认证响应格式不符合预期，已停止请求。".into(),
        }
    }

    fn message_at(&self, stage: &'static str) -> String {
        // Only fixed phase labels and safe status/code messages are exposed.
        // Never include response bodies, URLs with codes, or credential values.
        format!("{stage}失败：{}", self.message())
    }
}

#[async_trait]
trait AuthTransport: Send + Sync {
    async fn discovery(&self) -> Result<Discovery, AuthError>;
    async fn jwks(&self, endpoint: &str) -> Result<JwkSet, AuthError>;
    async fn token(&self, parameters: Vec<(String, String)>) -> Result<TokenResponse, AuthError>;
    async fn revoke(
        &self,
        endpoint: &str,
        client_id: &str,
        refresh_token: &str,
    ) -> Result<(), AuthError>;
}

struct OfficialTransport(reqwest::Client);
impl OfficialTransport {
    fn new() -> Result<Self, String> {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map(Self)
            .map_err(|error| AuthError::transport(error).message())
    }
}

async fn read_response<T: DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, AuthError> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RECORD_BYTES as u64)
    {
        return Err(AuthError::Protocol);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(AuthError::transport_read)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RECORD_BYTES {
            return Err(AuthError::Protocol);
        }
        bytes.extend_from_slice(&chunk);
    }
    decode_auth_response(status, &bytes)
}

fn decode_auth_response<T: DeserializeOwned>(
    status: reqwest::StatusCode,
    bytes: &[u8],
) -> Result<T, AuthError> {
    if !status.is_success() {
        #[derive(Deserialize)]
        struct ErrorBody {
            error: Option<String>,
        }
        let code = serde_json::from_slice::<ErrorBody>(bytes)
            .ok()
            .and_then(|body| body.error)
            .filter(|code| {
                code.len() <= 80
                    && code
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            });
        return Err(code
            .map(AuthError::OAuth)
            .unwrap_or(AuthError::Http(status.as_u16())));
    }
    serde_json::from_slice(bytes).map_err(|_| AuthError::Protocol)
}

#[async_trait]
impl AuthTransport for OfficialTransport {
    async fn discovery(&self) -> Result<Discovery, AuthError> {
        read_response(
            self.0
                .get(DISCOVERY)
                .send()
                .await
                .map_err(AuthError::transport)?,
        )
        .await
    }
    async fn jwks(&self, endpoint: &str) -> Result<JwkSet, AuthError> {
        trusted_endpoint(endpoint).map_err(|_| AuthError::Protocol)?;
        read_response(
            self.0
                .get(endpoint)
                .send()
                .await
                .map_err(AuthError::transport)?,
        )
        .await
    }
    async fn token(&self, parameters: Vec<(String, String)>) -> Result<TokenResponse, AuthError> {
        read_response(
            self.0
                .post(TOKEN)
                .form(&parameters)
                .send()
                .await
                .map_err(AuthError::transport)?,
        )
        .await
    }
    async fn revoke(
        &self,
        endpoint: &str,
        client_id: &str,
        refresh_token: &str,
    ) -> Result<(), AuthError> {
        trusted_endpoint(endpoint).map_err(|_| AuthError::Protocol)?;
        let response = self
            .0
            .post(endpoint)
            .form(&[
                ("token", refresh_token),
                ("token_type_hint", "refresh_token"),
                ("client_id", client_id),
            ])
            .send()
            .await
            .map_err(AuthError::transport)?;
        if response.status() == reqwest::StatusCode::OK {
            Ok(())
        } else if response.status().is_server_error() {
            Err(AuthError::Network)
        } else {
            Err(AuthError::Protocol)
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Clone, Serialize, Deserialize)]
struct Identity {
    sub: String,
    iss: String,
    exp: u64,
    iat: u64,
    aud: Audience,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

fn verify_identity(
    jwt: &str,
    jwks: &JwkSet,
    client_id: &str,
    expected_nonce: Option<&str>,
) -> Result<Identity, String> {
    let invalid = || "ChatGPT 身份验证失败，已停止登录。".to_string();
    let header = decode_header(jwt).map_err(|_| invalid())?;
    // An asymmetric-only allowlist prevents algorithm/key confusion. Identity
    // is trusted only after jsonwebtoken verifies the signature and claims.
    if !matches!(
        header.alg,
        Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::ES256
            | Algorithm::ES384
            | Algorithm::EdDSA
    ) {
        return Err(invalid());
    }
    let kid = header.kid.as_deref().ok_or_else(invalid)?;
    let jwk = jwks.find(kid).ok_or_else(invalid)?;
    if jwk
        .common
        .key_algorithm
        .is_some_and(|algorithm| algorithm.to_string() != format!("{:?}", header.alg))
        || jwk
            .common
            .public_key_use
            .as_ref()
            .is_some_and(|usage| usage != &jsonwebtoken::jwk::PublicKeyUse::Signature)
        || jwk
            .common
            .key_operations
            .as_ref()
            .is_some_and(|operations| {
                !operations.contains(&jsonwebtoken::jwk::KeyOperations::Verify)
            })
    {
        return Err(invalid());
    }
    let key = DecodingKey::from_jwk(jwk).map_err(|_| invalid())?;
    let mut validation = Validation::new(header.alg);
    validation.leeway = 5;
    validation.validate_nbf = true;
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["sub", "exp", "iat", "iss", "aud"]);
    let identity = decode::<Identity>(jwt, &key, &validation)
        .map_err(|_| invalid())?
        .claims;
    if identity.sub.is_empty()
        || identity.iat > now().saturating_add(5)
        || identity
            .azp
            .as_deref()
            .is_some_and(|party| party != client_id)
        || matches!(&identity.aud, Audience::Many(audiences) if audiences.len() > 1 && identity.azp.as_deref() != Some(client_id))
        || expected_nonce.is_some_and(|nonce| {
            identity
                .nonce
                .as_deref()
                .is_none_or(|returned| !secret_equal(returned, nonce))
        })
    {
        return Err(invalid());
    }
    Ok(identity)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn secret_equal(first: &str, second: &str) -> bool {
    // Equal-sized cryptographic digests keep comparison timing independent of
    // the first mismatching byte; use mature SHA-256 and subtle primitives.
    bool::from(Sha256::digest(first.as_bytes()).ct_eq(&Sha256::digest(second.as_bytes())))
}
fn fields(values: &[(&str, &str)]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}
fn scopes(value: &str) -> Vec<String> {
    value.split_ascii_whitespace().map(str::to_string).collect()
}
fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

struct Cancellation {
    cancelled: AtomicBool,
    notification: Notify,
}
impl Cancellation {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            notification: Notify::new(),
        }
    }
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notification.notify_one();
    }
    fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::SeqCst) {
            Err("ChatGPT 登录已取消。".into())
        } else {
            Ok(())
        }
    }
    async fn wait(&self) {
        while !self.cancelled.load(Ordering::SeqCst) {
            self.notification.notified().await;
        }
    }
}
fn pending_sign_in() -> &'static Mutex<Option<(Uuid, Arc<Cancellation>)>> {
    static PENDING: OnceLock<Mutex<Option<(Uuid, Arc<Cancellation>)>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}
struct AttemptGuard {
    id: Uuid,
    cancellation: Arc<Cancellation>,
}
impl AttemptGuard {
    fn begin() -> Result<Self, String> {
        let mut pending = pending_sign_in()
            .lock()
            .map_err(|_| "无法启动 ChatGPT 登录。".to_string())?;
        if pending.is_some() {
            return Err("ChatGPT 登录正在进行，请先完成或取消。".into());
        }
        let guard = Self {
            id: Uuid::new_v4(),
            cancellation: Arc::new(Cancellation::new()),
        };
        *pending = Some((guard.id, guard.cancellation.clone()));
        Ok(guard)
    }
}
impl Drop for AttemptGuard {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Ok(mut pending) = pending_sign_in().lock() {
            if pending.as_ref().is_some_and(|(id, _)| *id == self.id) {
                *pending = None;
            }
        }
    }
}

pub fn cancel_sign_in() -> Result<(), String> {
    if let Some((_, cancellation)) = pending_sign_in()
        .lock()
        .map_err(|_| "无法取消 ChatGPT 登录。".to_string())?
        .as_ref()
    {
        cancellation.cancel();
    }
    Ok(())
}

struct AuthorizationAttempt {
    state: CsrfToken,
    nonce: CsrfToken,
    verifier: PkceCodeVerifier,
    challenge: PkceCodeChallenge,
    redirect_uri: String,
    client_id: Option<String>,
    consumed: bool,
    expires_at: std::time::Instant,
}
impl AuthorizationAttempt {
    fn new(port: u16, client_id: Option<String>) -> Self {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        Self {
            state: CsrfToken::new_random(),
            nonce: CsrfToken::new_random(),
            verifier,
            challenge,
            redirect_uri: format!("http://127.0.0.1:{port}/auth/callback"),
            client_id,
            consumed: false,
            expires_at: std::time::Instant::now() + SIGN_IN_TIMEOUT,
        }
    }
    fn authorization_url(&self, record: &Record) -> String {
        let mut url = reqwest::Url::parse(AUTHORIZE).expect("static OAuth endpoint");
        let mut parameters = url.query_pairs_mut();
        parameters.extend_pairs([
            (
                "client_id",
                self.client_id.as_deref().unwrap_or(DYNAMIC_CLIENT),
            ),
            ("ext_agent_host_id", record.host_id.as_str()),
            ("response_type", "code"),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("scope", SCOPE),
            ("resource", RESOURCE),
            ("state", self.state.secret()),
            ("nonce", self.nonce.secret()),
            ("code_challenge_method", "S256"),
            ("code_challenge", self.challenge.as_str()),
        ]);
        if self.client_id.is_none() {
            parameters.append_pair("agent_name_hint", "AngelBot");
        }
        if let Some(registration) = record.registration.as_ref() {
            if let Some(tokens) = &registration.tokens {
                parameters.append_pair("id_token_hint", &tokens.id_token);
            }
            if let Some(email) = &registration.email {
                parameters.append_pair("login_hint", email);
            }
            if registration
                .tokens
                .as_ref()
                .is_some_and(|tokens| !tokens.scopes.iter().any(|scope| scope == DIRECT_SCOPE))
            {
                // Explicit settings action to enable the previously declined scope.
                parameters.append_pair("prompt", "consent");
            }
        }
        drop(parameters);
        url.into()
    }
}

struct Callback {
    code: String,
    client_id: String,
}
fn parse_callback(
    target: &str,
    attempt: &mut AuthorizationAttempt,
) -> Result<Option<Callback>, String> {
    if attempt.consumed {
        return Err("ChatGPT 登录回调已使用。".into());
    }
    if std::time::Instant::now() >= attempt.expires_at {
        attempt.consumed = true;
        return Err("ChatGPT 登录已超时，请重试。".into());
    }
    if target.len() > 8192 || !target.starts_with("/auth/callback?") {
        return Ok(None);
    }
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| "ChatGPT 回调无效。".to_string())?;
    if url.path() != "/auth/callback" || url.fragment().is_some() {
        return Ok(None);
    }
    let mut parameters = std::collections::HashMap::new();
    for (key, value) in url.query_pairs() {
        if parameters
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Ok(None);
        }
    }
    if parameters
        .get("state")
        .is_none_or(|state| !secret_equal(state, attempt.state.secret()))
    {
        return Ok(None);
    }
    attempt.consumed = true;
    if let Some(error) = parameters.get("error") {
        return Err(if error == "access_denied" {
            "ChatGPT 授权已取消。"
        } else {
            "ChatGPT 授权失败，请重新登录。"
        }
        .into());
    }
    let client_id = match (&attempt.client_id, parameters.remove("client_id")) {
        (Some(expected), Some(returned)) if expected != &returned => {
            return Err("ChatGPT 回调的客户端身份不匹配。".into())
        }
        (Some(expected), _) => expected.clone(),
        (None, Some(returned)) if valid_client_id(&returned) => returned,
        _ => return Err("ChatGPT 注册尚未完成，缺少已签发的客户端 ID。".into()),
    };
    let code = parameters
        .remove("code")
        .filter(|code| !code.is_empty())
        .ok_or("ChatGPT 回调缺少授权码。")?;
    Ok(Some(Callback { code, client_id }))
}

async fn listen_for_callback(
    server: tiny_http::Server,
    mut attempt: AuthorizationAttempt,
    cancellation: Arc<Cancellation>,
) -> Result<(AuthorizationAttempt, Callback), String> {
    tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + SIGN_IN_TIMEOUT;
        loop {
            cancellation.check()?;
            if std::time::Instant::now() >= deadline {
                return Err("ChatGPT 登录已超时，请重试。".into());
            }
            let Some(request) = server
                .recv_timeout(Duration::from_millis(200))
                .map_err(|_| "无法读取 ChatGPT 本地回调。".to_string())?
            else {
                continue;
            };
            let result = if request.method() == &tiny_http::Method::Get {
                parse_callback(request.url(), &mut attempt)
            } else {
                Ok(None)
            };
            let accepted = !matches!(result, Ok(None));
            let message = if accepted {
                "ChatGPT authorization received. Return to AngelBot to check whether the account connection completed. You may close this window."
            } else {
                "Unknown authorization callback."
            };
            let response = tiny_http::Response::from_string(message)
                .with_status_code(if accepted { 200 } else { 400 })
                .with_header(
                    tiny_http::Header::from_bytes("Cache-Control", "no-store")
                        .expect("static header"),
                )
                .with_header(
                    tiny_http::Header::from_bytes(
                        "Content-Security-Policy",
                        "default-src 'none'; frame-ancestors 'none'",
                    )
                    .expect("static header"),
                );
            let _ = request.respond(response);
            match result {
                Ok(Some(callback)) => return Ok((attempt, callback)),
                Err(error) => return Err(error),
                Ok(None) => {}
            }
        }
    })
    .await
    .map_err(|_| "ChatGPT 本地回调已停止。".to_string())?
}

struct Session {
    store: Arc<dyn CredentialStore>,
    transport: Arc<dyn AuthTransport>,
    gate: Arc<AsyncMutex<()>>,
    lock_path: Option<PathBuf>,
}

#[derive(Clone, Copy)]
enum AccountMode {
    Reauthorize,
    ChangeAccount,
}
impl AccountMode {
    fn selected_record(self, original: &Record) -> Record {
        let mut selected = original.clone();
        if matches!(self, Self::ChangeAccount) {
            selected.registration = None;
        }
        selected
    }
}
impl Session {
    fn production(app: &tauri::AppHandle) -> Result<Self, String> {
        Ok(Self {
            store: Arc::new(ProtectedStore::new(app)),
            transport: Arc::new(OfficialTransport::new()?),
            gate: process_gate(),
            lock_path: Some(crate::app_data_dir().join("chatgpt-plan.lock")),
        })
    }

    async fn initialize(&self) -> Result<Record, String> {
        let _gate = self.gate.lock().await;
        let _process = cross_process_lock(self.lock_path.clone()).await?;
        if let Some(mut record) = self.store.load()? {
            // A crashed sign-out must remain unusable. Only an explicit new
            // sign-in may finish local cleanup and begin another session.
            if record.disconnecting {
                if let Some(registration) = &mut record.registration {
                    registration.tokens = None;
                }
                record.disconnecting = false;
                record.authorization_epoch = record
                    .authorization_epoch
                    .checked_add(1)
                    .ok_or(STORE_ERROR)?;
                self.store.save(&record)?;
            }
            return Ok(record);
        }
        let record = Record::new();
        self.store.save(&record)?;
        Ok(record)
    }

    #[cfg(test)]
    async fn save_sign_in(
        &self,
        original: &Record,
        attempt: &AuthorizationAttempt,
        callback: &Callback,
        response: TokenResponse,
        jwks: &JwkSet,
        cancellation: &Cancellation,
    ) -> Result<ChatGptPlanStatus, String> {
        self.save_sign_in_with_mode(
            original,
            attempt,
            callback,
            response,
            jwks,
            cancellation,
            AccountMode::Reauthorize,
        )
        .await
    }

    async fn save_sign_in_with_mode(
        &self,
        original: &Record,
        attempt: &AuthorizationAttempt,
        callback: &Callback,
        response: TokenResponse,
        jwks: &JwkSet,
        cancellation: &Cancellation,
        mode: AccountMode,
    ) -> Result<ChatGptPlanStatus, String> {
        let id_token = nonempty(response.id_token).ok_or("OpenAI 未返回可验证的身份凭据。")?;
        let identity = verify_identity(
            &id_token,
            jwks,
            &callback.client_id,
            Some(attempt.nonce.secret()),
        )?;
        if matches!(mode, AccountMode::Reauthorize)
            && original.registration.as_ref().is_some_and(|registration| {
                registration.client_id != callback.client_id || registration.subject != identity.sub
            })
        {
            return Err("返回的 ChatGPT 账号与当前注册不匹配，已保留原连接。".into());
        }
        let granted = scopes(response.scope.as_deref().unwrap_or_default());
        let access_token = nonempty(response.access_token);
        let expires_in = response.expires_in.unwrap_or(0);
        if access_token.is_some()
            && (response
                .token_type
                .as_deref()
                .is_none_or(|kind| !kind.eq_ignore_ascii_case("Bearer"))
                || expires_in == 0)
        {
            return Err("OpenAI 返回的访问凭据格式无效。".into());
        }
        let _gate = self.gate.lock().await;
        let _process = cross_process_lock(self.lock_path.clone()).await?;
        cancellation.check()?;
        let current = self.store.load()?.ok_or(STORE_ERROR)?;
        // A concurrent disconnect/account mutation must not revive stale state.
        if current.authorization_epoch != original.authorization_epoch
            || current
                .registration
                .as_ref()
                .map(|r| (&r.credential_ref, &r.subject))
                != original
                    .registration
                    .as_ref()
                    .map(|r| (&r.credential_ref, &r.subject))
        {
            return Err("ChatGPT 连接已更改，请重新登录。".into());
        }
        let registration = Registration {
            credential_ref: matches!(mode, AccountMode::Reauthorize)
                .then_some(original)
                .and_then(|record| record.registration.as_ref())
                .map(|registration| registration.credential_ref.clone())
                .unwrap_or_else(|| format!("{REFERENCE_PREFIX}{}", Uuid::new_v4())),
            client_id: callback.client_id.clone(),
            subject: identity.sub,
            email: identity.email,
            tokens: Some(Tokens {
                access_token,
                refresh_token: nonempty(response.refresh_token),
                id_token,
                expires_at: now().saturating_add(expires_in),
                scopes: granted,
                nonce: attempt.nonce.secret().clone(),
            }),
        };
        let record = Record {
            registration: Some(registration),
            authorization_epoch: current
                .authorization_epoch
                .checked_add(1)
                .ok_or("ChatGPT 连接版本无效，请联系支持。")?,
            disconnecting: false,
            ..current
        };
        self.store.save(&record)?;
        Ok(record.status())
    }

    async fn refresh_with_backoff(
        &self,
        parameters: Vec<(String, String)>,
    ) -> Result<TokenResponse, AuthError> {
        // A transport interruption has unknown delivery status. Do not replay
        // rotating refresh tokens; only an explicit availability failure retries.
        for retry in 0..3 {
            match self.transport.token(parameters.clone()).await {
                Err(AuthError::Network) if retry < 2 => {
                    tokio::time::sleep(Duration::from_millis(150 * (1 << retry))).await;
                }
                response => return response,
            }
        }
        Err(AuthError::Network)
    }

    #[cfg(test)]
    async fn bearer(&self, credential_ref: &str) -> Result<String, String> {
        self.bearer_at_epoch(credential_ref, None).await
    }

    fn validate_source(
        &self,
        credential_ref: &str,
        authorization_epoch: u64,
    ) -> Result<(), String> {
        validate_reference(credential_ref)?;
        let record = self.store.load()?.ok_or(SIGN_IN_REQUIRED)?;
        if record.disconnecting || record.authorization_epoch != authorization_epoch {
            return Err(SIGN_IN_REQUIRED.into());
        }
        let tokens = record
            .registration
            .as_ref()
            .filter(|registration| registration.credential_ref == credential_ref)
            .and_then(|registration| registration.tokens.as_ref())
            .ok_or(SIGN_IN_REQUIRED)?;
        if tokens.access_token.is_none() || !tokens.scopes.iter().any(|scope| scope == DIRECT_SCOPE)
        {
            return Err("ChatGPT 套餐授权已失效，请重新授权。".into());
        }
        Ok(())
    }

    async fn bearer_at_epoch(
        &self,
        credential_ref: &str,
        authorization_epoch: Option<u64>,
    ) -> Result<String, String> {
        validate_reference(credential_ref)?;
        let _gate = self.gate.lock().await;
        let _process = cross_process_lock(self.lock_path.clone()).await?;
        let mut record = self.store.load()?.ok_or(SIGN_IN_REQUIRED)?;
        if record.disconnecting
            || authorization_epoch.is_some_and(|epoch| epoch != record.authorization_epoch)
        {
            return Err(SIGN_IN_REQUIRED.into());
        }
        let registration = record
            .registration
            .as_mut()
            .filter(|registration| registration.credential_ref == credential_ref)
            .ok_or(SIGN_IN_REQUIRED)?;
        let tokens = registration
            .tokens
            .as_ref()
            .ok_or(SIGN_IN_REQUIRED)?
            .clone();
        if !tokens.scopes.iter().any(|scope| scope == DIRECT_SCOPE) {
            return Err("此 ChatGPT 连接尚未授权套餐用量，请在设置中授权。".into());
        }
        if tokens.expires_at > now().saturating_add(60) {
            return tokens
                .access_token
                .ok_or_else(|| "ChatGPT 套餐访问凭据缺失，请重新授权。".into());
        }
        let refresh_token = tokens.refresh_token.as_deref().ok_or(SIGN_IN_REQUIRED)?;
        let response = match self
            .refresh_with_backoff(fields(&[
                ("grant_type", "refresh_token"),
                ("client_id", &registration.client_id),
                ("refresh_token", refresh_token),
                ("resource", RESOURCE),
            ]))
            .await
        {
            Ok(response) => response,
            Err(error) => {
                if error.terminal_refresh() {
                    registration.tokens = None;
                    self.store.save(&record)?;
                    return Err(SIGN_IN_REQUIRED.into());
                }
                return Err(error.message_at("刷新套餐授权"));
            }
        };
        let new_access = nonempty(response.access_token).ok_or("OpenAI 刷新响应缺少访问凭据。")?;
        let new_refresh =
            nonempty(response.refresh_token).ok_or("OpenAI 刷新响应缺少轮换凭据，请重新登录。")?;
        let expires_in = response
            .expires_in
            .filter(|seconds| *seconds > 0)
            .ok_or("OpenAI 刷新响应缺少有效期。")?;
        if response
            .token_type
            .as_deref()
            .is_none_or(|kind| !kind.eq_ignore_ascii_case("Bearer"))
        {
            return Err("OpenAI 刷新响应类型无效。".into());
        }
        let new_scopes = response
            .scope
            .as_deref()
            .map(scopes)
            .unwrap_or_else(|| tokens.scopes.clone());
        let id_token = if let Some(jwt) = nonempty(response.id_token) {
            let discovery = self
                .transport
                .discovery()
                .await
                .map_err(|error| error.message_at("读取认证元数据"))?;
            discovery.validate()?;
            let jwks = self
                .transport
                .jwks(&discovery.jwks_uri)
                .await
                .map_err(|error| error.message_at("读取签名密钥"))?;
            let identity = verify_identity(&jwt, &jwks, &registration.client_id, None)?;
            if identity.sub != registration.subject
                || identity
                    .nonce
                    .as_deref()
                    .is_some_and(|nonce| nonce != tokens.nonce)
            {
                return Err("刷新后的 ChatGPT 身份不匹配，已停止请求。".into());
            }
            jwt
        } else {
            tokens.id_token
        };
        registration.tokens = Some(Tokens {
            access_token: Some(new_access.clone()),
            refresh_token: Some(new_refresh),
            id_token,
            expires_at: now().saturating_add(expires_in),
            scopes: new_scopes.clone(),
            nonce: tokens.nonce,
        });
        // Persist replacement before returning it; failed durable writes cannot
        // silently use a rotating token that a later process cannot recover.
        self.store.save(&record)?;
        if !new_scopes.iter().any(|scope| scope == DIRECT_SCOPE) {
            return Err("ChatGPT 套餐授权已被移除，请重新授权。".into());
        }
        Ok(new_access)
    }

    async fn disconnect(&self) -> Result<ChatGptPlanStatus, String> {
        let _gate = self.gate.lock().await;
        let _process = cross_process_lock(self.lock_path.clone()).await?;
        let Some(mut record) = self.store.load()? else {
            return Ok(Record::new().status());
        };
        // Invalidate sources before remote I/O. Retain secrets under the
        // protected record solely for bounded revocation/recovery, never for
        // inference. A crash preserves this fail-closed marker.
        record.authorization_epoch = record
            .authorization_epoch
            .checked_add(1)
            .ok_or(STORE_ERROR)?;
        record.disconnecting = true;
        self.store.save(&record)?;
        let mut remote_confirmed = true;
        if let Some(registration) = &record.registration {
            if let Some(refresh_token) = registration
                .tokens
                .as_ref()
                .and_then(|tokens| tokens.refresh_token.as_deref())
            {
                remote_confirmed = false;
                if let Ok(discovery) = self.transport.discovery().await {
                    if discovery.validate().is_ok() {
                        if let Some(endpoint) = discovery.revocation_endpoint {
                            for attempt in 0..3 {
                                match self
                                    .transport
                                    .revoke(&endpoint, &registration.client_id, refresh_token)
                                    .await
                                {
                                    Ok(()) => {
                                        remote_confirmed = true;
                                        break;
                                    }
                                    Err(AuthError::Network) if attempt < 2 => {
                                        tokio::time::sleep(Duration::from_millis(
                                            150 * (1 << attempt),
                                        ))
                                        .await
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(registration) = &mut record.registration {
            registration.tokens = None;
        }
        record.disconnecting = false;
        // Retain the issued client/verified identity mapping and stable host ID.
        self.store.save(&record)?;
        if !remote_confirmed {
            return Err(
                "已清除本机 ChatGPT 凭据，但未确认远端撤销。请在 ChatGPT 设置中断开 AngelBot。"
                    .into(),
            );
        }
        Ok(record.status())
    }
}

/// Starts the browser flow only after an explicit user settings action.
pub async fn sign_in(app: &tauri::AppHandle) -> Result<ChatGptPlanStatus, String> {
    sign_in_with_mode(app, AccountMode::Reauthorize).await
}

/// Explicit settings action: authorize another account independently and only
/// replace the active connection after validation and protected persistence.
/// There remains one active registration in V1; old references are never reused.
pub async fn change_account(app: &tauri::AppHandle) -> Result<ChatGptPlanStatus, String> {
    sign_in_with_mode(app, AccountMode::ChangeAccount).await
}

async fn sign_in_with_mode(
    app: &tauri::AppHandle,
    mode: AccountMode,
) -> Result<ChatGptPlanStatus, String> {
    #[cfg(feature = "desktop-e2e")]
    {
        let _ = (app, mode);
        return Err("自动化测试不启动真实 ChatGPT 登录。".into());
    }
    #[cfg(not(feature = "desktop-e2e"))]
    {
        let guard = AttemptGuard::begin()?;
        let session = Session::production(app)?;
        let flow = async {
            let original = session.initialize().await?;
            let selected = mode.selected_record(&original);
            let discovery = session
                .transport
                .discovery()
                .await
                .map_err(|error| error.message_at("读取认证元数据"))?;
            discovery.validate()?;
            let mut client_id = selected
                .registration
                .as_ref()
                .map(|registration| registration.client_id.clone());
            // One fresh attempt after an expired authorization code. Never
            // reuse its state, nonce, verifier, code, or callback listener.
            for retry in 0..2 {
                guard.cancellation.check()?;
                let server = tiny_http::Server::http(("127.0.0.1", 0))
                    .map_err(|_| "无法启动 ChatGPT 本地登录回调。".to_string())?;
                let port = server
                    .server_addr()
                    .to_ip()
                    .ok_or("无法获取 ChatGPT 回调端口。")?
                    .port();
                let attempt = AuthorizationAttempt::new(port, client_id.clone());
                #[allow(deprecated)]
                app.shell()
                    .open(attempt.authorization_url(&selected), None)
                    .map_err(|_| "无法打开系统浏览器，请检查默认浏览器。".to_string())?;
                let (attempt, callback) =
                    listen_for_callback(server, attempt, guard.cancellation.clone()).await?;
                client_id = Some(callback.client_id.clone());
                let response = match session
                    .transport
                    .token(fields(&[
                        ("grant_type", "authorization_code"),
                        ("client_id", &callback.client_id),
                        ("code", &callback.code),
                        ("code_verifier", attempt.verifier.secret()),
                        ("redirect_uri", &attempt.redirect_uri),
                        ("resource", RESOURCE),
                    ]))
                    .await
                {
                    Err(AuthError::OAuth(code)) if code == "invalid_grant" && retry == 0 => {
                        continue
                    }
                    Err(error) => return Err(error.message_at("交换登录授权码")),
                    Ok(response) => response,
                };
                let jwks = session
                    .transport
                    .jwks(&discovery.jwks_uri)
                    .await
                    .map_err(|error| error.message_at("读取签名密钥"))?;
                return session
                    .save_sign_in_with_mode(
                        &original,
                        &attempt,
                        &callback,
                        response,
                        &jwks,
                        &guard.cancellation,
                        mode,
                    )
                    .await;
            }
            Err("ChatGPT 授权码已失效，请重新登录。".into())
        };
        tokio::select! {
            result = tokio::time::timeout(SIGN_IN_TIMEOUT, flow) => result.map_err(|_| "ChatGPT 登录已超时，请重试。".to_string())?,
            _ = guard.cancellation.wait() => Err("ChatGPT 登录已取消。".into()),
        }
    }
}

pub fn status(app: &tauri::AppHandle) -> Result<ChatGptPlanStatus, String> {
    #[cfg(feature = "desktop-e2e")]
    {
        let _ = app;
        return Ok(Record::new().status());
    }
    #[cfg(not(feature = "desktop-e2e"))]
    Ok(ProtectedStore::new(app)
        .load()?
        .unwrap_or_else(Record::new)
        .status())
}

pub async fn disconnect(app: &tauri::AppHandle) -> Result<ChatGptPlanStatus, String> {
    cancel_sign_in()?;
    #[cfg(feature = "desktop-e2e")]
    {
        let _ = app;
        return Ok(Record::new().status());
    }
    #[cfg(not(feature = "desktop-e2e"))]
    Session::production(app)?.disconnect().await
}

struct PlanCredentialSource {
    session: Session,
    credential_ref: String,
    authorization_epoch: u64,
}
#[async_trait]
impl ModelCredentialSource for PlanCredentialSource {
    async fn bearer_token(&self) -> Result<String, String> {
        self.session
            .bearer_at_epoch(&self.credential_ref, Some(self.authorization_epoch))
            .await
    }
    fn credential_ref(&self) -> &str {
        &self.credential_ref
    }
    fn validate_session(&self) -> Result<(), String> {
        self.session
            .validate_source(&self.credential_ref, self.authorization_epoch)
    }
}

pub fn credential_source(
    app: &tauri::AppHandle,
    opaque_ref: &str,
) -> Result<Arc<dyn ModelCredentialSource>, String> {
    validate_reference(opaque_ref)?;
    #[cfg(feature = "desktop-e2e")]
    {
        let _ = app;
        return Err("自动化测试不读取真实 ChatGPT 凭据。".into());
    }
    #[cfg(not(feature = "desktop-e2e"))]
    {
        let session = Session::production(app)?;
        let record = session.store.load()?.ok_or(SIGN_IN_REQUIRED)?;
        if record
            .registration
            .as_ref()
            .is_none_or(|registration| registration.credential_ref != opaque_ref)
        {
            return Err(SIGN_IN_REQUIRED.into());
        }
        session.validate_source(opaque_ref, record.authorization_epoch)?;
        Ok(Arc::new(PlanCredentialSource {
            session,
            credential_ref: opaque_ref.to_string(),
            authorization_epoch: record.authorization_epoch,
        }))
    }
}

#[cfg(test)]
#[path = "chatgpt_auth_tests.rs"]
mod tests;
