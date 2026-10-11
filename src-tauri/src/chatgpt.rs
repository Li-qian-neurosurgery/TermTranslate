//! Official Sign in with ChatGPT public-client flow. Credentials never cross IPC.
use crate::{
    lock,
    storage::atomic_json,
    vault::{self, SecretStore, SystemStore},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
pub const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub client_id: String,
    pub subject: Option<String>,
    pub email: Option<String>,
    pub connected: bool,
    pub plan_enabled: bool,
    pub usage_acknowledged: bool,
}
impl Account {
    pub fn label(&self) -> String {
        format!(
            "{} · {}",
            self.email.as_deref().unwrap_or("ChatGPT 账户"),
            &self.id[..8.min(self.id.len())]
        )
    }
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Registry {
    schema_version: u32,
    host_id: String,
    accounts: Vec<Account>,
}
#[derive(Serialize, Deserialize)]
struct Tokens {
    client_id: String,
    subject: String,
    access_token: String,
    refresh_token: Option<String>,
    id_token: String,
    expires_at: u64,
    scopes: Vec<String>,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: String,
    expires_in: u64,
    scope: Option<String>,
}
#[derive(Deserialize, Clone)]
struct Identity {
    sub: String,
    email: Option<String>,
    nonce: Option<String>,
    iat: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub slug: String,
    pub display_name: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResult {
    pub account_id: String,
    pub plan_enabled: bool,
}
struct Pending {
    id: String,
    cancel: CancellationToken,
}

pub struct ChatGpt {
    root: PathBuf,
    registry: Mutex<Registry>,
    vault: Box<dyn SecretStore>,
    session_lock: tokio::sync::Mutex<()>,
    pending: Mutex<Option<Pending>>,
    models: Mutex<std::collections::HashMap<String, Vec<Model>>>,
    client: Client,
    #[cfg(test)]
    mock_origin: Option<String>,
}
impl ChatGpt {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        Self::with_vault(root, Box::new(SystemStore))
    }
    fn with_vault(root: PathBuf, vault: Box<dyn SecretStore>) -> Result<Self, String> {
        let path = root.join("chatgpt.json");
        let registry: Registry = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "ChatGPT 账户文件损坏，请保留文件并检查备份。")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Registry {
                schema_version: 1,
                host_id: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
                accounts: vec![],
            },
            Err(_) => return Err("无法读取 ChatGPT 账户文件".into()),
        };
        if registry.schema_version != 1 || !registry.host_id.starts_with("urn:uuid:") {
            return Err("ChatGPT 账户文件版本或主机标识无效".into());
        }
        // Persist the host identifier once, before opening any authorization attempt.
        atomic_json(&path, &registry)?;
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "无法初始化 ChatGPT 连接")?;
        Ok(Self {
            root,
            registry: Mutex::new(registry),
            vault,
            session_lock: Default::default(),
            pending: Default::default(),
            models: Default::default(),
            client,
            #[cfg(test)]
            mock_origin: None,
        })
    }
    fn endpoint(&self, production: &str) -> String {
        #[cfg(test)]
        if let Some(origin) = &self.mock_origin {
            return format!("{origin}{}", Url::parse(production).unwrap().path());
        }
        production.into()
    }
    fn validate_auth_endpoint(&self, value: &str) -> Result<Url, String> {
        #[cfg(test)]
        if let Some(origin) = &self.mock_origin {
            let url = Url::parse(value).map_err(|_| "invalid mock endpoint")?;
            if url.origin().ascii_serialization() == *origin {
                return Ok(url);
            }
        }
        official_auth_url(value)
    }
    pub fn accounts(&self) -> Result<Vec<Account>, String> {
        Ok(lock(&self.registry)?.accounts.clone())
    }
    fn account(&self, id: &str) -> Result<Account, String> {
        lock(&self.registry)?
            .accounts
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or("ChatGPT 账户不存在".into())
    }
    fn update_account(&self, account: Account) -> Result<(), String> {
        let mut current = lock(&self.registry)?;
        let mut next = current.clone();
        if let Some(old) = next.accounts.iter_mut().find(|a| a.id == account.id) {
            *old = account;
        } else {
            next.accounts.push(account);
        }
        atomic_json(&self.root.join("chatgpt.json"), &next)?;
        *current = next;
        Ok(())
    }
    pub fn acknowledge(&self, id: &str) -> Result<(), String> {
        let mut account = self.account(id)?;
        account.usage_acknowledged = true;
        self.update_account(account)
    }
    pub fn cancel_login(&self) -> Result<(), String> {
        if let Some(p) = lock(&self.pending)?.as_ref() {
            p.cancel.cancel();
        }
        Ok(())
    }
    pub async fn login(
        &self,
        account_id: Option<String>,
        consent: bool,
        open_browser: impl FnOnce(&str) -> Result<(), String>,
    ) -> Result<LoginResult, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        {
            let mut pending = lock(&self.pending)?;
            if let Some(previous) = pending.take() {
                previous.cancel.cancel();
            }
            *pending = Some(Pending {
                id: id.clone(),
                cancel: cancel.clone(),
            });
        }
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err("已取消 ChatGPT 登录".into()),
            result = tokio::time::timeout(Duration::from_secs(600), self.login_inner(account_id, consent, open_browser)) =>
                result.unwrap_or_else(|_| Err("登录等待超过 10 分钟，请重新点击使用 ChatGPT 登录。".into())),
        };
        let mut pending = lock(&self.pending)?;
        if pending.as_ref().is_some_and(|p| p.id == id) {
            *pending = None;
        }
        result
    }
    async fn login_inner(
        &self,
        account_id: Option<String>,
        consent: bool,
        open_browser: impl FnOnce(&str) -> Result<(), String>,
    ) -> Result<LoginResult, String> {
        let existing = account_id
            .as_deref()
            .map(|id| self.account(id))
            .transpose()?;
        let previous: Option<Tokens> = existing
            .as_ref()
            .map(|a| vault::read(self.vault.as_ref(), &a.id))
            .transpose()?
            .flatten();
        let host = lock(&self.registry)?.host_id.clone();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "无法启动本机登录回调，请检查本机网络权限。")?;
        let redirect = format!(
            "http://127.0.0.1:{}/auth/callback",
            listener
                .local_addr()
                .map_err(|_| "无法读取登录回调地址")?
                .port()
        );
        let attempt = Attempt::new(redirect, existing.as_ref().map(|a| a.client_id.clone()));
        let url = attempt.authorization_url(
            &host,
            previous.as_ref().map(|t| t.id_token.as_str()),
            existing.as_ref().and_then(|a| a.email.as_deref()),
            consent,
        )?;
        open_browser(url.as_str())?;
        let callback = wait_callback(listener, &attempt).await?;
        let _session = self.session_lock.lock().await;
        let mut account = existing.unwrap_or(Account {
            id: uuid::Uuid::new_v4().to_string(),
            client_id: callback.client_id.clone(),
            subject: None,
            email: None,
            connected: false,
            plan_enabled: false,
            usage_acknowledged: false,
        });
        if self
            .accounts()?
            .iter()
            .any(|a| a.id != account.id && a.client_id == callback.client_id)
        {
            return Err("此 ChatGPT 注册已存在，请从已保存账户中选择。".into());
        }
        // Keep the issued registration if code exchange fails, so reauthorization reuses it.
        self.update_account(account.clone())?;
        let response = self
            .client
            .post(self.endpoint(TOKEN))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", callback.client_id.as_str()),
                ("code", callback.code.as_str()),
                ("code_verifier", attempt.verifier.as_str()),
                ("redirect_uri", attempt.redirect.as_str()),
                ("resource", RESOURCE),
            ])
            .send()
            .await
            .map_err(|_| "无法连接 OpenAI 登录服务，请检查网络后重新登录。")?;
        let token = read_token_response(response).await.map_err(|e| e.message)?;
        let id_token = token.id_token.as_deref().ok_or("登录响应缺少身份凭据")?;
        let identity = self
            .verify_identity(id_token, &account.client_id, Some(&attempt.nonce))
            .await?;
        if account
            .subject
            .as_ref()
            .is_some_and(|sub| sub != &identity.sub)
        {
            return Err("登录账户与所选账户不一致，原账户凭据未修改。".into());
        }
        let tokens = Tokens::from_login(token, &account.client_id, &identity.sub)?;
        account.subject = Some(identity.sub);
        account.email = identity.email;
        account.connected = true;
        account.plan_enabled = has_plan_permission(&tokens.scopes);
        vault::write(self.vault.as_ref(), &account.id, &tokens)?;
        self.update_account(account.clone())?;
        lock(&self.models)?.remove(&account.id);
        Ok(LoginResult {
            account_id: account.id,
            plan_enabled: account.plan_enabled,
        })
    }
    async fn discovery(&self) -> Result<Value, String> {
        let value: Value = self
            .client
            .get(self.endpoint(&format!("{ISSUER}/.well-known/openid-configuration")))
            .send()
            .await
            .map_err(|_| "无法获取 OpenAI 登录配置")?
            .error_for_status()
            .map_err(|_| "OpenAI 登录配置暂不可用")?
            .json()
            .await
            .map_err(|_| "OpenAI 登录配置格式无效")?;
        if value["issuer"] != ISSUER {
            return Err("OpenAI 身份提供方不匹配".into());
        }
        Ok(value)
    }
    async fn verify_identity(
        &self,
        token: &str,
        client_id: &str,
        nonce: Option<&str>,
    ) -> Result<Identity, String> {
        let discovery = self.discovery().await?;
        let url = self.validate_auth_endpoint(
            discovery["jwks_uri"]
                .as_str()
                .ok_or("登录配置缺少签名密钥地址")?,
        )?;
        let jwks: JwkSet = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| "无法读取 OpenAI 签名密钥")?
            .error_for_status()
            .map_err(|_| "OpenAI 签名密钥暂不可用")?
            .json()
            .await
            .map_err(|_| "签名密钥格式无效")?;
        verify_jwt(token, client_id, nonce, &jwks)
    }
    // A translation may stop while a rotating refresh is in flight. Keep renewal alive
    // until its replacement is safely stored, even if its caller drops the join handle.
    pub fn access_token_task(
        self: &Arc<Self>,
        id: String,
    ) -> tokio::task::JoinHandle<Result<String, String>> {
        let service = Arc::clone(self);
        tokio::spawn(async move { service.access_token(&id).await })
    }
    pub async fn access_token(&self, id: &str) -> Result<String, String> {
        let _session = self.session_lock.lock().await;
        let account = self.account(id)?;
        if !account.connected {
            return Err("请先使用 ChatGPT 登录此账户。".into());
        }
        let mut saved: Tokens =
            vault::read(self.vault.as_ref(), id)?.ok_or("ChatGPT 登录凭据不存在，请重新登录。")?;
        if saved.client_id != account.client_id
            || account.subject.as_deref() != Some(&saved.subject)
        {
            return Err("ChatGPT 凭据与账户不匹配，请重新登录。".into());
        }
        if !has_plan_permission(&saved.scopes) {
            return Err("尚未授权使用 ChatGPT 套餐，请在模型设置中点击「启用订阅额度」。".into());
        }
        if saved.expires_at > now() + 60 {
            return Ok(saved.access_token);
        }
        let refresh = saved
            .refresh_token
            .as_deref()
            .ok_or("ChatGPT 登录已到期，请重新登录。")?;
        let response = self
            .client
            .post(self.endpoint(TOKEN))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", saved.client_id.as_str()),
                ("refresh_token", refresh),
                ("resource", RESOURCE),
            ])
            .send()
            .await
            .map_err(|_| "ChatGPT 登录续期暂时无法连接，请稍后重试；已保留凭据。")?;
        let refreshed = match read_token_response(response).await {
            Ok(value) => value,
            Err(error) => {
                if error.terminal {
                    vault::clear(self.vault.as_ref(), id)?;
                    let mut account = account;
                    account.connected = false;
                    account.plan_enabled = false;
                    self.update_account(account)?;
                }
                return Err(error.message);
            }
        };
        if let Some(id_token) = &refreshed.id_token {
            let identity = self
                .verify_identity(id_token, &saved.client_id, None)
                .await?;
            if identity.sub != saved.subject {
                return Err("续期身份与当前 ChatGPT 账户不一致。".into());
            }
        }
        saved.replace(refreshed)?;
        vault::write(self.vault.as_ref(), id, &saved)?;
        let mut account = account;
        account.plan_enabled = has_plan_permission(&saved.scopes);
        self.update_account(account)?;
        if !has_plan_permission(&saved.scopes) {
            return Err("ChatGPT 套餐使用权限已取消，请重新授权。".into());
        }
        Ok(saved.access_token)
    }
    pub async fn list_models(&self, id: &str) -> Result<Vec<Model>, String> {
        let token = self.access_token(id).await?;
        let response = self
            .client
            .get(self.endpoint(&format!("{RESOURCE}/models")))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "无法加载 ChatGPT 可用模型，请稍后重试。")?;
        let status = response.status().as_u16();
        let body: Value = response
            .json()
            .await
            .map_err(|_| "ChatGPT 模型列表格式无效")?;
        if !(200..300).contains(&status) {
            return Err(crate::provider::chatgpt_api_error(status, &body));
        }
        let models = parse_models(&body)?;
        lock(&self.models)?.insert(id.into(), models.clone());
        Ok(models)
    }
    pub fn selected_account(&self, id: &str, model: &str) -> Result<Account, String> {
        let account = self.account(id)?;
        if !account.connected || !account.plan_enabled {
            return Err("请先登录并授权使用 ChatGPT 套餐。".into());
        }
        if !lock(&self.models)?
            .get(id)
            .is_some_and(|models| models.iter().any(|m| m.slug == model))
        {
            return Err("请刷新模型列表并选择当前账户可用的模型。".into());
        }
        Ok(account)
    }
    pub async fn logout(&self, id: &str) -> Result<String, String> {
        self.cancel_login()?;
        let _session = self.session_lock.lock().await;
        let mut account = self.account(id)?;
        let saved: Option<Tokens> = vault::read(self.vault.as_ref(), id)?;
        let mut confirmed = saved.is_none();
        if let Some(token) = saved.as_ref().and_then(|t| t.refresh_token.as_ref()) {
            if let Ok(discovery) = self.discovery().await {
                if let Some(url) = discovery["revocation_endpoint"]
                    .as_str()
                    .and_then(|url| self.validate_auth_endpoint(url).ok())
                {
                    for attempt in 0..2 {
                        let result = self
                            .client
                            .post(url.clone())
                            .form(&[
                                ("token", token.as_str()),
                                ("token_type_hint", "refresh_token"),
                                ("client_id", account.client_id.as_str()),
                            ])
                            .send()
                            .await;
                        match result {
                            Ok(response) if response.status().as_u16() == 200 => {
                                confirmed = true;
                                break;
                            }
                            Ok(response) if !response.status().is_server_error() => break,
                            _ => {
                                if attempt == 0 {
                                    tokio::time::sleep(Duration::from_millis(300)).await;
                                }
                            }
                        }
                    }
                }
            }
        }
        vault::clear(self.vault.as_ref(), id)?;
        account.connected = false;
        account.plan_enabled = false;
        self.update_account(account)?;
        lock(&self.models)?.remove(id);
        Ok(if confirmed {
            "已退出 ChatGPT，账户注册已保留，可再次登录。"
        } else {
            "已清除本机登录凭据，但未能确认远端撤销。可在 ChatGPT 设置中断开此应用。"
        }
        .into())
    }
}

