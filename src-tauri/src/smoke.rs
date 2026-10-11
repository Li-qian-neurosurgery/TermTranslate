//! Explicit developer-only diagnostics. Never runs during a normal launch.
use crate::{credential, delete_key, lock, show_main, storage::atomic_json, AppState};
use serde_json::json;
use std::{sync::atomic::Ordering, time::Duration};
use tauri::Manager;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

pub fn credential_probe() -> bool {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--smoke-oauth-credential") {
        let ok = args.len() == 3
            && args[2].starts_with("smoke-")
            && crate::vault::read::<String>(&crate::vault::SystemStore, &args[2])
                .is_ok_and(|value| value.as_deref() == Some(&oauth_sample()));
        std::process::exit(if ok { 0 } else { 1 });
    }
    if args.get(1).map(String::as_str) != Some("--smoke-credential") {
        return false;
    }
    let ok = args.len() == 4
        && args[2].starts_with("smoke-")
        && credential(&args[2])
            .and_then(|e| e.get_password().map_err(|_| "read failed".into()))
            .is_ok_and(|value| value == args[3]);
    std::process::exit(if ok { 0 } else { 1 });
}

pub async fn run(app: tauri::AppHandle) {
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut checks = serde_json::Map::new();
    let state = app.state::<AppState>();
    let root = match lock(&state.store) {
        Ok(s) => s.root.clone(),
        Err(_) => {
            app.exit(1);
            return;
        }
    };
    if let Some(window) = app.get_webview_window("main") {
        checks.insert(
            "window_created".into(),
            json!(window.is_visible().unwrap_or(false)),
        );
        let _ = window.hide();
        tokio::time::sleep(Duration::from_millis(100)).await;
        checks.insert(
            "window_hide".into(),
            json!(!window.is_visible().unwrap_or(true)),
        );
        show_main(&app);
        tokio::time::sleep(Duration::from_millis(100)).await;
        checks.insert(
            "window_restore".into(),
            json!(window.is_visible().unwrap_or(false)),
        );
        checks.insert(
            "tray_created".into(),
            json!(app.tray_by_id("main-tray").is_some()),
        );
        let shortcut = lock(&state.store)
            .map(|s| s.settings.shortcut.clone())
            .unwrap_or_default();
        checks.insert(
            "shortcut_registered".into(),
            json!(app.global_shortcut().is_registered(shortcut.as_str())),
        );
        let duplicate = app.global_shortcut().register(shortcut.as_str());
        checks.insert(
            "shortcut_conflict_detected".into(),
            json!(duplicate.is_err()),
        );
        let before = state.activations.load(Ordering::SeqCst);
        if let Ok(exe) = std::env::current_exe() {
            match std::process::Command::new(&exe).arg("--smoke-peer").spawn() {
                Ok(mut child) => {
                    let mut exited = false;
                    for _ in 0..50 {
                        if child.try_wait().ok().flatten().is_some() {
                            exited = true;
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    if !exited {
                        let _ = child.kill();
                    }
                    checks.insert(
                        "single_instance".into(),
                        json!(exited && state.activations.load(Ordering::SeqCst) > before),
                    );
                }
                Err(_) => {
                    checks.insert("single_instance".into(), json!(false));
                }
            }
        }
        // Do not overwrite a non-text clipboard whose contents cannot be restored by this API.
        if let Ok(previous) = app.clipboard().read_text() {
            let sample = "TermTranslate 剪贴板测试🙂";
            let ok = app.clipboard().write_text(sample).is_ok()
                && app.clipboard().read_text().is_ok_and(|text| text == sample);
            let restored = app.clipboard().write_text(previous).is_ok();
            checks.insert(
                "clipboard_roundtrip_and_restore".into(),
                json!(ok && restored),
            );
        } else {
            checks.insert(
                "clipboard_roundtrip_and_restore".into(),
                json!("skipped: non-text clipboard"),
            );
        }
    }
    let id = format!("smoke-{}", uuid::Uuid::new_v4());
    let value = uuid::Uuid::new_v4().to_string();
    let saved = credential(&id).is_ok_and(|entry| entry.set_password(&value).is_ok());
    checks.insert("credential_write".into(), json!(saved));
    let read_back = credential(&id)
        .and_then(|entry| entry.get_password().map_err(|_| "read failed".into()))
        .is_ok_and(|text| text == value);
    checks.insert("credential_read".into(), json!(read_back));
    let restart_read = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            std::process::Command::new(exe)
                .args(["--smoke-credential", &id, &value])
                .status()
                .ok()
        })
        .is_some_and(|status| status.success());
    checks.insert(
        "credential_read_in_fresh_process".into(),
        json!(restart_read),
    );
    let removed = delete_key(&id).is_ok();
    checks.insert("credential_cleanup".into(), json!(removed));
    let oauth_id = format!("smoke-{}", uuid::Uuid::new_v4());
    let oauth_value = oauth_sample();
    let vault = crate::vault::SystemStore;
    checks.insert(
        "oauth_large_credential_write".into(),
        json!(crate::vault::write(&vault, &oauth_id, &oauth_value).is_ok()),
    );
    checks.insert(
        "oauth_large_credential_read".into(),
        json!(crate::vault::read::<String>(&vault, &oauth_id)
            .is_ok_and(|value| value.as_deref() == Some(oauth_value.as_str()))),
    );
    let fresh_read = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            std::process::Command::new(exe)
                .args(["--smoke-oauth-credential", &oauth_id])
                .status()
                .ok()
        })
        .is_some_and(|status| status.success());
    checks.insert(
        "oauth_large_credential_fresh_process".into(),
        json!(fresh_read),
    );
    let cleared = crate::vault::clear(&vault, &oauth_id).is_ok()
        && crate::vault::read::<String>(&vault, &oauth_id).is_ok_and(|value| value.is_none());
    checks.insert("oauth_large_credential_cleanup".into(), json!(cleared));
    let passed = checks.values().all(|v| v.as_bool().unwrap_or(true));
    let report = json!({ "passed": passed, "platform": std::env::consts::OS, "checkedAt": chrono::Utc::now().to_rfc3339(), "checks": checks });
    let _ = atomic_json(&root.join("native-smoke.json"), &report);
    app.exit(if passed { 0 } else { 1 });
}

fn oauth_sample() -> String {
    "TermTranslate OAuth 测试凭据🙂".repeat(500)
}
