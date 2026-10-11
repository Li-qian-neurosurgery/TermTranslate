mod chatgpt;
mod model;
mod provider;
mod smoke;
mod storage;
mod vault;

use model::*;
use serde::Serialize;
use std::sync::{Arc, Mutex, MutexGuard};
use storage::Store;
use tauri::{
    ipc::Channel,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, State,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;

struct ActiveRequest {
    id: String,
    cancel: CancellationToken,
}
#[derive(Default)]
struct RequestGate(Mutex<Option<ActiveRequest>>);
impl RequestGate {
    fn cancel_all(&self) -> Result<(), String> {
        if let Some(active) = lock(&self.0)?.as_ref() {
            active.cancel.cancel();
        }
        Ok(())
    }
    fn begin(&self, id: String) -> Result<CancellationToken, String> {
        let mut active = lock(&self.0)?;
        if let Some(previous) = active.take() {
            previous.cancel.cancel();
        }
        let cancel = CancellationToken::new();
        *active = Some(ActiveRequest {
            id,
            cancel: cancel.clone(),
        });
        Ok(cancel)
    }
    fn cancel(&self, id: &str) -> Result<(), String> {
        if let Some(active) = lock(&self.0)?.as_ref() {
            if active.id == id {
                active.cancel.cancel();
            }
        }
        Ok(())
    }
}
struct AppState {
    store: Mutex<Store>,
    chatgpt: Arc<chatgpt::ChatGpt>,
    requests: RequestGate,
    activations: std::sync::atomic::AtomicUsize,
}
fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
    mutex
        .lock()
        .map_err(|_| "内部状态暂不可用，请重新启动程序".into())
}

