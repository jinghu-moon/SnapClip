//! User path shortcuts. The app owns persistence; tags only choose a directory.
use gpui_kit::Global;
use std::path::{Path, PathBuf};

#[derive(Clone, Default)]
pub(super) struct DirectoryPresets {
    pub paths: Vec<PathBuf>,
    pub writing: bool,
    // None is an isolated in-memory store for tests.
    file: Option<PathBuf>,
}

impl Global for DirectoryPresets {}

impl DirectoryPresets {
    pub fn load(file: PathBuf) -> Self {
        let paths = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<PathBuf>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|path| path.is_absolute())
            .collect();
        Self {
            paths,
            writing: false,
            file: Some(file),
        }
    }

    /// Runs on the background executor. Only publish the returned snapshot once
    /// its write succeeds; neither invalid paths nor failed writes add a tag.
    pub fn add(mut self, text: &str) -> Result<Option<Self>, String> {
        let path = PathBuf::from(text.trim());
        if !path.is_absolute() || !path.is_dir() {
            return Err("请先输入有效的绝对文件夹路径，再添加预设".into());
        }
        let key = directory_key(&path);
        let builtins = [
            crate::save::dirs::desktop(),
            crate::save::dirs::pictures(),
            crate::save::dirs::downloads(),
        ];
        if self.paths.iter().any(|other| directory_key(other) == key)
            || builtins
                .iter()
                .any(|other| !other.is_empty() && directory_key(Path::new(other)) == key)
        {
            return Ok(None);
        }
        self.paths.push(path);
        if let Some(file) = &self.file {
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("无法保存路径预设：{e}"))?;
            }
            let bytes = serde_json::to_vec_pretty(&self.paths)
                .map_err(|e| format!("无法保存路径预设：{e}"))?;
            let temporary = file.with_extension("json.tmp");
            std::fs::write(&temporary, bytes).map_err(|e| format!("无法保存路径预设：{e}"))?;
            if let Err(error) = std::fs::rename(&temporary, file) {
                let _ = std::fs::remove_file(&temporary);
                return Err(format!("无法保存路径预设：{error}"));
            }
        }
        Ok(Some(self))
    }
}

fn directory_key(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    resolved
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn root() -> PathBuf {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "snapclip-path-presets-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn added_paths_survive_reload_and_cannot_be_duplicated() {
        let root = root();
        let first = root.join("first");
        let second = root.join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let file = root.join("presets.json");
        let stored = DirectoryPresets::load(file.clone())
            .add(&first.to_string_lossy())
            .unwrap()
            .unwrap()
            .add(&second.to_string_lossy())
            .unwrap()
            .unwrap();
        let reloaded = DirectoryPresets::load(file);
        assert_eq!(reloaded.paths, [first.clone(), second]);
        assert_eq!(reloaded.paths, stored.paths);
        let duplicate = format!("{}\\", first.to_string_lossy().to_uppercase());
        assert!(reloaded.add(&duplicate).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_paths_and_failed_writes_do_not_create_presets() {
        let root = root();
        let blocked = root.join("file");
        std::fs::write(&blocked, b"unchanged").unwrap();
        for path in ["", "relative", blocked.to_str().unwrap()] {
            assert!(DirectoryPresets::default().add(path).is_err());
        }
        let store = DirectoryPresets::load(blocked.join("presets.json"));
        assert!(store.clone().add(root.to_str().unwrap()).is_err());
        assert!(store.paths.is_empty());
        assert_eq!(std::fs::read(&blocked).unwrap(), b"unchanged");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn builtin_paths_are_not_added_as_user_presets() {
        assert!(
            DirectoryPresets::default()
                .add(&crate::save::dirs::desktop())
                .unwrap()
                .is_none()
        );
    }
}
