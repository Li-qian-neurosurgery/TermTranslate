use crate::model::{default_prompts, HistoryEntry, Prompt, Settings};
use atomicwrites::{AllowOverwrite, AtomicFile};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document<T> {
    schema_version: u32,
    items: Vec<T>,
}

pub struct Store {
    pub root: PathBuf,
    pub settings: Settings,
    pub prompts: Vec<Prompt>,
    pub history: Vec<HistoryEntry>,
    pub warnings: Vec<String>,
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|_| "无法创建应用数据目录")?;
        let mut warnings = vec![];
        let settings: Settings = load_or_recover(
            &root.join("settings.json"),
            Settings::default(),
            &mut warnings,
        )?;
        if settings.schema_version != 1 {
            return Err("配置来自更新版本的软件，请使用新版 TermTranslate 打开".into());
        }
        let prompts: Document<Prompt> = load_or_recover(
            &root.join("prompts.json"),
            Document {
                schema_version: 1,
                items: default_prompts(),
            },
            &mut warnings,
        )?;
        let history: Document<HistoryEntry> = load_or_recover(
            &root.join("history.json"),
            Document {
                schema_version: 1,
                items: vec![],
            },
            &mut warnings,
        )?;
        if prompts.schema_version != 1 || history.schema_version != 1 {
            return Err("本地数据版本过新，请使用新版 TermTranslate".into());
        }
        let mut store = Self {
            root,
            settings,
            prompts: prompts.items,
            history: history.items,
            warnings,
        };
        for prompt in default_prompts() {
            if !store.prompts.iter().any(|p| p.id == prompt.id) {
                store.prompts.push(prompt);
            }
        }
        store.history.truncate(100);
        Ok(store)
    }

    pub fn save_settings(&mut self, settings: Settings) -> Result<(), String> {
        atomic_json(&self.root.join("settings.json"), &settings)?;
        self.settings = settings;
        Ok(())
    }
    pub fn save_prompts(&mut self, prompts: Vec<Prompt>) -> Result<(), String> {
        atomic_json(
            &self.root.join("prompts.json"),
            &Document {
                schema_version: 1,
                items: prompts.clone(),
            },
        )?;
        self.prompts = prompts;
        Ok(())
    }
    pub fn save_history(&mut self, history: Vec<HistoryEntry>) -> Result<(), String> {
        let history: Vec<_> = history.into_iter().take(100).collect();
        atomic_json(
            &self.root.join("history.json"),
            &Document {
                schema_version: 1,
                items: history.clone(),
            },
        )?;
        self.history = history;
        Ok(())
    }
    pub fn append(&mut self, entry: HistoryEntry) -> Result<(), String> {
        if !self.settings.record_history {
            return Ok(());
        }
        let mut history = self.history.clone();
        history.insert(0, entry);
        self.save_history(history)
    }
}

fn load_or_recover<T: DeserializeOwned>(
    path: &Path,
    fallback: T,
    warnings: &mut Vec<String>,
) -> Result<T, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(fallback),
        Err(_) => return Err("无法读取本地数据，请检查数据目录权限".into()),
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) => Ok(value),
        Err(_) => {
            let backup = path.with_extension(format!("corrupt-{}.json", uuid::Uuid::new_v4()));
            fs::rename(path, backup).map_err(|_| "数据文件损坏，且无法保存恢复副本")?;
            warnings.push(format!(
                "{} 已损坏，原文件已另存为 corrupt 副本，本次使用默认值。",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
            Ok(fallback)
        }
    }
}

pub fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(value).map_err(|_| "无法序列化本地数据")?;
    AtomicFile::new(path, AllowOverwrite)
        .write(|file| {
            file.write_all(&data)?;
            file.sync_all()
        })
        .map_err(|_| "本地保存失败，请检查磁盘空间和目录权限".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_retention_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().into()).unwrap();
        for n in 0..105 {
            store
                .append(HistoryEntry {
                    id: n.to_string(),
                    created_at: "now".into(),
                    source: "原文".into(),
                    translation: "result".into(),
                    profile_name: "test".into(),
                    model: "m".into(),
                    prompt_name: "p".into(),
                    target_language: "en".into(),
                })
                .unwrap();
        }
        assert_eq!(store.history.len(), 100);
        assert_eq!(store.history[0].id, "104");
        assert_eq!(Store::open(dir.path().into()).unwrap().history.len(), 100);
        let mut settings = store.settings.clone();
        settings.record_history = false;
        store.save_settings(settings).unwrap();
        let mut duplicate = store.history[0].clone();
        duplicate.id = "should-not-record".into();
        store.append(duplicate).unwrap();
        assert!(!store.history.iter().any(|h| h.id == "should-not-record"));
        store.save_history(vec![]).unwrap();
        assert!(
            !Store::open(dir.path().into())
                .unwrap()
                .settings
                .record_history
        );
        assert!(Store::open(dir.path().into()).unwrap().history.is_empty());
    }
    #[test]
    fn corrupt_file_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.json"), "broken").unwrap();
        let store = Store::open(dir.path().into()).unwrap();
        assert_eq!(store.warnings.len(), 1);
        assert!(fs::read_dir(dir.path()).unwrap().any(|f| f
            .unwrap()
            .path()
            .to_string_lossy()
            .contains("corrupt-")));
    }
    #[test]
    fn edited_prompts_survive_restart_and_default_can_be_restored() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().into()).unwrap();
        let mut prompts = store.prompts.clone();
        prompts[0].content = "Custom instruction: {{target_language}}".into();
        store.save_prompts(prompts).unwrap();
        let mut reloaded = Store::open(dir.path().into()).unwrap();
        assert_eq!(
            crate::model::render_prompt(&reloaded.prompts[0].content, "en").unwrap(),
            "Custom instruction: 英语"
        );
        reloaded.save_prompts(default_prompts()).unwrap();
        assert_eq!(
            Store::open(dir.path().into()).unwrap().prompts[0].content,
            default_prompts()[0].content
        );
    }
}