const KEY_SERVICE: &str = "com.termtranslate.desktop";
fn credential(id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEY_SERVICE, id).map_err(|_| "无法访问系统凭据存储".to_string())
}
fn read_key(profile: &Profile) -> Result<String, String> {
    if !profile.has_key {
        return Ok(String::new());
    }
    match credential(&profile.id)?.get_password() {
        Ok(key) => Ok(key),
        Err(keyring::Error::NoEntry) if !profile.has_key => Ok(String::new()),
        Err(keyring::Error::NoEntry) => {
            Err("系统凭据中未找到 API Key，请在设置中重新保存。".into())
        }
        Err(_) => Err("无法读取系统凭据，请解锁凭据库后重试。".into()),
    }
}
fn delete_key(id: &str) -> Result<(), String> {
    match credential(id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err("无法从系统凭据中删除 API Key".into()),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    settings: Settings,
    prompts: Vec<Prompt>,
    history: Vec<HistoryEntry>,
    warnings: Vec<String>,
    platform: String,
    chatgpt_accounts: Vec<chatgpt::Account>,
}
#[tauri::command]
fn get_snapshot(state: State<'_, AppState>) -> Result<Snapshot, String> {
    let store = lock(&state.store)?;
    Ok(Snapshot {
        settings: store.settings.clone(),
        prompts: store.prompts.clone(),
        history: store.history.clone(),
        warnings: store.warnings.clone(),
        platform: std::env::consts::OS.into(),
        chatgpt_accounts: state.chatgpt.accounts()?,
    })
}

#[tauri::command]
fn save_profile(
    state: State<'_, AppState>,
    mut profile: Profile,
    api_key: Option<String>,
    clear_key: bool,
) -> Result<(), String> {
    if profile.provider == "chatgpt" || state.chatgpt.accounts()?.iter().any(|a| a.id == profile.id)
    {
        return Err("请在 ChatGPT 订阅页管理此账户与模型。".into());
    }
    provider::validate_profile(&profile)?;
    if profile.id.len() > 80
        || profile.id.is_empty()
        || !profile
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("配置 ID 无效".into());
    }
    let mut store = lock(&state.store)?;
    profile.has_key = store
        .settings
        .profiles
        .iter()
        .find(|p| p.id == profile.id)
        .is_some_and(|p| p.has_key);
    if clear_key {
        delete_key(&profile.id)?;
        profile.has_key = false;
    }
    if let Some(key) = api_key.filter(|s| !s.trim().is_empty()) {
        credential(&profile.id)?
            .set_password(key.trim())
            .map_err(|_| "API Key 未保存：无法写入系统凭据库，请检查权限后重试。")?;
        profile.has_key = true;
    }
    let mut settings = store.settings.clone();
    if let Some(existing) = settings.profiles.iter_mut().find(|p| p.id == profile.id) {
        *existing = profile.clone();
    } else {
        settings.profiles.push(profile.clone());
    }
    if settings.active_profile_id.is_empty() {
        settings.active_profile_id = profile.id;
    }
    store.save_settings(settings)
}
#[tauri::command]
fn delete_profile(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if state.chatgpt.accounts()?.iter().any(|a| a.id == id) {
        return Err("请在 ChatGPT 订阅页退出账户。注册信息将保留以便下次登录。".into());
    }
    let mut store = lock(&state.store)?;
    if !store.settings.profiles.iter().any(|p| p.id == id) {
        return Err("模型配置不存在".into());
    }
    delete_key(&id)?;
    let mut settings = store.settings.clone();
    settings.profiles.retain(|p| p.id != id);
    if settings.active_profile_id == id {
        settings.active_profile_id = settings
            .profiles
            .first()
            .map(|p| p.id.clone())
            .unwrap_or_default();
    }
    store.save_settings(settings)
}
#[tauri::command]
fn set_preferences(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    profile_id: String,
    prompt_id: String,
    target_language: String,
    shortcut: String,
    record_history: bool,
) -> Result<(), String> {
    render_prompt("{{target_language}}", &target_language)?;
    let mut store = lock(&state.store)?;
    if !profile_id.is_empty() && !store.settings.profiles.iter().any(|p| p.id == profile_id) {
        return Err("模型配置不存在".into());
    }
    if !store.prompts.iter().any(|p| p.id == prompt_id) {
        return Err("提示词不存在".into());
    }
    let old_shortcut = store.settings.shortcut.clone();
    if shortcut != old_shortcut {
        app.global_shortcut()
            .register(shortcut.as_str())
            .map_err(|_| "快捷键无效或已被其他软件占用，请换一个组合。")?;
    }
    let mut settings = store.settings.clone();
    settings.active_profile_id = profile_id;
    settings.active_prompt_id = prompt_id;
    settings.target_language = target_language;
    settings.shortcut = shortcut.clone();
    settings.record_history = record_history;
    if let Err(error) = store.save_settings(settings) {
        if shortcut != old_shortcut {
            let _ = app.global_shortcut().unregister(shortcut.as_str());
        }
        return Err(error);
    }
    if shortcut != old_shortcut {
        let _ = app.global_shortcut().unregister(old_shortcut.as_str());
    }
    if app.global_shortcut().is_registered(shortcut.as_str()) {
        store.warnings.retain(|w| !w.starts_with("全局快捷键"));
    }
    Ok(())
}
#[tauri::command]
fn save_prompt(state: State<'_, AppState>, mut prompt: Prompt) -> Result<(), String> {
    if prompt.name.trim().is_empty() || prompt.id.is_empty() {
        return Err("提示词名称不能为空".into());
    }
    render_prompt(&prompt.content, "auto")?;
    prompt.builtin = default_prompts().iter().any(|p| p.id == prompt.id);
    let mut store = lock(&state.store)?;
    let mut prompts = store.prompts.clone();
    if let Some(existing) = prompts.iter_mut().find(|p| p.id == prompt.id) {
        *existing = prompt;
    } else {
        prompts.push(prompt);
    }
    store.save_prompts(prompts)
}
#[tauri::command]
fn restore_prompt(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let default = default_prompts()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or("仅内置提示词可恢复默认")?;
    let mut store = lock(&state.store)?;
    let mut prompts = store.prompts.clone();
    if let Some(prompt) = prompts.iter_mut().find(|p| p.id == id) {
        *prompt = default;
    }
    store.save_prompts(prompts)
}
#[tauri::command]
fn delete_prompt(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if default_prompts().iter().any(|p| p.id == id) {
        return Err("内置提示词不能删除，可以编辑或恢复默认。".into());
    }
    let mut store = lock(&state.store)?;
    let mut prompts = store.prompts.clone();
    prompts.retain(|p| p.id != id);
    store.save_prompts(prompts)?;
    if store.settings.active_prompt_id == id {
        let mut settings = store.settings.clone();
        settings.active_prompt_id = "general".into();
        store.save_settings(settings)?;
    }
    Ok(())
}
#[tauri::command]
fn preview_prompt(content: String, target_language: String) -> Result<String, String> {
    render_prompt(&content, &target_language)
}
#[tauri::command]
fn delete_history(state: State<'_, AppState>, id: Option<String>) -> Result<(), String> {
    let mut store = lock(&state.store)?;
    let history = match id {
        Some(id) => store
            .history
            .iter()
            .filter(|h| h.id != id)
            .cloned()
            .collect(),
        None => vec![],
    };
    store.save_history(history)
}

#[tauri::command]
fn cancel_translation(state: State<'_, AppState>, request_id: String) -> Result<(), String> {
    state.requests.cancel(&request_id)
}

