use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::discover;
use crate::model::{Manifest, Platform, Settings};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub base: PathBuf,
    pub data_dir: PathBuf,
    pub apps_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub settings_path: PathBuf,
}

pub struct ManifestLoad {
    pub manifest: Manifest,
    pub first_run: bool,
    pub discovered: usize,
    pub error: Option<String>,
}

impl AppPaths {
    pub fn resolve() -> Self {
        let base = std::env::var_os("PRISMA_DOCK_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
            })
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        Self::from_base(base)
    }

    pub fn from_base(base: impl Into<PathBuf>) -> Self {
        let base = base.into();
        let data_dir = base.join("data");
        let apps_dir = base.join("apps");
        Self {
            manifest_path: data_dir.join("apps.json"),
            settings_path: data_dir.join("settings.json"),
            base,
            data_dir,
            apps_dir,
        }
    }

    pub fn ensure_dirs(&self) -> Result<(), String> {
        fs::create_dir_all(&self.data_dir)
            .map_err(|error| format!("Could not create {}: {error}", self.data_dir.display()))?;
        fs::create_dir_all(&self.apps_dir)
            .map_err(|error| format!("Could not create {}: {error}", self.apps_dir.display()))?;
        Ok(())
    }

    /// Remove abandoned staging directories and partial downloads older than
    /// one hour. The installer removes its own staging directory on normal
    /// errors and cancellation; this handles process kills and power loss.
    pub fn cleanup_stale_install_artifacts(&self) {
        let Ok(app_dirs) = fs::read_dir(&self.apps_dir) else {
            return;
        };
        let cutoff = SystemTime::now().checked_sub(Duration::from_secs(60 * 60));

        for app_dir in app_dirs.flatten() {
            if !app_dir
                .file_type()
                .map(|kind| kind.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let Ok(children) = fs::read_dir(app_dir.path()) else {
                continue;
            };
            for child in children.flatten() {
                let name = child.file_name().to_string_lossy().into_owned();
                if !name.starts_with(".staging-") && !name.ends_with(".part") {
                    continue;
                }
                let Ok(metadata) = child.metadata() else {
                    continue;
                };
                let stale = cutoff
                    .zip(metadata.modified().ok())
                    .map(|(cutoff, modified)| modified < cutoff)
                    .unwrap_or(false);
                if !stale {
                    continue;
                }
                if metadata.is_dir() {
                    let _ = fs::remove_dir_all(child.path());
                } else {
                    let _ = fs::remove_file(child.path());
                }
            }
        }
    }

    pub fn load_manifest(&self) -> ManifestLoad {
        match fs::read_to_string(&self.manifest_path) {
            Ok(text) => match serde_json::from_str::<Manifest>(&text) {
                Ok(manifest) => ManifestLoad {
                    manifest,
                    first_run: false,
                    discovered: 0,
                    error: None,
                },
                Err(error) => ManifestLoad {
                    manifest: Manifest::default(),
                    first_run: false,
                    discovered: 0,
                    error: Some(format!(
                        "Could not parse {}: {error}. The file was not changed.",
                        self.manifest_path.display()
                    )),
                },
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let (manifest, discovered) =
                    discover::starter_manifest(&self.base, Platform::current());
                let save_error = self.save_manifest(&manifest).err().map(|error| {
                    format!(
                        "A starter manifest was prepared in memory but could not be saved: {error}"
                    )
                });
                ManifestLoad {
                    manifest,
                    first_run: true,
                    discovered,
                    error: save_error,
                }
            }
            Err(error) => ManifestLoad {
                manifest: Manifest::default(),
                first_run: false,
                discovered: 0,
                error: Some(format!(
                    "Could not read {}: {error}",
                    self.manifest_path.display()
                )),
            },
        }
    }

    pub fn save_manifest(&self, manifest: &Manifest) -> Result<(), String> {
        let mut bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|error| format!("Could not serialize apps.json: {error}"))?;
        bytes.push(b'\n');
        write_atomic(&self.manifest_path, &bytes, None)
            .map_err(|error| format!("Could not save {}: {error}", self.manifest_path.display()))
    }

    pub fn backup_manifest(&self) -> Result<(), String> {
        if !self.manifest_path.is_file() {
            return Ok(());
        }
        let backup = self.manifest_path.with_extension("json.bak");
        fs::copy(&self.manifest_path, &backup)
            .map(|_| ())
            .map_err(|error| format!("Could not back up apps.json: {error}"))
    }

    pub fn load_settings(&self) -> (Settings, Option<String>) {
        match fs::read_to_string(&self.settings_path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(settings) => (settings, None),
                Err(error) => (
                    Settings::default(),
                    Some(format!(
                        "Could not parse {}: {error}. Defaults are being used.",
                        self.settings_path.display()
                    )),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let settings = Settings::default();
                let save_error = self.save_settings(&settings).err().map(|error| {
                    format!("Could not create {}: {error}", self.settings_path.display())
                });
                (settings, save_error)
            }
            Err(error) => (
                Settings::default(),
                Some(format!(
                    "Could not read {}: {error}",
                    self.settings_path.display()
                )),
            ),
        }
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<(), String> {
        let settings = settings.normalized();
        let mut bytes = serde_json::to_vec_pretty(&settings)
            .map_err(|error| format!("Could not serialize settings.json: {error}"))?;
        bytes.push(b'\n');
        #[cfg(unix)]
        let mode = Some(0o600);
        #[cfg(not(unix))]
        let mode = None;

        write_atomic(&self.settings_path, &bytes, mode)
            .map_err(|error| format!("Could not save {}: {error}", self.settings_path.display()))
    }

    pub fn install_dir(&self, app_id: &str) -> PathBuf {
        self.apps_dir.join(app_id)
    }

    pub fn resolve_launch_path(&self, app_id: &str, path: &str) -> PathBuf {
        let expanded = expand_tilde(path);
        if expanded.is_absolute() {
            expanded
        } else {
            self.install_dir(app_id).join(expanded)
        }
    }

    pub fn resolve_icon_path(&self, path: &str) -> PathBuf {
        let expanded = expand_tilde(path);
        if expanded.is_absolute() {
            expanded
        } else {
            self.base.join(expanded)
        }
    }
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }

    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }

    PathBuf::from(path)
}

