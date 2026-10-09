//! Source-app icons for history rows.
//!
//! Extraction goes through `win-icon-extractor`, so the shell does not grow a second way to
//! read an executable's icon. The extractor caches PNGs on disk by executable path; this type
//! adds the in-memory path cache, because a list row asks for the same handful of executables
//! over and over.
//!
//! GPUI loads images from a path, so the shell hands the cached PNG's *path* to `img()`
//! rather than image bytes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use win_icon_extractor::{IconCache, ImageFormat};

pub struct SourceIcons {
    icon_cache: IconCache,
    /// exe path → extracted PNG path (32px).
    paths: Mutex<HashMap<String, Option<PathBuf>>>,
}

impl SourceIcons {
    pub fn new(cache_dir: PathBuf) -> Result<Self, String> {
        let icon_cache = IconCache::builder(cache_dir)
            .format(ImageFormat::Png)
            .build()
            .map_err(|error| format!("init icon cache failed: {error}"))?;
        Ok(Self {
            icon_cache,
            paths: Mutex::new(HashMap::new()),
        })
    }

    /// Cached PNG for `exe_path`, or `None` when there is nothing to show.
    ///
    /// A row for a clipboard entry whose source is unknown, or whose executable is gone,
    /// simply renders without an icon — that is a normal state, not an error.
    pub fn png_path(&self, exe_path: &str) -> Option<PathBuf> {
        let exe_path = exe_path.trim();
        if exe_path.is_empty() {
            return None;
        }
        if let Ok(cache) = self.paths.lock() {
            if let Some(hit) = cache.get(exe_path) {
                return hit.clone();
            }
        }
        let extracted = self
            .icon_cache
            .extract_to_file_sized(exe_path, 32)
            .ok()
            .filter(|path| path.exists());
        if let Ok(mut cache) = self.paths.lock() {
            cache.insert(exe_path.to_string(), extracted.clone());
        }
        extracted
    }
}
