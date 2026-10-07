use super::*;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;

fn auth_http_fixture(
    response: Option<&'static [u8]>,
) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return,
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request);
        if let Some(response) = response {
            let _ = stream.write_all(response);
        } else {
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    (address, server)
}

#[tokio::test]
async fn authentication_timeout_is_distinct_and_keeps_request_url_private() {
    let (address, server) = auth_http_fixture(None);
    let error = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap()
        .get(format!("http://{address}/fixture-private-url"))
        .send()
        .await
        .err()
        .unwrap();
    server.join().unwrap();
    let error = AuthError::transport(error);
    assert!(matches!(
        error,
        AuthError::Transport(TransportFailure::Timeout)
    ));
    let message = error.message_at("交换登录授权码");
    assert!(message.contains("请求超时"));
    assert!(!message.contains("fixture-private"));
    assert!(!message.contains(&address.to_string()));
    assert!(!error.terminal_refresh());
}

#[tokio::test]
async fn authentication_connection_failure_keeps_request_url_private() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let error = reqwest::Client::builder()
        .no_proxy()
        // Windows can take roughly two seconds to report connection refusal.
        // Keep this distinct from the explicit timeout fixture above.
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
        .get(format!("http://{address}/fixture-private-url"))
        .send()
        .await
        .err()
        .unwrap();
    let error = AuthError::transport(error);
    assert!(matches!(
        error,
        AuthError::Transport(TransportFailure::Connect)
    ));
    let message = error.message_at("交换登录授权码");
    assert!(message.contains("连接建立失败"));
    assert!(!message.contains("fixture-private"));
    assert!(!message.contains(&address.to_string()));
    assert!(!error.terminal_refresh());
}

#[tokio::test]
async fn truncated_authentication_response_reports_read_failure_without_private_body() {
    let (address, server) = auth_http_fixture(Some(b"HTTP/1.1 200 OK\r\nContent-Length: 200\r\nConnection: close\r\n\r\nfixture-private-response"));
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
        .get(format!("http://{address}/fixture-private-url"))
        .send()
        .await
        .unwrap();
    let error = read_response::<TokenResponse>(response)
        .await
        .err()
        .unwrap();
    server.join().unwrap();
    let message = error.message_at("交换登录授权码");
    assert!(
        message.contains("响应读取中断"),
        "missing transport detail: {message}"
    );
    assert!(!message.contains("fixture-private"));
    assert!(!message.contains(&address.to_string()));
    assert!(!error.terminal_refresh());
}

#[test]
fn discovery_http_denial_reports_status_without_response_body() {
    let body = b"<html>access denied; fixture-private-response</html>";
    let error = decode_auth_response::<Discovery>(reqwest::StatusCode::FORBIDDEN, body)
        .err()
        .expect("a denied discovery response must fail closed");
    let message = error.message_at("读取认证元数据");
    assert!(
        message.contains("HTTP 403"),
        "missing actionable HTTP status: {message}"
    );
    assert!(message.contains("读取认证元数据失败"));
    assert!(!message.contains("fixture-private-response"));
    assert!(!message.contains("<html>"));
    assert!(!error.terminal_refresh());
}

#[test]
fn authentication_http_failures_preserve_safe_status_and_do_not_revoke_tokens() {
    for status in [302, 401, 429, 502, 503] {
        let error = decode_auth_response::<TokenResponse>(
            reqwest::StatusCode::from_u16(status).unwrap(),
            b"<html>fixture-private-response</html>",
        )
        .err()
        .unwrap();
        let message = error.message_at("交换登录授权码");
        assert!(message.contains(&format!("HTTP {status}")));
        assert!(!message.contains("fixture-private-response"));
        assert!(!error.terminal_refresh());
    }
}

#[test]
fn authentication_oauth_terminal_codes_remain_machine_readable() {
    let error = decode_auth_response::<TokenResponse>(
        reqwest::StatusCode::BAD_REQUEST,
        br#"{"error":"invalid_grant","error_description":"fixture-private-response"}"#,
    )
    .err()
    .unwrap();
    assert!(error.terminal_refresh());
    assert!(!error
        .message_at("交换登录授权码")
        .contains("fixture-private-response"));
}

#[test]
fn authentication_success_status_does_not_accept_invalid_metadata() {
    for body in [
        b"<html>fixture-private-response</html>".as_slice(),
        b"{}".as_slice(),
    ] {
        let error = decode_auth_response::<Discovery>(reqwest::StatusCode::OK, body)
            .err()
            .expect("success status must not bypass metadata validation");
        let message = error.message_at("读取认证元数据");
        assert!(message.contains("读取认证元数据失败"));
        assert!(!message.contains("fixture-private-response"));
    }
    let body = serde_json::to_vec(&json!({
        "issuer": ISSUER,
        "authorization_endpoint": AUTHORIZE,
        "token_endpoint": TOKEN,
        "jwks_uri": "https://auth.openai.com/.well-known/jwks.json",
    }))
    .unwrap();
    assert!(
        decode_auth_response::<Discovery>(reqwest::StatusCode::OK, &body)
            .ok()
            .unwrap()
            .validate()
            .is_ok()
    );
}