/// Store paths under the user's home directory with a leading `~` so a
/// manifest copied to another machine has a better chance of still working.
pub fn portable_path_string(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(relative) = path.strip_prefix(&home)
    {
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.is_empty() {
            return "~".to_string();
        }
        return format!("~/{relative}");
    }
    path.to_string_lossy().into_owned()
}

fn write_atomic(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("prisma-dock.tmp");
    let temp = path.with_file_name(format!(".{file_name}.{}.{nonce}.tmp", std::process::id()));

    let result = (|| -> Result<(), String> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(mode) = mode {
            options.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;

        let mut file = options
            .open(&temp)
            .map_err(|error| format!("Could not create {}: {error}", temp.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("Could not write {}: {error}", temp.display()))?;
        file.sync_all()
            .map_err(|error| format!("Could not flush {}: {error}", temp.display()))?;
        drop(file);

        fs::rename(&temp, path)
            .map_err(|error| format!("Could not replace {}: {error}", path.display()))?;

        // Best effort on Unix: make the rename durable across power loss.
        #[cfg(unix)]
        if let Some(parent) = path.parent()
            && let Ok(directory) = File::open(parent)
        {
            let _ = directory.sync_all();
        }

        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppEntry, LaunchTarget};

    fn test_app() -> Manifest {
        Manifest {
            apps: vec![AppEntry {
                id: "tool".to_string(),
                name: "Tool".to_string(),
                description: String::new(),
                category: "Other".to_string(),
                repo: None,
                installed_version: Some("1.0.0".to_string()),
                icon_path: None,
                windows: None,
                linux: Some(LaunchTarget::Binary {
                    path: "Tool.AppImage".to_string(),
                }),
            }],
        }
    }

    #[test]
    fn saves_and_loads_manifest_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_base(temp.path());
        paths.ensure_dirs().unwrap();
        paths.save_manifest(&test_app()).unwrap();

        assert!(paths.manifest_path.is_file());
        let loaded = paths.load_manifest();
        assert!(!loaded.first_run);
        assert!(loaded.error.is_none());
        assert_eq!(loaded.manifest.apps.len(), 1);
        assert_eq!(loaded.manifest.apps[0].name, "Tool");
        assert!(
            std::fs::read_dir(&paths.data_dir)
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp")),
            "atomic save should not leave a temp file"
        );
    }

    #[test]
    fn failed_atomic_replace_does_not_delete_the_existing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("not-a-file");
        std::fs::create_dir(&destination).unwrap();

        let error = write_atomic(&destination, b"data", None);
        assert!(error.is_err());
        assert!(destination.is_dir(), "existing destination must remain");
        assert!(
            std::fs::read_dir(temp.path())
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp")),
            "failed write should clean up its temp file"
        );
    }

    #[test]
    fn resolves_relative_and_absolute_launch_paths() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_base(temp.path());
        assert_eq!(
            paths.resolve_launch_path("tool", "bin/tool"),
            temp.path().join("apps").join("tool").join("bin/tool")
        );

        let absolute = temp.path().join("outside").join("tool");
        assert_eq!(
            paths.resolve_launch_path("tool", &absolute.to_string_lossy()),
            absolute
        );
    }

    #[test]
    fn malformed_manifest_is_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_base(temp.path());
        paths.ensure_dirs().unwrap();
        std::fs::write(&paths.manifest_path, "{not json").unwrap();

        let loaded = paths.load_manifest();
        assert!(loaded.error.is_some());
        assert_eq!(
            std::fs::read_to_string(&paths.manifest_path).unwrap(),
            "{not json"
        );
    }

    #[cfg(unix)]
    #[test]
    fn settings_are_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_base(temp.path());
        paths.ensure_dirs().unwrap();
        paths.save_settings(&Settings::default()).unwrap();

        let mode = std::fs::metadata(&paths.settings_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn abbreviates_home_paths() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let path = home.join("Desktop").join("Tool.AppImage");
        let portable = portable_path_string(&path);
        assert!(portable.starts_with("~/"), "{portable}");
        assert!(portable.ends_with("Desktop/Tool.AppImage"));
    }
}
