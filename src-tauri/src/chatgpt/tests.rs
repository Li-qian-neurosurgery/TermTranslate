use super::*;
use crate::vault::tests::MemoryStore;
use jsonwebtoken::{EncodingKey, Header};
use serde_json::json;
use std::sync::Arc;

fn fixture_jwks() -> JwkSet {
    serde_json::from_str(include_str!("test-jwks.json")).unwrap()
}
fn signed(mut claims: Value) -> String {
    if claims.get("exp").is_none() {
        claims["exp"] = json!(now() + 600);
    }
    if claims.get("iat").is_none() {
        claims["iat"] = json!(now());
    }
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("test-only".into());
    jsonwebtoken::encode(
        &header,
        &claims,
        &EncodingKey::from_ec_pem(include_bytes!("test-private-key.pem")).unwrap(),
    )
    .unwrap()
}
fn claims() -> Value {
    json!({"iss":ISSUER,"aud":"oaiapp_test","sub":"account-one","nonce":"nonce-one","email":"test@example.invalid"})
}

#[test]
fn jwt_checks_signature_identity_nonce_and_expiry() {
    let token = signed(claims());
    assert_eq!(
        verify_jwt(&token, "oaiapp_test", Some("nonce-one"), &fixture_jwks())
            .unwrap()
            .sub,
        "account-one"
    );
    for (field, value) in [
        ("iss", json!("https://evil.invalid")),
        ("aud", json!("another-client")),
        ("exp", json!(now() - 120)),
        ("nonce", json!("wrong")),
        ("sub", json!("")),
        ("iat", json!(now() + 300)),
    ] {
        let mut value_claims = claims();
        value_claims[field] = value;
        assert!(
            verify_jwt(
                &signed(value_claims),
                "oaiapp_test",
                Some("nonce-one"),
                &fixture_jwks()
            )
            .is_err(),
            "{field}"
        );
    }
    let mut parts: Vec<_> = token.split('.').map(str::to_owned).collect();
    parts[1] = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"sub":"tampered"})).unwrap());
    assert!(verify_jwt(&parts.join("."), "oaiapp_test", None, &fixture_jwks()).is_err());
}

#[test]
fn authorization_pkce_registration_reuse_and_callback_validation() {
    let attempt = Attempt::new("http://127.0.0.1:54321/auth/callback".into(), None);
    let url = attempt
        .authorization_url("urn:uuid:host", None, None, false)
        .unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(params["client_id"], "dynamic_agent_client");
    assert_eq!(params["agent_name_hint"], "TermTranslate");
    assert_eq!(
        params["code_challenge"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(attempt.verifier.as_bytes()))
    );
    assert_eq!(params["resource"], RESOURCE);
    assert!(params["scope"].contains(DIRECT_SCOPE));
    let callback = format!(
        "/auth/callback?state={}&code=fake-code&client_id=oaiapp_test",
        attempt.state
    );
    assert_eq!(
        parse_callback(&callback, &attempt).unwrap().client_id,
        "oaiapp_test"
    );
    assert!(parse_callback(&callback.replace(&attempt.state, "bad-state"), &attempt).is_err());
    assert!(parse_callback(&format!("{callback}&state={}", attempt.state), &attempt).is_err());
    assert!(parse_callback(
        &format!("/auth/callback?state={}&code=x", attempt.state),
        &attempt
    )
    .is_err());
    assert!(parse_callback(
        &format!("/auth/callback?state={}&error=access_denied", attempt.state),
        &attempt
    )
    .err()
    .unwrap()
    .contains("取消"));
    let returning = Attempt::new(attempt.redirect, Some("oaiapp_test".into()));
    let url = returning
        .authorization_url(
            "urn:uuid:host",
            Some("retained-id-token"),
            Some("test@example.invalid"),
            true,
        )
        .unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert!(!params.contains_key("agent_name_hint"));
    assert_eq!(params["id_token_hint"], "retained-id-token");
    assert_eq!(params["prompt"], "consent");
    assert!(parse_callback(
        &format!(
            "/auth/callback?state={}&code=x&client_id=different",
            returning.state
        ),
        &returning
    )
    .is_err());
    assert_eq!(
        parse_callback(
            &format!("/auth/callback?state={}&code=x", returning.state),
            &returning
        )
        .unwrap()
        .client_id,
        "oaiapp_test"
    );
    assert_ne!(attempt.state, returning.state);
}