// RFC 8032 test-vector seed/public key. Public, deterministic fixture material,
// not an account token or user credential. jsonwebtoken performs real EdDSA.
const FIXTURE_PKCS8: &str = "MC4CAQAwBQYDK2VwBCIEIJ1hsZ3v/VpguoRK9JLsLMREScVpezJpGXA7rAMcrn9g";
fn fixture_jwks() -> JwkSet {
    serde_json::from_value(json!({"keys":[{"kty":"OKP","crv":"Ed25519","use":"sig","alg":"EdDSA","kid":"fixture-key","x":"11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo"}]})).unwrap()
}
fn identity_jwt(subject: &str, audience: &str, nonce: &str, expiry: u64) -> String {
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some("fixture-key".into());
    encode(&header, &json!({"iss":ISSUER,"sub":subject,"aud":audience,"iat":now()-1,"exp":expiry,"nonce":nonce,"email":"fixture@example.invalid"}),
        &EncodingKey::from_ed_der(&STANDARD.decode(FIXTURE_PKCS8).unwrap())).unwrap()
}

#[derive(Default)]
struct MemoryStore {
    record: Mutex<Option<Record>>,
    fail_save: AtomicBool,
}
impl CredentialStore for MemoryStore {
    fn load(&self) -> Result<Option<Record>, String> {
        Ok(self.record.lock().unwrap().clone())
    }
    fn save(&self, record: &Record) -> Result<(), String> {
        if self.fail_save.load(Ordering::SeqCst) {
            return Err(STORE_ERROR.into());
        }
        record.validate()?;
        *self.record.lock().unwrap() = Some(record.clone());
        Ok(())
    }
}

struct FakeTransport {
    responses: Mutex<VecDeque<Result<TokenResponse, AuthError>>>,
    requests: Mutex<Vec<Vec<(String, String)>>>,
    refresh_count: AtomicUsize,
    revoke_count: AtomicUsize,
    fail_revoke: AtomicBool,
    revoke_requests: Mutex<Vec<(String, String)>>,
    pause_revoke: AtomicBool,
    revoke_started: Notify,
    revoke_release: Notify,
}
impl FakeTransport {
    fn new(responses: Vec<Result<TokenResponse, AuthError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
            refresh_count: AtomicUsize::new(0),
            revoke_count: AtomicUsize::new(0),
            fail_revoke: AtomicBool::new(false),
            revoke_requests: Mutex::new(Vec::new()),
            pause_revoke: AtomicBool::new(false),
            revoke_started: Notify::new(),
            revoke_release: Notify::new(),
        }
    }
}
#[async_trait]
impl AuthTransport for FakeTransport {
    async fn discovery(&self) -> Result<Discovery, AuthError> {
        Ok(Discovery {
            issuer: ISSUER.into(),
            authorization_endpoint: AUTHORIZE.into(),
            token_endpoint: TOKEN.into(),
            jwks_uri: "https://auth.openai.com/.well-known/jwks.json".into(),
            revocation_endpoint: Some("https://auth.openai.com/oauth/revoke".into()),
        })
    }
    async fn jwks(&self, _: &str) -> Result<JwkSet, AuthError> {
        Ok(fixture_jwks())
    }
    async fn token(&self, parameters: Vec<(String, String)>) -> Result<TokenResponse, AuthError> {
        self.refresh_count.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(parameters);
        tokio::task::yield_now().await;
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(AuthError::Protocol))
    }
    async fn revoke(&self, _: &str, client_id: &str, refresh_token: &str) -> Result<(), AuthError> {
        self.revoke_count.fetch_add(1, Ordering::SeqCst);
        self.revoke_requests
            .lock()
            .unwrap()
            .push((client_id.to_string(), refresh_token.to_string()));
        self.revoke_started.notify_one();
        if self.pause_revoke.load(Ordering::SeqCst) {
            self.revoke_release.notified().await;
        }
        if self.fail_revoke.load(Ordering::SeqCst) {
            Err(AuthError::Protocol)
        } else {
            Ok(())
        }
    }
}
fn response(access: &str, refresh: &str) -> TokenResponse {
    TokenResponse {
        access_token: Some(access.into()),
        refresh_token: Some(refresh.into()),
        id_token: None,
        token_type: Some("Bearer".into()),
        expires_in: Some(3600),
        scope: None,
    }
}
fn registration() -> Registration {
    Registration {
        credential_ref: format!("{REFERENCE_PREFIX}00000000-0000-4000-8000-000000000001"),
        client_id: "oaiapp_fixture".into(),
        subject: "fixture-subject".into(),
        email: Some("fixture@example.invalid".into()),
        tokens: Some(Tokens {
            access_token: Some("fixture-access-old".into()),
            refresh_token: Some("fixture-refresh-old".into()),
            id_token: "fixture-retained-id".into(),
            expires_at: 1,
            scopes: scopes(SCOPE),
            nonce: "fixture-nonce".into(),
        }),
    }
}
fn session_with(
    responses: Vec<Result<TokenResponse, AuthError>>,
) -> (Arc<Session>, Arc<MemoryStore>, Arc<FakeTransport>, String) {
    let registration = registration();
    let reference = registration.credential_ref.clone();
    let store = Arc::new(MemoryStore {
        record: Mutex::new(Some(Record {
            version: 1,
            host_id: "urn:uuid:00000000-0000-4000-8000-000000000002".into(),
            registration: Some(registration),
            authorization_epoch: 0,
            disconnecting: false,
        })),
        ..Default::default()
    });
    let transport = Arc::new(FakeTransport::new(responses));
    (
        Arc::new(Session {
            store: store.clone(),
            transport: transport.clone(),
            gate: Arc::new(AsyncMutex::new(())),
            lock_path: None,
        }),
        store,
        transport,
        reference,
    )
}

