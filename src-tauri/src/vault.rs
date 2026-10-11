//! OS credential storage with an atomic generation pointer. Windows credentials have a
//! per-entry size limit, so OAuth token sets are split into bounded binary entries.
use serde::{Deserialize, Serialize};

pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String>;
    fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String>;
    fn remove(&self, name: &str) -> Result<(), String>;
}
pub struct SystemStore;
impl SecretStore for SystemStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        match crate::credential(name)?.get_secret() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("无法读取 ChatGPT 系统凭据，请解锁凭据库后重试。".into()),
        }
    }
    fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        crate::credential(name)?
            .set_secret(bytes)
            .map_err(|_| "无法保存 ChatGPT 系统凭据，请检查凭据库权限。".into())
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        crate::delete_key(name)
    }
}

#[derive(Serialize, Deserialize)]
struct Pointer {
    generation: String,
    count: usize,
}
fn pointer_name(id: &str) -> String {
    format!("chatgpt-{id}-current")
}
fn chunk_name(id: &str, p: &Pointer, index: usize) -> String {
    format!("chatgpt-{id}-{}-{index}", p.generation)
}
fn pointer(vault: &dyn SecretStore, id: &str) -> Result<Option<Pointer>, String> {
    vault
        .get(&pointer_name(id))?
        .map(|data| {
            let p: Pointer =
                serde_json::from_slice(&data).map_err(|_| "ChatGPT 凭据索引损坏，请重新登录。")?;
            if p.count == 0 || p.count > 128 || uuid::Uuid::parse_str(&p.generation).is_err() {
                return Err("ChatGPT 凭据索引无效，请重新登录。".into());
            }
            Ok(p)
        })
        .transpose()
}
pub fn read<T: serde::de::DeserializeOwned>(
    vault: &dyn SecretStore,
    id: &str,
) -> Result<Option<T>, String> {
    let Some(p) = pointer(vault, id)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    for index in 0..p.count {
        bytes.extend(
            vault
                .get(&chunk_name(id, &p, index))?
                .ok_or("ChatGPT 凭据不完整，请重新登录。")?,
        );
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "ChatGPT 凭据无法读取，请重新登录。".into())
}
pub fn write<T: Serialize>(vault: &dyn SecretStore, id: &str, value: &T) -> Result<(), String> {
    let old = pointer(vault, id)?;
    let bytes = serde_json::to_vec(value).map_err(|_| "无法序列化 ChatGPT 凭据")?;
    if bytes.len() > 128 * 2400 {
        return Err("ChatGPT 凭据大小异常".into());
    }
    let p = Pointer {
        generation: uuid::Uuid::new_v4().to_string(),
        count: bytes.len().div_ceil(2400),
    };
    for (index, chunk) in bytes.chunks(2400).enumerate() {
        if let Err(error) = vault.put(&chunk_name(id, &p, index), chunk) {
            for n in 0..=index {
                let _ = vault.remove(&chunk_name(id, &p, n));
            }
            return Err(error);
        }
    }
    let encoded = serde_json::to_vec(&p).map_err(|_| "无法序列化凭据索引")?;
    if let Err(error) = vault.put(&pointer_name(id), &encoded) {
        for n in 0..p.count {
            let _ = vault.remove(&chunk_name(id, &p, n));
        }
        return Err(error);
    }
    if let Some(old) = old {
        for n in 0..old.count {
            let _ = vault.remove(&chunk_name(id, &old, n));
        }
    }
    Ok(())
}
pub fn clear(vault: &dyn SecretStore, id: &str) -> Result<(), String> {
    let p = pointer(vault, id)?;
    if let Some(p) = p {
        // Keep the pointer until all referenced entries have been removed, so a retry can finish cleanup.
        for n in 0..p.count {
            vault.remove(&chunk_name(id, &p, n))?;
        }
    }
    vault.remove(&pointer_name(id))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};
    #[derive(Default)]
    pub struct MemoryStore {
        pub values: Mutex<HashMap<String, Vec<u8>>>,
        pub fail: Mutex<bool>,
    }
    impl SecretStore for MemoryStore {
        fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.values.lock().unwrap().get(name).cloned())
        }
        fn put(&self, name: &str, value: &[u8]) -> Result<(), String> {
            if *self.fail.lock().unwrap() {
                return Err("simulated credential write failure".into());
            }
            assert!(value.len() <= 2400);
            self.values
                .lock()
                .unwrap()
                .insert(name.into(), value.into());
            Ok(())
        }
        fn remove(&self, name: &str) -> Result<(), String> {
            self.values.lock().unwrap().remove(name);
            Ok(())
        }
    }
    #[test]
    fn large_tokens_rotate_atomically_and_accounts_are_isolated() {
        let vault = MemoryStore::default();
        let old = "中文token".repeat(1200);
        write(&vault, "one", &old).unwrap();
        write(&vault, "two", &"other account").unwrap();
        assert_eq!(read::<String>(&vault, "one").unwrap(), Some(old.clone()));
        *vault.fail.lock().unwrap() = true;
        assert!(write(&vault, "one", &"new").is_err());
        assert_eq!(read::<String>(&vault, "one").unwrap(), Some(old));
        *vault.fail.lock().unwrap() = false;
        write(&vault, "one", &"rotated").unwrap();
        assert_eq!(
            read::<String>(&vault, "one").unwrap(),
            Some("rotated".into())
        );
        clear(&vault, "one").unwrap();
        assert!(read::<String>(&vault, "one").unwrap().is_none());
        assert_eq!(
            read::<String>(&vault, "two").unwrap(),
            Some("other account".into())
        );
    }
}