impl Tokens {
    fn from_login(response: TokenResponse, client_id: &str, subject: &str) -> Result<Self, String> {
        validate_token_response(&response)?;
        Ok(Self {
            client_id: client_id.into(),
            subject: subject.into(),
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            id_token: response.id_token.ok_or("登录响应缺少身份凭据")?,
            expires_at: now() + response.expires_in,
            scopes: response
                .scope
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
        })
    }
    fn replace(&mut self, response: TokenResponse) -> Result<(), String> {
        validate_token_response(&response)?;
        let refresh = response
            .refresh_token
            .ok_or("续期响应缺少轮换凭据，请重新登录。")?;
        self.access_token = response.access_token;
        self.refresh_token = Some(refresh);
        if let Some(id_token) = response.id_token {
            self.id_token = id_token;
        }
        if let Some(scope) = response.scope {
            self.scopes = scope.split_whitespace().map(str::to_owned).collect();
        }
        self.expires_at = now() + response.expires_in;
        Ok(())
    }
}
fn validate_token_response(value: &TokenResponse) -> Result<(), String> {
    if !value.token_type.eq_ignore_ascii_case("Bearer")
        || value.access_token.is_empty()
        || value.expires_in == 0
        || value.expires_in > 86400
    {
        return Err("OpenAI 登录响应无效".into());
    }
    Ok(())
}
fn now() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}
fn has_plan_permission(scopes: &[String]) -> bool {
    [DIRECT_SCOPE, "resource.invoke"]
        .iter()
        .all(|scope| scopes.iter().any(|s| s == scope))
}
fn random_value() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn official_auth_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "OpenAI 登录端点无效")?;
    if url.scheme() != "https"
        || url.host_str() != Some("auth.openai.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("拒绝非 OpenAI 登录端点".into());
    }
    Ok(url)
}
struct Attempt {
    state: String,
    nonce: String,
    verifier: String,
    redirect: String,
    client_id: Option<String>,
}
impl Attempt {
    fn new(redirect: String, client_id: Option<String>) -> Self {
        Self {
            state: random_value(),
            nonce: random_value(),
            verifier: random_value(),
            redirect,
            client_id,
        }
    }
    fn authorization_url(
        &self,
        host: &str,
        id_hint: Option<&str>,
        email: Option<&str>,
        consent: bool,
    ) -> Result<Url, String> {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes()));
        let mut url = official_auth_url(AUTHORIZE)?;
        {
            let mut params = url.query_pairs_mut();
            params.extend_pairs([
                (
                    "client_id",
                    self.client_id.as_deref().unwrap_or("dynamic_agent_client"),
                ),
                ("ext_agent_host_id", host),
                ("response_type", "code"),
                ("redirect_uri", self.redirect.as_str()),
                ("scope", SCOPES),
                ("resource", RESOURCE),
                ("state", self.state.as_str()),
                ("nonce", self.nonce.as_str()),
                ("code_challenge_method", "S256"),
                ("code_challenge", challenge.as_str()),
            ]);
            if self.client_id.is_none() {
                params.append_pair("agent_name_hint", "TermTranslate");
            } else {
                if let Some(hint) = id_hint {
                    params.append_pair("id_token_hint", hint);
                }
                if let Some(email) = email {
                    params.append_pair("login_hint", email);
                }
            }
            if consent {
                params.append_pair("prompt", "consent");
            }
        }
        Ok(url)
    }
}
struct Callback {
    code: String,
    client_id: String,
}
fn parse_callback(target: &str, attempt: &Attempt) -> Result<Callback, String> {
    if !target.starts_with("/auth/callback?") {
        return Err("登录回调路径无效".into());
    }
    let url = Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| "登录回调格式无效")?;
    let mut pairs = std::collections::HashMap::new();
    for (key, value) in url.query_pairs() {
        if pairs.insert(key.into_owned(), value.into_owned()).is_some() {
            return Err("登录回调含重复参数".into());
        }
    }
    if pairs.get("state") != Some(&attempt.state) {
        return Err("登录回调校验失败，请重新登录。".into());
    }
    if pairs.contains_key("error") {
        return Err(
            if pairs.get("error").map(String::as_str) == Some("access_denied") {
                "你已取消授权，未启用 ChatGPT 订阅额度。"
            } else {
                "ChatGPT 授权未完成，请重新登录。"
            }
            .into(),
        );
    }
    let client_id = match (&attempt.client_id, pairs.get("client_id")) {
        (Some(expected), Some(received)) if expected != received => {
            return Err("登录回调与所选账户注册不一致".into())
        }
        (Some(expected), _) => expected.clone(),
        (None, Some(issued)) if issued != "dynamic_agent_client" && !issued.is_empty() => {
            issued.clone()
        }
        _ => return Err("ChatGPT 注册未完成：回调缺少签发的客户端 ID。".into()),
    };
    let code = pairs
        .get("code")
        .filter(|code| !code.is_empty())
        .ok_or("登录回调缺少授权码")?
        .clone();
    Ok(Callback { code, client_id })
}
async fn wait_callback(listener: TcpListener, attempt: &Attempt) -> Result<Callback, String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|_| "本机登录回调连接失败")?;
        let request = match tokio::time::timeout(
            Duration::from_secs(5),
            read_http_request(&mut stream),
        )
        .await
        {
            Ok(Ok(request)) => request,
            _ => {
                let _ = reply(&mut stream, 400).await;
                continue;
            }
        };
        let first = request.lines().next().unwrap_or_default();
        let mut parts = first.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let target = parts.next().unwrap_or_default();
        if !target.starts_with("/auth/callback?") {
            let _ = reply(&mut stream, 404).await;
            continue;
        }
        let expected_host = attempt
            .redirect
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or_default();
        let hosts: Vec<_> = request
            .lines()
            .filter_map(|line| line.split_once(':'))
            .filter(|(key, _)| key.eq_ignore_ascii_case("host"))
            .map(|(_, value)| value.trim())
            .collect();
        if method != "GET" || hosts != [expected_host] {
            let _ = reply(&mut stream, 400).await;
            continue;
        }
        let result = parse_callback(target, attempt);
        let _ = reply(&mut stream, if result.is_ok() { 200 } else { 400 }).await;
        return result;
    }
}
async fn read_http_request(stream: &mut TcpStream) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    while bytes.len() < 16384 {
        let read = stream.read(&mut chunk).await.map_err(|_| "回调读取失败")?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            return String::from_utf8(bytes).map_err(|_| "回调格式无效".into());
        }
    }
    Err("回调请求过长或不完整".into())
}
async fn reply(stream: &mut TcpStream, status: u16) -> std::io::Result<()> {
    let message = if status == 200 {
        "已收到授权。请返回 TermTranslate 查看登录结果，你可以关闭此页面。"
    } else {
        "登录回调无效。请返回 TermTranslate 重新发起登录。"
    };
    let body = format!("<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><title>TermTranslate</title><h1>TermTranslate</h1><p>{message}</p></html>");
    let response = format!("HTTP/1.1 {status} {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n{body}", if status == 200 { "OK" } else { "Bad Request" }, body.len());
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}
fn verify_jwt(
    token: &str,
    client_id: &str,
    nonce: Option<&str>,
    jwks: &JwkSet,
) -> Result<Identity, String> {
    let header = decode_header(token).map_err(|_| "身份凭据格式无效")?;
    if !matches!(
        header.alg,
        Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::ES256
            | Algorithm::ES384
    ) {
        return Err("身份凭据签名算法不受支持".into());
    }
    let key = jwks
        .find(header.kid.as_deref().ok_or("身份凭据缺少密钥 ID")?)
        .ok_or("OpenAI 签名密钥未找到，请重试")?;
    let key = DecodingKey::from_jwk(key).map_err(|_| "OpenAI 签名密钥无效")?;
    let mut validation = Validation::new(header.alg);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.leeway = 5;
    validation.validate_nbf = true;
    validation.set_required_spec_claims(&["sub", "exp", "iat", "iss", "aud"]);
    let identity = decode::<Identity>(token, &key, &validation)
        .map_err(|_| "ChatGPT 身份验证失败：签名、有效期或账户注册不匹配。")?
        .claims;
    if identity.sub.is_empty()
        || identity.iat > now() + 5
        || nonce.is_some_and(|expected| identity.nonce.as_deref() != Some(expected))
    {
        return Err("ChatGPT 身份随机值或签发时间校验失败。".into());
    }
    Ok(identity)
}
fn parse_models(body: &Value) -> Result<Vec<Model>, String> {
    let rows = body["models"]
        .as_array()
        .ok_or("ChatGPT 未返回可用模型列表")?;
    let models: Vec<_> = rows
        .iter()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| {
            let slug = m["slug"].as_str()?.to_owned();
            if slug.is_empty() {
                return None;
            }
            Some(Model {
                display_name: m["display_name"].as_str().unwrap_or(&slug).into(),
                slug,
            })
        })
        .collect();
    if models.is_empty() {
        return Err("当前 ChatGPT 账户没有可用于此应用的模型，请检查套餐与工作空间权限。".into());
    }
    Ok(models)
}
struct TokenError {
    message: String,
    terminal: bool,
}
async fn read_token_response(response: reqwest::Response) -> Result<TokenResponse, TokenError> {
    let status = response.status().as_u16();
    let body: Value = response.json().await.map_err(|_| TokenError {
        message: "OpenAI 登录服务返回无效数据，请稍后重试。".into(),
        terminal: false,
    })?;
    if !(200..300).contains(&status) {
        let code = body["error"]
            .as_str()
            .or_else(|| body["error"]["code"].as_str())
            .unwrap_or_default();
        let terminal = matches!(
            code,
            "invalid_grant"
                | "invalid_refresh_token"
                | "token_expired"
                | "refresh_token_expired"
                | "refresh_token_invalidated"
                | "refresh_token_reused"
        );
        let message = if terminal {
            "ChatGPT 授权已过期或撤销，请重新登录。"
        } else if code == "invalid_client" {
            "ChatGPT 客户端注册无效，请检查注册信息。"
        } else {
            "ChatGPT 登录服务暂时不可用，请稍后重试；原有凭据已保留。"
        };
        return Err(TokenError {
            message: message.into(),
            terminal,
        });
    }
    serde_json::from_value(body).map_err(|_| TokenError {
        message: "OpenAI 登录响应字段不完整".into(),
        terminal: false,
    })
}

#[cfg(test)]
mod tests;