fn source_at_epoch(
    session: &Session,
    reference: &str,
    authorization_epoch: u64,
) -> PlanCredentialSource {
    PlanCredentialSource {
        session: Session {
            store: session.store.clone(),
            transport: session.transport.clone(),
            gate: session.gate.clone(),
            lock_path: session.lock_path.clone(),
        },
        credential_ref: reference.to_string(),
        authorization_epoch,
    }
}

#[test]
fn pkce_uses_rfc7636_s256_and_fresh_attempt_values() {
    let verifier = PkceCodeVerifier::new("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
    assert_eq!(
        PkceCodeChallenge::from_code_verifier_sha256(&verifier).as_str(),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    let first = AuthorizationAttempt::new(54321, None);
    let second = AuthorizationAttempt::new(54322, None);
    assert_ne!(first.state.secret(), second.state.secret());
    assert_ne!(first.nonce.secret(), second.nonce.secret());
    assert_ne!(first.verifier.secret(), second.verifier.secret());
    let url = reqwest::Url::parse(&first.authorization_url(&Record::new())).unwrap();
    let fields: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(fields["client_id"], DYNAMIC_CLIENT);
    assert_eq!(fields["agent_name_hint"], "AngelBot");
    assert_eq!(
        fields["redirect_uri"],
        "http://127.0.0.1:54321/auth/callback"
    );
    assert_eq!(fields["resource"], RESOURCE);
    assert_eq!(fields["code_challenge_method"], "S256");
}

#[test]
fn callback_requires_state_issued_id_and_bound_returning_registration() {
    let mut attempt = AuthorizationAttempt::new(54321, None);
    assert!(parse_callback(
        "/auth/callback?state=forged&code=fixture&client_id=oaiapp_fixture",
        &mut attempt
    )
    .unwrap()
    .is_none());
    let state = attempt.state.secret().clone();
    assert!(parse_callback(
        &format!("/callback?state={state}&code=fixture&client_id=oaiapp_fixture"),
        &mut attempt
    )
    .unwrap()
    .is_none());
    assert!(parse_callback(
        &format!(
            "/auth/callback?state={state}&state={state}&code=fixture&client_id=oaiapp_fixture"
        ),
        &mut attempt
    )
    .unwrap()
    .is_none());
    let callback = parse_callback(
        &format!("/auth/callback?state={state}&code=fixture&client_id=oaiapp_fixture"),
        &mut attempt,
    )
    .unwrap()
    .unwrap();
    assert_eq!(callback.client_id, "oaiapp_fixture");
    assert!(parse_callback(
        &format!("/auth/callback?state={state}&code=fixture&client_id=oaiapp_fixture"),
        &mut attempt
    )
    .is_err());
    for suffix in [
        "code=fixture",
        "code=fixture&client_id=dynamic_agent_client",
        "error=access_denied",
    ] {
        let mut invalid = AuthorizationAttempt::new(54321, None);
        let target = format!("/auth/callback?state={}&{suffix}", invalid.state.secret());
        assert!(parse_callback(&target, &mut invalid).is_err());
        assert!(invalid.consumed);
    }
    let mut returning = AuthorizationAttempt::new(54321, Some("oaiapp_saved".into()));
    let mismatched = format!(
        "/auth/callback?state={}&code=fixture&client_id=oaiapp_other",
        returning.state.secret()
    );
    assert!(parse_callback(&mismatched, &mut returning).is_err());
    let mut returning = AuthorizationAttempt::new(54321, Some("oaiapp_saved".into()));
    let target = format!(
        "/auth/callback?state={}&code=fixture",
        returning.state.secret()
    );
    let omitted = parse_callback(&target, &mut returning).unwrap().unwrap();
    assert_eq!(omitted.client_id, "oaiapp_saved");
    let mut expired = AuthorizationAttempt::new(54321, None);
    expired.expires_at = std::time::Instant::now() - Duration::from_secs(1);
    assert!(parse_callback("/auth/callback?state=anything", &mut expired).is_err());
    assert!(expired.consumed);
}

#[test]
fn identity_requires_real_signature_issuer_audience_expiry_and_nonce() {
    let good = identity_jwt(
        "fixture-subject",
        "oaiapp_fixture",
        "fixture-nonce",
        now() + 60,
    );
    assert_eq!(
        verify_identity(
            &good,
            &fixture_jwks(),
            "oaiapp_fixture",
            Some("fixture-nonce")
        )
        .unwrap()
        .sub,
        "fixture-subject"
    );
    assert!(verify_identity(
        &good,
        &fixture_jwks(),
        "oaiapp_fixture",
        Some("wrong-nonce")
    )
    .is_err());
    assert!(verify_identity(
        &good,
        &fixture_jwks(),
        "oaiapp_other",
        Some("fixture-nonce")
    )
    .is_err());
    let expired = identity_jwt(
        "fixture-subject",
        "oaiapp_fixture",
        "fixture-nonce",
        now() - 60,
    );
    assert!(verify_identity(
        &expired,
        &fixture_jwks(),
        "oaiapp_fixture",
        Some("fixture-nonce")
    )
    .is_err());
    let mut pieces: Vec<_> = good.split('.').map(str::to_string).collect();
    pieces[1] = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"attacker","iss":"https://auth.openai.com","aud":"oaiapp_fixture","exp":4102444800,"iat":1,"nonce":"fixture-nonce"}"#);
    assert!(verify_identity(
        &pieces.join("."),
        &fixture_jwks(),
        "oaiapp_fixture",
        Some("fixture-nonce")
    )
    .is_err());
    let mut jwks = fixture_jwks();
    jwks.keys[0].common.key_id = Some("unrecognized-key".into());
    assert!(verify_identity(&good, &jwks, "oaiapp_fixture", Some("fixture-nonce")).is_err());
    let key = EncodingKey::from_ed_der(&STANDARD.decode(FIXTURE_PKCS8).unwrap());
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some("fixture-key".into());
    for invalid_claims in [
        json!({"iss":"https://untrusted.example.invalid","sub":"fixture-subject","aud":"oaiapp_fixture","exp":now()+60,"iat":now()-1,"nonce":"fixture-nonce"}),
        json!({"iss":ISSUER,"sub":"fixture-subject","aud":"oaiapp_fixture","exp":now()+60,"iat":now()-1}),
        json!({"iss":ISSUER,"sub":"fixture-subject","aud":"oaiapp_fixture","exp":now()+60,"nonce":"fixture-nonce"}),
        json!({"iss":ISSUER,"sub":"fixture-subject","aud":["oaiapp_fixture","oaiapp_other"],"exp":now()+60,"iat":now()-1,"nonce":"fixture-nonce"}),
        json!({"iss":ISSUER,"sub":"fixture-subject","aud":"oaiapp_fixture","azp":"oaiapp_other","exp":now()+60,"iat":now()-1,"nonce":"fixture-nonce"}),
    ] {
        let token = encode(&header, &invalid_claims, &key).unwrap();
        assert!(verify_identity(
            &token,
            &fixture_jwks(),
            "oaiapp_fixture",
            Some("fixture-nonce")
        )
        .is_err());
    }
}

#[test]
fn protected_blob_supports_large_tokens_and_detects_tampering() {
    let key = [7; 32];
    let plaintext = format!("fixture-token-canary-{}", "x".repeat(20_000));
    let mut blob = encrypt_record(&key, plaintext.as_bytes()).unwrap();
    assert!(!String::from_utf8_lossy(&blob).contains("fixture-token-canary"));
    assert_eq!(decrypt_record(&key, &blob).unwrap(), plaintext.as_bytes());
    assert!(decrypt_record(&[8; 32], &blob).is_err());
    blob[20] ^= 1;
    assert!(decrypt_record(&key, &blob).is_err());
}

#[tokio::test]
async fn sixteen_concurrent_callers_refresh_once_and_use_durable_rotation() {
    let (session, store, transport, reference) = session_with(vec![Ok(response(
        "fixture-access-new",
        "fixture-refresh-new",
    ))]);
    let mut callers = Vec::new();
    for _ in 0..16 {
        let (session, reference) = (session.clone(), reference.clone());
        callers.push(tokio::spawn(
            async move { session.bearer(&reference).await },
        ));
    }
    for caller in callers {
        assert_eq!(caller.await.unwrap().unwrap(), "fixture-access-new");
    }
    assert_eq!(transport.refresh_count.load(Ordering::SeqCst), 1);
    let record = store.load().unwrap().unwrap();
    let saved = record.registration.unwrap();
    assert_eq!(
        saved.tokens.unwrap().refresh_token.as_deref(),
        Some("fixture-refresh-new")
    );
    assert_eq!(saved.credential_ref, reference);
    let requests = transport.requests.lock().unwrap();
    assert!(requests[0].contains(&("client_id".into(), "oaiapp_fixture".into())));
    assert!(requests[0].contains(&("resource".into(), RESOURCE.into())));
    assert!(!requests[0].iter().any(|(key, _)| key == "scope"));
}

#[tokio::test]
async fn terminal_refresh_clears_tokens_but_retains_registration_and_host() {
    let (session, store, _, reference) =
        session_with(vec![Err(AuthError::OAuth("refresh_token_reused".into()))]);
    assert_eq!(
        session.bearer(&reference).await.unwrap_err(),
        SIGN_IN_REQUIRED
    );
    let record = store.load().unwrap().unwrap();
    assert_eq!(
        record.host_id,
        "urn:uuid:00000000-0000-4000-8000-000000000002"
    );
    let status = record.status();
    assert!(!status.connected && status.needs_sign_in);
    assert_eq!(status.credential_ref.as_deref(), Some(reference.as_str()));
    assert_eq!(record.registration.unwrap().client_id, "oaiapp_fixture");
}

#[tokio::test]
async fn temporary_failure_preserves_credentials_and_invalid_reference_never_refreshes() {
    let (session, store, transport, reference) = session_with(vec![
        Err(AuthError::Network),
        Err(AuthError::Network),
        Err(AuthError::Network),
    ]);
    assert!(session.bearer(&reference).await.is_err());
    assert!(store
        .load()
        .unwrap()
        .unwrap()
        .registration
        .unwrap()
        .tokens
        .is_some());
    let other = format!("{REFERENCE_PREFIX}00000000-0000-4000-8000-000000000003");
    assert!(session.bearer(&other).await.is_err());
    assert_eq!(transport.refresh_count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn ambiguous_refresh_transport_failure_is_not_replayed_or_treated_as_revocation() {
    for kind in [
        TransportFailure::Timeout,
        TransportFailure::Connect,
        TransportFailure::Request,
        TransportFailure::Response,
    ] {
        let (session, store, transport, reference) = session_with(vec![
            Err(AuthError::Transport(kind)),
            Ok(response(
                "fixture-access-unexpected",
                "fixture-refresh-unexpected",
            )),
        ]);
        assert!(session.bearer(&reference).await.is_err());
        assert_eq!(transport.refresh_count.load(Ordering::SeqCst), 1);
        assert!(store
            .load()
            .unwrap()
            .unwrap()
            .registration
            .unwrap()
            .tokens
            .is_some());
        assert_eq!(transport.responses.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn independent_session_gates_serialize_rotation_with_the_file_lock() {
    let (session, store, transport, reference) = session_with(vec![Ok(response(
        "fixture-access-new",
        "fixture-refresh-new",
    ))]);
    let temporary = tempfile::tempdir().unwrap();
    let lock_path = Some(temporary.path().join("fixture-only.lock"));
    let first = Arc::new(Session {
        store: store.clone(),
        transport: transport.clone(),
        gate: Arc::new(AsyncMutex::new(())),
        lock_path: lock_path.clone(),
    });
    let second = Arc::new(Session {
        store: store.clone(),
        transport: transport.clone(),
        gate: Arc::new(AsyncMutex::new(())),
        lock_path,
    });
    let (first_reference, second_reference) = (reference.clone(), reference.clone());
    let first = tokio::spawn(async move { first.bearer(&first_reference).await });
    let second = tokio::spawn(async move { second.bearer(&second_reference).await });
    assert_eq!(first.await.unwrap().unwrap(), "fixture-access-new");
    assert_eq!(second.await.unwrap().unwrap(), "fixture-access-new");
    assert_eq!(transport.refresh_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        session.bearer(&reference).await.unwrap(),
        "fixture-access-new"
    );
}

#[tokio::test]
async fn rotation_is_not_returned_if_protected_persistence_fails() {
    let (session, store, _, reference) = session_with(vec![Ok(response(
        "fixture-access-new",
        "fixture-refresh-new",
    ))]);
    store.fail_save.store(true, Ordering::SeqCst);
    assert!(session.bearer(&reference).await.is_err());
}

#[tokio::test]
async fn disconnect_revokes_latest_token_and_preserves_registration_for_reauthorization() {
    let (session, store, transport, reference) = session_with(vec![Ok(response(
        "fixture-access-new",
        "fixture-refresh-new",
    ))]);
    session.bearer(&reference).await.unwrap();
    let status = session.disconnect().await.unwrap();
    assert!(!status.connected && !status.plan_enabled && status.needs_sign_in);
    assert_eq!(transport.revoke_count.load(Ordering::SeqCst), 1);
    assert!(transport
        .revoke_requests
        .lock()
        .unwrap()
        .iter()
        .any(|(client, token)| client == "oaiapp_fixture" && token == "fixture-refresh-new"));
    assert!(session.bearer(&reference).await.is_err());
    assert_eq!(
        store
            .load()
            .unwrap()
            .unwrap()
            .registration
            .unwrap()
            .client_id,
        "oaiapp_fixture"
    );
    session.disconnect().await.unwrap();
    assert_eq!(transport.revoke_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_remote_revocation_still_clears_all_local_tokens_and_reports_it() {
    let (session, store, transport, _) = session_with(vec![]);
    transport.fail_revoke.store(true, Ordering::SeqCst);
    assert!(session
        .disconnect()
        .await
        .unwrap_err()
        .contains("未确认远端撤销"));
    assert!(store
        .load()
        .unwrap()
        .unwrap()
        .registration
        .unwrap()
        .tokens
        .is_none());
}

#[tokio::test]
async fn sign_in_only_uses_granted_scopes_and_verifies_returning_subject() {
    let (session, store, _, _) = session_with(vec![]);
    let original = store.load().unwrap().unwrap();
    let attempt = AuthorizationAttempt::new(54321, Some("oaiapp_fixture".into()));
    let callback = Callback {
        code: "fixture-code".into(),
        client_id: "oaiapp_fixture".into(),
    };
    let token_response = TokenResponse {
        id_token: Some(identity_jwt(
            "fixture-subject",
            "oaiapp_fixture",
            attempt.nonce.secret(),
            now() + 60,
        )),
        scope: Some("openid email profile".into()),
        ..response("fixture-access", "fixture-refresh")
    };
    let status = session
        .save_sign_in(
            &original,
            &attempt,
            &callback,
            token_response,
            &fixture_jwks(),
            &Cancellation::new(),
        )
        .await
        .unwrap();
    assert!(status.connected && !status.plan_enabled);
    let wrong_identity = TokenResponse {
        id_token: Some(identity_jwt(
            "different-account",
            "oaiapp_fixture",
            attempt.nonce.secret(),
            now() + 60,
        )),
        scope: Some(SCOPE.into()),
        ..response("fixture-access-attacker", "fixture-refresh-attacker")
    };
    assert!(session
        .save_sign_in(
            &original,
            &attempt,
            &callback,
            wrong_identity,
            &fixture_jwks(),
            &Cancellation::new()
        )
        .await
        .is_err());
    assert_eq!(
        store.load().unwrap().unwrap().registration.unwrap().subject,
        "fixture-subject"
    );
}

#[tokio::test]
async fn cancelled_attempt_cannot_replace_credentials_and_cancel_wakes_waiter() {
    let (session, store, _, _) = session_with(vec![]);
    let original = store.load().unwrap().unwrap();
    let attempt = AuthorizationAttempt::new(54321, Some("oaiapp_fixture".into()));
    let callback = Callback {
        code: "fixture-code".into(),
        client_id: "oaiapp_fixture".into(),
    };
    let token_response = TokenResponse {
        id_token: Some(identity_jwt(
            "fixture-subject",
            "oaiapp_fixture",
            attempt.nonce.secret(),
            now() + 60,
        )),
        scope: Some(SCOPE.into()),
        ..response("fixture-access-new", "fixture-refresh-new")
    };
    let cancellation = Arc::new(Cancellation::new());
    cancellation.cancel();
    tokio::time::timeout(Duration::from_millis(50), cancellation.wait())
        .await
        .unwrap();
    assert!(session
        .save_sign_in(
            &original,
            &attempt,
            &callback,
            token_response,
            &fixture_jwks(),
            &cancellation
        )
        .await
        .unwrap_err()
        .contains("取消"));
    assert_eq!(
        store
            .load()
            .unwrap()
            .unwrap()
            .registration
            .unwrap()
            .tokens
            .unwrap()
            .access_token
            .as_deref(),
        Some("fixture-access-old")
    );
}

#[tokio::test]
async fn cancelling_and_dropping_attempt_releases_the_one_shot_pending_slot() {
    let guard = AttemptGuard::begin().unwrap();
    assert!(AttemptGuard::begin().is_err());
    cancel_sign_in().unwrap();
    tokio::time::timeout(Duration::from_millis(50), guard.cancellation.wait())
        .await
        .unwrap();
    drop(guard);
    let next = AttemptGuard::begin().unwrap();
    assert!(next.cancellation.check().is_ok());
    drop(next);
}

#[tokio::test]
async fn another_process_disconnect_prevents_a_late_sign_in_from_reviving_tokens() {
    let (session, store, _, reference) = session_with(vec![]);
    let original = store.load().unwrap().unwrap();
    let attempt = AuthorizationAttempt::new(54321, Some("oaiapp_fixture".into()));
    let callback = Callback {
        code: "fixture-code".into(),
        client_id: "oaiapp_fixture".into(),
    };
    let token_response = TokenResponse {
        id_token: Some(identity_jwt(
            "fixture-subject",
            "oaiapp_fixture",
            attempt.nonce.secret(),
            now() + 60,
        )),
        scope: Some(SCOPE.into()),
        ..response("fixture-access-new", "fixture-refresh-new")
    };
    // No cancellation object is shared across processes. The durable epoch,
    // not a process-local flag, must reject this otherwise valid callback.
    session.disconnect().await.unwrap();
    assert!(session
        .save_sign_in(
            &original,
            &attempt,
            &callback,
            token_response,
            &fixture_jwks(),
            &Cancellation::new()
        )
        .await
        .unwrap_err()
        .contains("已更改"));
    let status = store.load().unwrap().unwrap().status();
    assert!(!status.connected && status.needs_sign_in);
    assert_eq!(status.credential_ref.as_deref(), Some(reference.as_str()));
}

#[tokio::test]
async fn reauthorization_reuses_issued_client_stable_ref_and_host_without_retained_id_hint() {
    let (session, store, _, reference) = session_with(vec![]);
    session.disconnect().await.unwrap();
    let original = store.load().unwrap().unwrap();
    let attempt = AuthorizationAttempt::new(54321, Some("oaiapp_fixture".into()));
    let url = reqwest::Url::parse(&attempt.authorization_url(&original)).unwrap();
    let parameters: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(parameters["client_id"], "oaiapp_fixture");
    assert_eq!(parameters["ext_agent_host_id"], original.host_id);
    assert!(!parameters.contains_key("agent_name_hint"));
    assert!(!parameters.contains_key("id_token_hint"));
    let callback = Callback {
        code: "fixture-code".into(),
        client_id: "oaiapp_fixture".into(),
    };
    let token_response = TokenResponse {
        id_token: Some(identity_jwt(
            "fixture-subject",
            "oaiapp_fixture",
            attempt.nonce.secret(),
            now() + 60,
        )),
        scope: Some(SCOPE.into()),
        ..response("fixture-access-new", "fixture-refresh-new")
    };
    let status = session
        .save_sign_in(
            &original,
            &attempt,
            &callback,
            token_response,
            &fixture_jwks(),
            &Cancellation::new(),
        )
        .await
        .unwrap();
    assert!(status.connected && status.plan_enabled && !status.needs_sign_in);
    assert_eq!(status.credential_ref.as_deref(), Some(reference.as_str()));
    assert_eq!(store.load().unwrap().unwrap().host_id, original.host_id);
}

#[test]
fn status_and_errors_do_not_contain_token_material() {
    let mut record = Record::new();
    record.registration = Some(registration());
    let serialized = serde_json::to_string(&record.status()).unwrap();
    assert!(!serialized.contains("fixture-access-old"));
    assert!(!serialized.contains("fixture-refresh-old"));
    assert!(!serialized.contains("fixture-retained-id"));
    assert!(!AuthError::OAuth("fixture-token-canary".into())
        .message()
        .contains("fixture-token-canary"));
    assert!(trusted_endpoint("https://auth.openai.com.evil.invalid/jwks").is_err());
    assert!(trusted_endpoint("http://auth.openai.com/jwks").is_err());
    assert!(trusted_endpoint("https://auth.openai.com@evil.invalid/jwks").is_err());
}

#[tokio::test]
async fn old_source_rejects_reauthorization_but_rotation_keeps_its_epoch_valid() {
    let (session, store, transport, reference) = session_with(vec![Ok(response(
        "fixture-access-new",
        "fixture-refresh-new",
    ))]);
    let old_source = source_at_epoch(&session, &reference, 0);
    old_source.validate_session().unwrap();
    assert_eq!(
        old_source.bearer_token().await.unwrap(),
        "fixture-access-new"
    );
    old_source.validate_session().unwrap();
    let mut reauthorized = store.load().unwrap().unwrap();
    reauthorized.authorization_epoch += 1;
    store.save(&reauthorized).unwrap();
    assert!(old_source.validate_session().is_err());
    assert!(old_source.bearer_token().await.is_err());
    let new_source = source_at_epoch(&session, &reference, reauthorized.authorization_epoch);
    new_source.validate_session().unwrap();
    assert_eq!(
        new_source.bearer_token().await.unwrap(),
        "fixture-access-new"
    );
    assert_eq!(transport.refresh_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn disconnect_invalidates_a_late_inference_before_remote_revocation_finishes() {
    let (session, store, transport, reference) = session_with(vec![]);
    let source = source_at_epoch(&session, &reference, 0);
    source.validate_session().unwrap();
    transport.pause_revoke.store(true, Ordering::SeqCst);
    let sign_out_session = session.clone();
    let task = tokio::spawn(async move { sign_out_session.disconnect().await });
    tokio::time::timeout(Duration::from_secs(1), transport.revoke_started.notified())
        .await
        .unwrap();
    assert!(store.load().unwrap().unwrap().disconnecting);
    assert!(!store.load().unwrap().unwrap().status().connected);
    // This is the same read-only guard the Responses adapter invokes before
    // handing a completed stream/tool batch back to the execution runner.
    assert!(source.validate_session().is_err());
    transport.revoke_release.notify_one();
    task.await.unwrap().unwrap();
    assert!(source.bearer_token().await.is_err());
    assert!(store
        .load()
        .unwrap()
        .unwrap()
        .registration
        .unwrap()
        .tokens
        .is_none());
}

#[tokio::test]
async fn interrupted_sign_out_stays_unusable_until_explicit_sign_in_cleanup() {
    let (session, store, _, reference) = session_with(vec![]);
    let mut interrupted = store.load().unwrap().unwrap();
    interrupted.disconnecting = true;
    interrupted.authorization_epoch += 1;
    store.save(&interrupted).unwrap();
    assert!(
        source_at_epoch(&session, &reference, interrupted.authorization_epoch)
            .validate_session()
            .is_err()
    );
    assert!(session.bearer(&reference).await.is_err());
    assert!(!interrupted.status().connected && interrupted.status().needs_sign_in);
    let recovered = session.initialize().await.unwrap();
    assert!(!recovered.disconnecting);
    assert!(recovered.registration.unwrap().tokens.is_none());
}

#[tokio::test]
async fn explicit_account_change_uses_dynamic_registration_and_never_reuses_old_reference() {
    let (session, store, _, old_reference) = session_with(vec![]);
    let original = store.load().unwrap().unwrap();
    let selected = AccountMode::ChangeAccount.selected_record(&original);
    let attempt = AuthorizationAttempt::new(54321, None);
    let url = reqwest::Url::parse(&attempt.authorization_url(&selected)).unwrap();
    let parameters: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(parameters["client_id"], DYNAMIC_CLIENT);
    assert_eq!(parameters["agent_name_hint"], "AngelBot");
    assert_eq!(parameters["ext_agent_host_id"], original.host_id);
    assert!(!parameters.contains_key("id_token_hint"));
    assert!(!parameters.contains_key("login_hint"));
    let old_source = source_at_epoch(&session, &old_reference, original.authorization_epoch);
    old_source.validate_session().unwrap();
    let callback = Callback {
        code: "fixture-code".into(),
        client_id: "oaiapp_other_account".into(),
    };
    let token_response = TokenResponse {
        id_token: Some(identity_jwt(
            "other-account-subject",
            "oaiapp_other_account",
            attempt.nonce.secret(),
            now() + 60,
        )),
        scope: Some(SCOPE.into()),
        ..response("fixture-other-access", "fixture-other-refresh")
    };
    let status = session
        .save_sign_in_with_mode(
            &original,
            &attempt,
            &callback,
            token_response,
            &fixture_jwks(),
            &Cancellation::new(),
            AccountMode::ChangeAccount,
        )
        .await
        .unwrap();
    let new_reference = status.credential_ref.unwrap();
    assert_ne!(new_reference, old_reference);
    assert!(old_source.validate_session().is_err());
    assert!(old_source.bearer_token().await.is_err());
    assert!(session.bearer(&old_reference).await.is_err());
    assert_eq!(
        session.bearer(&new_reference).await.unwrap(),
        "fixture-other-access"
    );
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.host_id, original.host_id);
    assert_eq!(saved.registration.unwrap().subject, "other-account-subject");
}

#[tokio::test]
async fn cancelled_or_failed_account_change_preserves_the_original_active_connection() {
    let (session, store, _, old_reference) = session_with(vec![]);
    let original = store.load().unwrap().unwrap();
    let attempt = AuthorizationAttempt::new(54321, None);
    let callback = Callback {
        code: "fixture-code".into(),
        client_id: "oaiapp_other_account".into(),
    };
    let cancellation = Cancellation::new();
    cancellation.cancel();
    let response_for = || TokenResponse {
        id_token: Some(identity_jwt(
            "other-account-subject",
            "oaiapp_other_account",
            attempt.nonce.secret(),
            now() + 60,
        )),
        scope: Some(SCOPE.into()),
        ..response("fixture-other-access", "fixture-other-refresh")
    };
    assert!(session
        .save_sign_in_with_mode(
            &original,
            &attempt,
            &callback,
            response_for(),
            &fixture_jwks(),
            &cancellation,
            AccountMode::ChangeAccount
        )
        .await
        .is_err());
    assert_eq!(
        store.load().unwrap().unwrap().authorization_epoch,
        original.authorization_epoch
    );
    store.fail_save.store(true, Ordering::SeqCst);
    assert!(session
        .save_sign_in_with_mode(
            &original,
            &attempt,
            &callback,
            response_for(),
            &fixture_jwks(),
            &Cancellation::new(),
            AccountMode::ChangeAccount
        )
        .await
        .is_err());
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.authorization_epoch, original.authorization_epoch);
    let registration = saved.registration.unwrap();
    assert_eq!(registration.credential_ref, old_reference);
    assert_eq!(registration.subject, "fixture-subject");
    assert_eq!(
        registration.tokens.unwrap().access_token.as_deref(),
        Some("fixture-access-old")
    );
}