#[tauri::command]
async fn start_translation(
    state: State<'_, AppState>,
    request: TranslateRequest,
    on_event: Channel<TranslationEvent>,
) -> Result<(), String> {
    if request.text.trim().is_empty() {
        return Err("请输入需要翻译的文字".into());
    }
    let (profile, prompt) = {
        let store = lock(&state.store)?;
        (
            store
                .settings
                .profiles
                .iter()
                .find(|p| p.id == request.profile_id)
                .cloned()
                .ok_or("请先配置模型接口")?,
            store
                .prompts
                .iter()
                .find(|p| p.id == request.prompt_id)
                .cloned()
                .ok_or("请选择提示词模板")?,
        )
    };
    let instruction = render_prompt(&prompt.content, &request.target_language)?;
    let cancel = state.requests.begin(request.request_id.clone())?;
    let _ = on_event.send(TranslationEvent::new(
        &request.request_id,
        "started",
        "",
        "",
    ));
    let work = async {
        let key = if profile.provider == "chatgpt" {
            state
                .chatgpt
                .access_token_task(profile.id.clone())
                .await
                .map_err(|_| "ChatGPT 登录续期任务未完成")??
        } else {
            read_key(&profile)?
        };
        provider::translate(
            &profile,
            &key,
            &instruction,
            &request.text,
            false,
            cancel.clone(),
            |text| {
                if !cancel.is_cancelled() {
                    let _ = on_event.send(TranslationEvent::new(
                        &request.request_id,
                        "delta",
                        text,
                        "",
                    ));
                }
            },
        )
        .await
    };
    let result = tokio::select! { biased; _ = cancel.cancelled() => Err("请求已取消".into()), result = work => result };
    // Serialize the commit against begin(): superseded requests can neither commit history nor emit a successful result.
    let mut active = lock(&state.requests.0)?;
    if !active.as_ref().is_some_and(|a| a.id == request.request_id) {
        return Ok(());
    }
    let terminal = if cancel.is_cancelled() {
        TranslationEvent::new(
            &request.request_id,
            "cancelled",
            "",
            "已停止生成，保留部分译文",
        )
    } else {
        match result {
            Ok(text) => {
                let warning = lock(&state.store)?
                    .append(HistoryEntry {
                        id: uuid::Uuid::new_v4().to_string(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                        source: request.text,
                        translation: text.clone(),
                        profile_name: profile.name,
                        model: profile.model,
                        prompt_name: prompt.name,
                        target_language: request.target_language,
                    })
                    .err()
                    .unwrap_or_default();
                TranslationEvent::new(&request.request_id, "completed", &text, &warning)
            }
            Err(error) => TranslationEvent::new(&request.request_id, "error", "", &error),
        }
    };
    *active = None;
    let _ = on_event.send(terminal);
    Ok(())
}

#[tauri::command]
async fn test_connection(state: State<'_, AppState>, profile_id: String) -> Result<String, String> {
    let profile = lock(&state.store)?
        .settings
        .profiles
        .iter()
        .find(|p| p.id == profile_id)
        .cloned()
        .ok_or("请先保存模型配置")?;
    let id = format!("probe-{}", uuid::Uuid::new_v4());
    let cancel = state.requests.begin(id.clone())?;
    let work = async {
        let key = if profile.provider == "chatgpt" {
            state
                .chatgpt
                .access_token_task(profile.id.clone())
                .await
                .map_err(|_| "ChatGPT 登录续期任务未完成")??
        } else {
            read_key(&profile)?
        };
        provider::translate(
            &profile,
            &key,
            "Translate the user text into Simplified Chinese. Output only the translation.",
            "Hello",
            true,
            cancel.clone(),
            |_| {},
        )
        .await
    };
    let result = tokio::select! { biased; _ = cancel.cancelled() => Err("请求已取消".into()), result = work => result };
    let mut active = lock(&state.requests.0)?;
    if active.as_ref().is_some_and(|a| a.id == id) {
        *active = None;
    }
    result.map(|_| "连接成功 · 模型已返回测试译文".into())
}

#[tauri::command]
async fn chatgpt_login(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    account_id: Option<String>,
    consent: bool,
) -> Result<chatgpt::LoginResult, String> {
    state
        .chatgpt
        .login(account_id, consent, |url| {
            app.opener()
                .open_url(url, None::<&str>)
                .map_err(|_| "无法打开系统浏览器，请检查默认浏览器设置。".into())
        })
        .await
}
#[tauri::command]
fn cancel_chatgpt_login(state: State<'_, AppState>) -> Result<(), String> {
    state.chatgpt.cancel_login()
}
#[tauri::command]
async fn chatgpt_models(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<chatgpt::Model>, String> {
    state.chatgpt.list_models(&account_id).await
}
#[tauri::command]
fn select_chatgpt_model(
    state: State<'_, AppState>,
    account_id: String,
    model: String,
) -> Result<(), String> {
    let account = state.chatgpt.selected_account(&account_id, &model)?;
    let mut store = lock(&state.store)?;
    let mut settings = store.settings.clone();
    let profile = Profile {
        id: account.id.clone(),
        name: format!("ChatGPT · {}", account.label()),
        provider: "chatgpt".into(),
        protocol: Protocol::Responses,
        base_url: chatgpt::RESOURCE.into(),
        model,
        timeout_seconds: 120,
        has_key: false,
    };
    if let Some(old) = settings.profiles.iter_mut().find(|p| p.id == profile.id) {
        *old = profile;
    } else {
        settings.profiles.push(profile);
    }
    settings.active_profile_id = account.id;
    store.save_settings(settings)
}
#[tauri::command]
async fn chatgpt_logout(state: State<'_, AppState>, account_id: String) -> Result<String, String> {
    state.requests.cancel_all()?;
    let message = state.chatgpt.logout(&account_id).await?;
    let mut store = lock(&state.store)?;
    let mut settings = store.settings.clone();
    settings.profiles.retain(|p| p.id != account_id);
    if settings.active_profile_id == account_id {
        settings.active_profile_id = String::new();
    }
    store.save_settings(settings)?;
    Ok(message)
}
#[tauri::command]
fn acknowledge_chatgpt_usage(state: State<'_, AppState>, account_id: String) -> Result<(), String> {
    state.chatgpt.acknowledge(&account_id)
}
#[tauri::command]
fn open_chatgpt_usage(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url("https://chatgpt.com/settings/usage", None::<&str>)
        .map_err(|_| "无法打开系统浏览器".into())
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state
            .activations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = window.emit("focus-input", ());
    }
}

pub fn run() {
    if smoke::credential_probe() {
        return;
    }
    let mut context = tauri::generate_context!();
    // Explicit diagnostics use their own single-instance identity and never activate a user's app.
    if std::env::args()
        .any(|arg| matches!(arg.as_str(), "--smoke-test" | "--smoke-peer" | "--smoke-ui"))
    {
        context.config_mut().identifier.push_str(".smoke");
        if let Some(window) = context.config_mut().app.windows.first_mut() {
            window.title = "TermTranslate · 验证窗口".into();
        }
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_main(app)
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state == ShortcutState::Pressed {
                        show_main(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            let root = std::env::var_os("TERMTRANSLATE_DATA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or(app.path().app_data_dir()?);
            let mut store = Store::open(root.clone()).map_err(std::io::Error::other)?;
            if app
                .global_shortcut()
                .register(store.settings.shortcut.as_str())
                .is_err()
            {
                store.warnings.push(
                    "全局快捷键注册失败：可能已被占用，请到设置中更换；仍可从托盘打开。".into(),
                );
            }
            app.manage(AppState {
                store: Mutex::new(store),
                chatgpt: Arc::new(chatgpt::ChatGpt::open(root).map_err(std::io::Error::other)?),
                requests: RequestGate::default(),
                activations: std::sync::atomic::AtomicUsize::new(0),
            });
            let open = MenuItem::with_id(app, "open", "打开翻译", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出 TermTranslate", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &settings, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main-tray")
                .tooltip("TermTranslate · 翻译触手可及")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main(app),
                    "settings" => {
                        show_main(app);
                        let _ = app.emit("open-settings", ());
                    }
                    "quit" => {
                        if let Some(state) = app.try_state::<AppState>() {
                            if let Ok(active) = state.requests.0.lock() {
                                if let Some(active) = active.as_ref() {
                                    active.cancel.cancel();
                                }
                            }
                        }
                        app.exit(0);
                    }
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            if std::env::args().any(|arg| arg == "--smoke-test") {
                let app = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    smoke::run(app).await;
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            save_profile,
            delete_profile,
            set_preferences,
            save_prompt,
            restore_prompt,
            delete_prompt,
            preview_prompt,
            delete_history,
            start_translation,
            cancel_translation,
            test_connection,
            chatgpt_login,
            cancel_chatgpt_login,
            chatgpt_models,
            select_chatgpt_model,
            chatgpt_logout,
            acknowledge_chatgpt_usage,
            open_chatgpt_usage
        ])
        .run(context)
        .expect("TermTranslate failed to start");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newer_request_cancels_previous_and_old_cancel_is_ignored() {
        let gate = RequestGate::default();
        let first = gate.begin("first".into()).unwrap();
        let second = gate.begin("second".into()).unwrap();
        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
        gate.cancel("first").unwrap();
        assert!(!second.is_cancelled());
        gate.cancel("second").unwrap();
        assert!(second.is_cancelled());
    }
}