#[test]
fn protected_endpoints_and_account_specific_model_catalog() {
    for endpoint in [
        "http://auth.openai.com/token",
        "https://evil.invalid/token",
        "https://auth.openai.com.evil.invalid/token",
        "https://user@auth.openai.com/token",
    ] {
        assert!(official_auth_url(endpoint).is_err());
    }
    let models = parse_models(&json!({"models":[{"visibility":"hidden","slug":"secret"},{"visibility":"list","slug":"a","display_name":"Alpha"},{"visibility":"list","slug":"b"}]})).unwrap();
    assert_eq!(
        models.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert!(parse_models(&json!({"models":[]})).is_err());
    assert!(!has_plan_permission(&[
        "openid".into(),
        "resource.invoke".into()
    ]));
    assert!(has_plan_permission(&[
        DIRECT_SCOPE.into(),
        "resource.invoke".into()
    ]));
}

#[derive(Default)]
struct ServerState {
    nonce: String,
    grants: String,
    refreshes: usize,
    refresh_error: Option<String>,
    revocations: usize,
    delay_refresh: bool,
    requests: Vec<(String, String)>,
}
async fn mock_server() -> (String, Arc<Mutex<ServerState>>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(ServerState {
        grants: SCOPES.into(),
        ..Default::default()
    }));
    let saved = state.clone();
    let server_origin = origin.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let state = saved.clone();
            let origin = server_origin.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut chunk = [0; 1024];
                loop {
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..end]);
                        let length = header
                            .lines()
                            .filter_map(|l| l.split_once(':'))
                            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                let (header, body) = request.split_once("\r\n\r\n").unwrap();
                let path = header
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                let (status, value) = {
                    let mut state = state.lock().unwrap();
                    state.requests.push((path.clone(), body.into()));
                    match path.as_str() {
                        "/.well-known/openid-configuration" => (
                            200,
                            json!({"issuer":ISSUER,"jwks_uri":format!("{origin}/jwks"),"revocation_endpoint":format!("{origin}/revoke")}),
                        ),
                        "/jwks" => (
                            200,
                            serde_json::from_str(include_str!("test-jwks.json")).unwrap(),
                        ),
                        "/api/accounts/oauth/token" => {
                            if body.contains("grant_type=refresh_token") {
                                state.refreshes += 1;
                                if let Some(error) = &state.refresh_error {
                                    (
                                        if error == "invalid_grant" { 400 } else { 503 },
                                        json!({"error":error}),
                                    )
                                } else {
                                    (
                                        200,
                                        json!({"access_token":"renewed-access","refresh_token":"renewed-refresh","token_type":"Bearer","expires_in":3600}),
                                    )
                                }
                            } else {
                                let mut identity = claims();
                                identity["nonce"] = json!(state.nonce);
                                (
                                    200,
                                    json!({"access_token":"initial-access","refresh_token":"initial-refresh","id_token":signed(identity),"token_type":"Bearer","expires_in":3600,"scope":state.grants}),
                                )
                            }
                        }
                        "/v1/models" => {
                            assert!(header.to_lowercase().contains("authorization: bearer"));
                            (
                                200,
                                json!({"models":[{"slug":"test-model","display_name":"Test model","visibility":"list"}]}),
                            )
                        }
                        "/revoke" => {
                            state.revocations += 1;
                            (200, Value::Null)
                        }
                        _ => (404, json!({})),
                    }
                };
                if path.contains("oauth/token")
                    && body.contains("grant_type=refresh_token")
                    && state.lock().unwrap().delay_refresh
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                let body = if path == "/revoke" {
                    String::new()
                } else {
                    value.to_string()
                };
                let response = format!("HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    (origin, state, task)
}
fn browser_callback(url: &str, state: Arc<Mutex<ServerState>>) -> Result<(), String> {
    let url = Url::parse(url).unwrap();
    let params: std::collections::HashMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    state.lock().unwrap().nonce = params["nonce"].clone();
    let mut callback = Url::parse(&params["redirect_uri"]).unwrap();
    callback.query_pairs_mut().extend_pairs([
        ("state", params["state"].as_str()),
        ("code", "fake-code"),
        ("client_id", "oaiapp_test"),
    ]);
    tokio::spawn(async move {
        reqwest::get(callback).await.unwrap();
    });
    Ok(())
}

#[tokio::test]
async fn full_local_oauth_refresh_rotation_and_logout() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, state, server) = mock_server().await;
    let mut service =
        ChatGpt::with_vault(dir.path().into(), Box::<MemoryStore>::default()).unwrap();
    service.mock_origin = Some(origin);
    let login = service
        .login(None, false, |url| browser_callback(url, state.clone()))
        .await
        .unwrap();
    assert!(login.plan_enabled);
    assert_eq!(service.accounts().unwrap().len(), 1);
    let models = service.list_models(&login.account_id).await.unwrap();
    assert_eq!(models[0].slug, "test-model");
    assert!(service
        .selected_account(&login.account_id, "not-available")
        .is_err());
    let mut tokens: Tokens = vault::read(service.vault.as_ref(), &login.account_id)
        .unwrap()
        .unwrap();
    tokens.expires_at = 1;
    vault::write(service.vault.as_ref(), &login.account_id, &tokens).unwrap();
    let (a, b) = tokio::join!(
        service.access_token(&login.account_id),
        service.access_token(&login.account_id)
    );
    assert_eq!(a.unwrap(), "renewed-access");
    assert_eq!(b.unwrap(), "renewed-access");
    assert_eq!(state.lock().unwrap().refreshes, 1);
    let tokens: Tokens = vault::read(service.vault.as_ref(), &login.account_id)
        .unwrap()
        .unwrap();
    assert_eq!(tokens.refresh_token.as_deref(), Some("renewed-refresh"));
    let json = std::fs::read_to_string(dir.path().join("chatgpt.json")).unwrap();
    assert!(
        !json.contains("initial-access")
            && !json.contains("renewed-refresh")
            && !json.contains("id_token")
    );
    let registry: Registry = serde_json::from_str(&json).unwrap();
    assert!(registry.host_id.starts_with("urn:uuid:"));
    let token_request = state
        .lock()
        .unwrap()
        .requests
        .iter()
        .find(|(p, b)| p.contains("oauth/token") && b.contains("authorization_code"))
        .unwrap()
        .1
        .clone();
    assert!(
        token_request.contains("client_id=oaiapp_test")
            && token_request.contains("code_verifier=")
            && token_request.contains("resource=https%3A%2F%2Fapi.openai.com%2Fv1")
    );
    assert!(!token_request.contains("client_secret"));
    service.logout(&login.account_id).await.unwrap();
    assert_eq!(state.lock().unwrap().revocations, 1);
    assert!(
        vault::read::<Tokens>(service.vault.as_ref(), &login.account_id)
            .unwrap()
            .is_none()
    );
    let account = service.accounts().unwrap().remove(0);
    assert!(!account.connected);
    assert_eq!(account.client_id, "oaiapp_test");
    server.abort();
}

