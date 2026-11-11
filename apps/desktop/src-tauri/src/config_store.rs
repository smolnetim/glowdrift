//! Persistence of the user configuration as `config.json`.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use glowdrift_core::Config;

const FILE_NAME: &str = "config.json";
const BACKUP_NAME: &str = "config.json.bak";
const TEMP_NAME: &str = "config.json.tmp";
const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Reads and writes the configuration file inside a directory.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    dir: PathBuf,
}

impl ConfigStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    /// Loads the stored configuration, validated.
    ///
    /// A missing file yields the defaults. A file that cannot be read as a configuration is
    /// copied to `config.json.bak` and the defaults are returned.
    pub fn load(&self) -> Config {
        let path = self.path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Config::default(),
            Err(e) => {
                log::warn!("cannot read {}: {e}", path.display());
                return Config::default();
            }
        };
        let json = bytes.strip_prefix(UTF8_BOM).unwrap_or(&bytes);
        match serde_json::from_slice::<Config>(json) {
            Ok(config) => config.validated(),
            Err(e) => {
                log::warn!("config file is corrupt ({e}); keeping a backup and using defaults");
                if let Err(e) = fs::copy(&path, self.dir.join(BACKUP_NAME)) {
                    log::warn!("cannot back up the corrupt config: {e}");
                }
                Config::default()
            }
        }
    }

    /// Writes the configuration atomically: temp file in the same directory, then rename.
    pub fn save(&self, config: &Config) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let temp = self.dir.join(TEMP_NAME);
        let json = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
        write_synced(&temp, &json)?;
        fs::rename(&temp, self.path()).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "glowdrift-config-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_file_yields_defaults() {
        let tmp = TempDir::new();
        assert_eq!(ConfigStore::new(&tmp.0).load(), Config::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let tmp = TempDir::new();
        let store = ConfigStore::new(tmp.0.join("nested"));
        let config = Config {
            onboarding_complete: true,
            monitor_id: Some("mon-2".to_owned()),
            capture_fps: 30,
            smoothing_ms: 1200,
            ..Config::default()
        };
        store.save(&config).unwrap();
        assert_eq!(store.load(), config);
        assert!(!tmp.0.join("nested").join(TEMP_NAME).exists());
    }

    #[test]
    fn save_replaces_existing_file() {
        let tmp = TempDir::new();
        let store = ConfigStore::new(&tmp.0);
        store.save(&Config::default()).unwrap();
        let changed = Config {
            capture_fps: 45,
            ..Config::default()
        };
        store.save(&changed).unwrap();
        assert_eq!(store.load().capture_fps, 45);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_defaults_returned() {
        let tmp = TempDir::new();
        fs::create_dir_all(&tmp.0).unwrap();
        let store = ConfigStore::new(&tmp.0);
        fs::write(store.path(), b"{ not json").unwrap();
        assert_eq!(store.load(), Config::default());
        assert_eq!(fs::read(tmp.0.join(BACKUP_NAME)).unwrap(), b"{ not json");
    }

    #[test]
    fn utf8_byte_order_mark_is_accepted() {
        let tmp = TempDir::new();
        fs::create_dir_all(&tmp.0).unwrap();
        let store = ConfigStore::new(&tmp.0);
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"{"onboardingComplete": true}"#);
        fs::write(store.path(), bytes).unwrap();
        assert!(store.load().onboarding_complete);
        assert!(!tmp.0.join(BACKUP_NAME).exists());
    }

    #[test]
    fn loaded_values_are_clamped() {
        let tmp = TempDir::new();
        fs::create_dir_all(&tmp.0).unwrap();
        let store = ConfigStore::new(&tmp.0);
        fs::write(
            store.path(),
            br#"{"captureFps": 500, "smoothingMs": 999999, "extra": 1}"#,
        )
        .unwrap();
        let config = store.load();
        assert_eq!(config.capture_fps, 60);
        assert_eq!(config.smoothing_ms, Config::SMOOTHING_MS_MAX);
    }
}
