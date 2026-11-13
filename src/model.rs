use std::collections::HashSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            Self::Other
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Linux => "Linux",
            Self::Other => "this platform",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub apps: Vec<AppEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub installed_version: Option<String>,
    #[serde(default)]
    pub icon_path: Option<String>,
    #[serde(default)]
    pub windows: Option<LaunchTarget>,
    #[serde(default)]
    pub linux: Option<LaunchTarget>,
}

impl AppEntry {
    pub fn target_for(&self, platform: Platform) -> Option<&LaunchTarget> {
        match platform {
            Platform::Windows => self.windows.as_ref(),
            Platform::Linux => self.linux.as_ref(),
            Platform::Other => None,
        }
    }

    pub fn set_target_for(&mut self, platform: Platform, target: LaunchTarget) {
        match platform {
            Platform::Windows => self.windows = Some(target),
            Platform::Linux => self.linux = Some(target),
            Platform::Other => {}
        }
    }

    pub fn set_binary_target_for(&mut self, platform: Platform, path: String) {
        self.set_target_for(platform, LaunchTarget::Binary { path });
    }

    pub fn version_label(&self, has_target: bool) -> String {
        let value = self.installed_version.as_deref().unwrap_or("").trim();
        if value.is_empty() {
            if has_target {
                "Local build".to_string()
            } else {
                "Not installed".to_string()
            }
        } else if value.starts_with('v') || value.starts_with('V') {
            value.to_string()
        } else {
            format!("v{value}")
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LaunchTarget {
    Binary {
        path: String,
    },
    Python {
        script_path: String,
        #[serde(default)]
        venv_path: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        windowless: bool,
    },
}

impl LaunchTarget {
    pub fn path(&self) -> &str {
        match self {
            Self::Binary { path } => path,
            Self::Python { script_path, .. } => script_path,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub github_token: String,
    #[serde(default)]
    pub remote_manifest_url: String,
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            github_token: String::new(),
            remote_manifest_url: String::new(),
            auto_check_updates: true,
        }
    }
}

impl Settings {
    pub fn normalized(&self) -> Self {
        Self {
            github_token: self.github_token.trim().to_string(),
            remote_manifest_url: self.remote_manifest_url.trim().to_string(),
            auto_check_updates: self.auto_check_updates,
        }
    }

    pub fn token_option(&self) -> Option<String> {
        let token = self.github_token.trim();
        (!token.is_empty()).then(|| token.to_string())
    }
}

pub fn default_category() -> String {
    "Other".to_string()
}

fn default_true() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

impl Manifest {
    pub fn app(&self, id: &str) -> Option<&AppEntry> {
        self.apps.iter().find(|app| app.id == id)
    }

    pub fn app_mut(&mut self, id: &str) -> Option<&mut AppEntry> {
        self.apps.iter_mut().find(|app| app.id == id)
    }

    pub fn ids(&self) -> HashSet<String> {
        self.apps.iter().map(|app| app.id.clone()).collect()
    }

    pub fn validation_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        let mut seen = HashSet::new();
        let mut seen_lower = HashSet::new();

        for app in &self.apps {
            if !valid_id(&app.id) {
                warnings.push(format!(
                    "App id {:?} is invalid; use letters, numbers, '-' or '_'.",
                    app.id
                ));
            }
            if !seen.insert(app.id.clone()) {
                warnings.push(format!("Duplicate app id {:?}.", app.id));
            }
            if !seen_lower.insert(app.id.to_ascii_lowercase()) {
                warnings.push(format!(
                    "App id {:?} differs only by case from another entry; Windows cannot distinguish these folders.",
                    app.id
                ));
            }
            if app.name.trim().is_empty() {
                warnings.push(format!("App {:?} has no name.", app.id));
            }
        }

        warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips_binary_and_python_targets() {
        let manifest = Manifest {
            apps: vec![AppEntry {
                id: "tool".to_string(),
                name: "Tool".to_string(),
                description: "Test app".to_string(),
                category: "Other".to_string(),
                repo: Some("owner/tool".to_string()),
                installed_version: Some("1.2.3".to_string()),
                icon_path: None,
                windows: Some(LaunchTarget::Binary {
                    path: "Tool.exe".to_string(),
                }),
                linux: Some(LaunchTarget::Python {
                    script_path: "tool.py".to_string(),
                    venv_path: Some(".venv".to_string()),
                    windowless: false,
                }),
            }],
        };

        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let loaded: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.apps.len(), 1);
        assert!(matches!(
            loaded.apps[0].windows,
            Some(LaunchTarget::Binary { .. })
        ));
        assert!(matches!(
            loaded.apps[0].linux,
            Some(LaunchTarget::Python { .. })
        ));
    }

    #[test]
    fn missing_optional_fields_use_defaults() {
        let json = r#"{"apps":[{"id":"x","name":"X"}]}"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.apps[0].category, "Other");
        assert_eq!(manifest.apps[0].installed_version, None);
        assert_eq!(manifest.apps[0].repo, None);
        assert_eq!(manifest.apps[0].windows, None);
    }

    #[test]
    fn settings_default_auto_check_on() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.auto_check_updates);
    }

    #[test]
    fn version_label_describes_missing_versions() {
        let mut app = AppEntry {
            id: "tool".to_string(),
            name: "Tool".to_string(),
            description: String::new(),
            category: "Other".to_string(),
            repo: None,
            installed_version: None,
            icon_path: None,
            windows: None,
            linux: None,
        };
        assert_eq!(app.version_label(false), "Not installed");
        assert_eq!(app.version_label(true), "Local build");

        app.installed_version = Some("v1.2.3".to_string());
        assert_eq!(app.version_label(false), "v1.2.3");
        app.installed_version = Some("1.2.3".to_string());
        assert_eq!(app.version_label(false), "v1.2.3");
    }

    #[test]
    fn serializes_missing_version_as_null_and_keeps_existing_strings() {
        let missing = AppEntry {
            id: "new".to_string(),
            name: "New".to_string(),
            description: String::new(),
            category: "Other".to_string(),
            repo: Some("asdw2747/new".to_string()),
            installed_version: None,
            icon_path: None,
            windows: None,
            linux: None,
        };
        let value = serde_json::to_value(&missing).unwrap();
        assert!(value.get("installed_version").unwrap().is_null());

        let existing = serde_json::from_str::<AppEntry>(
            r#"{"id":"old","name":"Old","installed_version":"1.0.0"}"#,
        )
        .unwrap();
        assert_eq!(existing.installed_version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn validates_ids() {
        assert!(valid_id("haunted-files"));
        assert!(valid_id("My_Tool2"));
        assert!(!valid_id(""));
        assert!(!valid_id("../escape"));
        assert!(!valid_id("with space"));
    }
}