#[tokio::test]
async fn missing_scope_blocks_inference_and_login_cancellation_closes_listener() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, state, server) = mock_server().await;
    state.lock().unwrap().grants = "openid email profile".into();
    let mut service =
        ChatGpt::with_vault(dir.path().into(), Box::<MemoryStore>::default()).unwrap();
    service.mock_origin = Some(origin);
    let service = Arc::new(service);
    let login = service
        .login(None, false, |url| browser_callback(url, state.clone()))
        .await
        .unwrap();
    assert!(!login.plan_enabled);
    assert!(service
        .access_token(&login.account_id)
        .await
        .unwrap_err()
        .contains("授权"));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let task_service = service.clone();
    let login_task = tokio::spawn(async move {
        task_service
            .login(None, false, |url| {
                tx.send(url.to_owned()).unwrap();
                Ok(())
            })
            .await
    });
    let url = Url::parse(&rx.await.unwrap()).unwrap();
    let callback = url
        .query_pairs()
        .find(|(k, _)| k == "redirect_uri")
        .unwrap()
        .1
        .into_owned();
    service.cancel_login().unwrap();
    assert!(login_task.await.unwrap().err().unwrap().contains("取消"));
    assert!(reqwest::get(callback).await.is_err());
    server.abort();
}

#[tokio::test]
async fn temporary_refresh_failure_preserves_credentials_revocation_clears_them() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, state, server) = mock_server().await;
    let mut service =
        ChatGpt::with_vault(dir.path().into(), Box::<MemoryStore>::default()).unwrap();
    service.mock_origin = Some(origin);
    let login = service
        .login(None, false, |url| browser_callback(url, state.clone()))
        .await
        .unwrap();
    let mut tokens: Tokens = vault::read(service.vault.as_ref(), &login.account_id)
        .unwrap()
        .unwrap();
    tokens.expires_at = 1;
    vault::write(service.vault.as_ref(), &login.account_id, &tokens).unwrap();
    state.lock().unwrap().refresh_error = Some("temporarily_unavailable".into());
    assert!(service.access_token(&login.account_id).await.is_err());
    assert!(
        vault::read::<Tokens>(service.vault.as_ref(), &login.account_id)
            .unwrap()
            .is_some()
    );
    state.lock().unwrap().refresh_error = Some("invalid_grant".into());
    assert!(service.access_token(&login.account_id).await.is_err());
    assert!(
        vault::read::<Tokens>(service.vault.as_ref(), &login.account_id)
            .unwrap()
            .is_none()
    );
    assert!(!service.account(&login.account_id).unwrap().connected);
    server.abort();
}

#[tokio::test]
async fn cancelling_translation_does_not_lose_a_rotated_refresh_token() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, state, server) = mock_server().await;
    let mut service =
        ChatGpt::with_vault(dir.path().into(), Box::<MemoryStore>::default()).unwrap();
    service.mock_origin = Some(origin);
    let service = Arc::new(service);
    let login = service
        .login(None, false, |url| browser_callback(url, state.clone()))
        .await
        .unwrap();
    let mut tokens: Tokens = vault::read(service.vault.as_ref(), &login.account_id)
        .unwrap()
        .unwrap();
    tokens.expires_at = 1;
    vault::write(service.vault.as_ref(), &login.account_id, &tokens).unwrap();
    state.lock().unwrap().delay_refresh = true;
    let pending = service.access_token_task(login.account_id.clone());
    tokio::time::timeout(Duration::from_secs(2), async {
        while state.lock().unwrap().refreshes == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    drop(pending); // Same cancellation boundary as the translation command's select!.
    assert_eq!(
        service.access_token(&login.account_id).await.unwrap(),
        "renewed-access"
    );
    assert_eq!(state.lock().unwrap().refreshes, 1);
    let saved: Tokens = vault::read(service.vault.as_ref(), &login.account_id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.refresh_token.as_deref(), Some("renewed-refresh"));
    server.abort();
}
