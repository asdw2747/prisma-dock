use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

use eframe::egui::ColorImage;
use eframe::egui::{self, TextureHandle, TextureOptions};

use crate::discover;
use crate::github::{self, ReleaseInfo};
use crate::icons;
use crate::installer::{CANCELED_MESSAGE, InstallRequest};
use crate::launch;
use crate::model::{AppEntry, Manifest, Platform, Settings, valid_id};
use crate::naming::{slugify, unique_id};
use crate::paths::AppPaths;
use crate::scan;
use crate::theme;
use crate::ui;
use crate::worker::{CheckRequest, Command, Event, Worker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub text: String,
    pub kind: StatusKind,
}

impl Default for StatusMessage {
    fn default() -> Self {
        Self {
            text: "Ready.".to_string(),
            kind: StatusKind::Info,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub enum CheckState {
    #[default]
    Unchecked,
    Checking,
    Ok,
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub enum InstallState {
    #[default]
    Idle,
    Downloading {
        downloaded: u64,
        total: Option<u64>,
    },
    Installing,
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub struct AppRuntime {
    pub check: CheckState,
    pub install: InstallState,
    pub release: Option<ReleaseInfo>,
}

impl AppRuntime {
    pub fn update_available(&self, installed_version: &str) -> bool {
        self.release
            .as_ref()
            .is_some_and(|release| github::is_newer(&release.version, installed_version))
    }

    pub fn has_asset(&self) -> bool {
        self.release
            .as_ref()
            .is_some_and(|release| release.selected.is_some())
    }
}

pub struct PrismaDockApp {
    pub(crate) paths: AppPaths,
    pub(crate) platform: Platform,
    pub(crate) manifest: Manifest,
    pub(crate) settings: Settings,
    pub(crate) runtime: HashMap<String, AppRuntime>,
    pub(crate) worker: Worker,
    pub(crate) worker_busy: bool,
    pub(crate) worker_dead: bool,
    pub(crate) auto_check_attempted: bool,
    pub(crate) checking_total: usize,
    pub(crate) check_index: usize,
    pub(crate) status: StatusMessage,
    pub(crate) startup_error: Option<String>,
    pub(crate) icon: Option<TextureHandle>,
    pub(crate) icon_cache: HashMap<String, Option<TextureHandle>>,
    pub(crate) auto_icons: HashMap<String, PathBuf>,
    pending_icons: HashSet<String>,
    pub(crate) add_modal: Option<ui::add_app::AddAppModal>,
    pub(crate) settings_modal: Option<ui::settings::SettingsModal>,
    pub(crate) discovery_added: usize,
    pub(crate) discovery_warning: Option<String>,
    active_install_cancel: Option<Arc<AtomicBool>>,
}

impl PrismaDockApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::configure(&cc.egui_ctx);

        let paths = AppPaths::resolve();
        let mut errors = Vec::new();
        if let Err(error) = paths.ensure_dirs() {
            errors.push(error);
        }
        paths.cleanup_stale_install_artifacts();

        let loaded = paths.load_manifest();
        let (settings, settings_error) = paths.load_settings();
        if let Some(error) = loaded.error {
            errors.push(error);
        }
        if let Some(error) = settings_error {
            errors.push(error);
        }
        errors.extend(loaded.manifest.validation_warnings());

        let icon = embedded_icon_texture(&cc.egui_ctx);
        let mut app = Self {
            paths,
            platform: Platform::current(),
            manifest: loaded.manifest,
            settings,
            runtime: HashMap::new(),
            worker: Worker::spawn(),
            worker_busy: false,
            worker_dead: false,
            auto_check_attempted: false,
            checking_total: 0,
            check_index: 0,
            status: StatusMessage::default(),
            startup_error: (!errors.is_empty()).then(|| errors.join("\n")),
            icon,
            icon_cache: HashMap::new(),
            auto_icons: HashMap::new(),
            pending_icons: HashSet::new(),
            add_modal: None,
            settings_modal: None,
            discovery_added: 0,
            discovery_warning: None,
            active_install_cancel: None,
        };
        app.sync_runtime();

        if loaded.first_run {
            app.status = if loaded.discovered > 0 {
                StatusMessage {
                    text: format!(
                        "Created data/apps.json and discovered {} local app(s).",
                        loaded.discovered
                    ),
                    kind: StatusKind::Success,
                }
            } else {
                StatusMessage {
                    text: "Created data/apps.json. Use Add App to connect your tools.".to_string(),
                    kind: StatusKind::Info,
                }
            };
        }

        app.queue_missing_icons();
        app
    }

    pub fn app_icon() -> egui::IconData {
        #[cfg(has_embedded_icon)]
        {
            if let Ok(image) = image::load_from_memory_with_format(
                include_bytes!("../assets/icon.png"),
                image::ImageFormat::Png,
            ) {
                let image = image.to_rgba8();
                let (width, height) = image.dimensions();
                return egui::IconData {
                    rgba: image.into_raw(),
                    width,
                    height,
                };
            }
        }
        fallback_icon()
    }

    fn sync_runtime(&mut self) {
        let ids: HashSet<String> = self
            .manifest
            .apps
            .iter()
            .map(|app| app.id.clone())
            .collect();
        self.runtime.retain(|id, _| ids.contains(id));
        for app in &self.manifest.apps {
            self.runtime.entry(app.id.clone()).or_default();
        }
    }

    fn queue_missing_icons(&mut self) {
        let apps: Vec<AppEntry> = self
            .manifest
            .apps
            .iter()
            .filter(|app| app.icon_path.is_none() && !self.pending_icons.contains(&app.id))
            .cloned()
            .collect();
        self.queue_icons(apps);
    }

    fn queue_app_icon(&mut self, app_id: &str) {
        if let Some(app) = self.manifest.app(app_id).cloned() {
            self.queue_icons(vec![app]);
        }
    }

    fn queue_icons(&mut self, apps: Vec<AppEntry>) {
        let apps: Vec<AppEntry> = apps
            .into_iter()
            .filter(|app| app.icon_path.is_none() && !self.pending_icons.contains(&app.id))
            .collect();
        if apps.is_empty() {
            return;
        }

        for app in &apps {
            self.pending_icons.insert(app.id.clone());
        }
        if let Err(error) = self.worker.send(Command::ExtractIcons {
            paths: self.paths.clone(),
            platform: self.platform,
            apps,
        }) {
            self.pending_icons.clear();
            self.worker_dead = true;
            self.notify(StatusKind::Error, error);
        }
    }

    pub(crate) fn runtime_for(&self, app_id: &str) -> AppRuntime {
        self.runtime.get(app_id).cloned().unwrap_or_default()
    }

    pub(crate) fn update_count(&self) -> usize {
        self.manifest
            .apps
            .iter()
            .filter(|app| {
                self.runtime.get(&app.id).is_some_and(|runtime| {
                    runtime.update_available(app.installed_version.as_deref().unwrap_or(""))
                        && runtime.has_asset()
                })
            })
            .count()
    }

    pub(crate) fn notify(&mut self, kind: StatusKind, text: impl Into<String>) {
        self.status = StatusMessage {
            text: text.into(),
            kind,
        };
    }

    pub(crate) fn start_check_all(&mut self) {
        if self.worker_busy {
            return;
        }
        if self.worker_dead {
            self.notify(
                StatusKind::Error,
                "The background worker is offline. Restart Prisma Dock.",
            );
            return;
        }

        let mut requests = Vec::new();
        let mut known_repos = Vec::new();
        for app in &self.manifest.apps {
            if let Some(repo) = app
                .repo
                .as_deref()
                .map(str::trim)
                .filter(|repo| !repo.is_empty())
            {
                known_repos.push(repo.to_ascii_lowercase());
                requests.push(CheckRequest {
                    app_id: app.id.clone(),
                    repo: repo.to_string(),
                });
            }
        }

        for request in &requests {
            if let Some(runtime) = self.runtime.get_mut(&request.app_id) {
                runtime.check = CheckState::Checking;
                runtime.release = None;
            }
        }

        self.checking_total = requests.len();
        self.check_index = 0;
        self.worker_busy = true;
        self.discovery_added = 0;
        self.discovery_warning = None;
        if requests.is_empty() {
            self.notify(
                StatusKind::Info,
                "Scanning GitHub for new apps and repositories…",
            );
        } else {
            self.notify(
                StatusKind::Info,
                format!(
                    "Checking {} app(s) and scanning GitHub for new repositories…",
                    requests.len()
                ),
            );
        }

        if let Err(error) = self.worker.send(Command::CheckAll {
            requests,
            known_repos,
            token: self.settings.token_option(),
        }) {
            self.worker_busy = false;
            self.worker_dead = true;
            self.checking_total = 0;
            self.check_index = 0;
            for runtime in self.runtime.values_mut() {
                if matches!(runtime.check, CheckState::Checking) {
                    runtime.check = CheckState::Unchecked;
                }
            }
            self.notify(StatusKind::Error, error);
        }
    }

    pub(crate) fn start_install(&mut self, app_id: &str) {
        if self.worker_busy {
            return;
        }
        if self.worker_dead {
            self.notify(
                StatusKind::Error,
                "The background worker is offline. Restart Prisma Dock.",
            );
            return;
        }

        let Some(app) = self.manifest.app(app_id).cloned() else {
            self.notify(StatusKind::Error, "That app is no longer in apps.json.");
            return;
        };
        let Some(runtime) = self.runtime.get(app_id) else {
            self.notify(StatusKind::Error, "No release information is available.");
            return;
        };
        let Some(release) = runtime.release.as_ref() else {
            self.notify(
                StatusKind::Info,
                "Check for updates before installing this app.",
            );
            return;
        };
        let release_version = release.version.clone();
        let release_tag = release.tag.clone();
        let Some(asset) = release.selected.clone() else {
            self.notify(
                StatusKind::Error,
                format!(
                    "{} has no compatible {} asset in {}.",
                    app.name,
                    self.platform.label(),
                    release_tag
                ),
            );
            return;
        };

        let request = InstallRequest {
            app_id: app.id.clone(),
            app_name: app.name.clone(),
            version: release_version.clone(),
            asset,
            token: self.settings.token_option(),
            apps_dir: self.paths.apps_dir.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        self.active_install_cancel = Some(request.cancel.clone());

        if let Some(runtime) = self.runtime.get_mut(app_id) {
            runtime.install = InstallState::Downloading {
                downloaded: 0,
                total: None,
            };
        }
        self.worker_busy = true;
        self.notify(
            StatusKind::Info,
            format!("Downloading {} v{}…", app.name, release_version),
        );

        if let Err(error) = self.worker.send(Command::Install(request)) {
            self.worker_busy = false;
            self.worker_dead = true;
            self.active_install_cancel = None;
            if let Some(runtime) = self.runtime.get_mut(app_id) {
                runtime.install = InstallState::Idle;
            }
            self.notify(StatusKind::Error, error);
        }
    }

    pub(crate) fn cancel_install(&mut self) {
        if let Some(cancel) = &self.active_install_cancel {
            cancel.store(true, Ordering::SeqCst);
            self.notify(StatusKind::Info, "Cancelling installation…");
        } else {
            self.notify(StatusKind::Info, "No installation is currently running.");
        }
    }

    pub(crate) fn launch_app(&mut self, app_id: &str) {
        let Some(app) = self.manifest.app(app_id).cloned() else {
            self.notify(StatusKind::Error, "That app is no longer in apps.json.");
            return;
        };

        match launch::launch(&self.paths, &app) {
            Ok(()) => self.notify(StatusKind::Success, format!("Launched {}.", app.name)),
            Err(error) => self.notify(StatusKind::Error, error),
        }
    }

    pub(crate) fn open_app_folder(&mut self, app_id: &str) {
        let Some(app) = self.manifest.app(app_id).cloned() else {
            return;
        };
        match launch::open_folder(&self.paths, &app) {
            Ok(()) => self.notify(
                StatusKind::Info,
                format!("Opened the folder for {}.", app.name),
            ),
            Err(error) => self.notify(StatusKind::Error, error),
        }
    }

    pub(crate) fn choose_icon(&mut self, app_id: &str) {
        let Some(app) = self.manifest.app(app_id).cloned() else {
            self.notify(StatusKind::Error, "That app is no longer in apps.json.");
            return;
        };
        let Some(source) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "ico"])
            .set_title(format!("Choose an icon for {}", app.name))
            .pick_file()
        else {
            return;
        };

        let relative = match icons::install_custom_icon(&self.paths, &app.id, &source) {
            Ok(relative) => relative,
            Err(error) => {
                self.notify(StatusKind::Error, error);
                return;
            }
        };

        let previous = app.icon_path.clone();
        if let Some(entry) = self.manifest.app_mut(app_id) {
            entry.icon_path = Some(relative.clone());
        }
        self.pending_icons.remove(app_id);
        self.auto_icons.remove(app_id);
        self.icon_cache.clear();

        match self.paths.save_manifest(&self.manifest) {
            Ok(()) => self.notify(
                StatusKind::Success,
                format!("Updated the icon for {}.", app.name),
            ),
            Err(error) => {
                if let Some(entry) = self.manifest.app_mut(app_id) {
                    entry.icon_path = previous;
                }
                self.icon_cache.clear();
                self.notify(StatusKind::Error, error);
            }
        }
    }

    pub(crate) fn clear_icon(&mut self, app_id: &str) {
        let Some(app) = self.manifest.app(app_id).cloned() else {
            self.notify(StatusKind::Error, "That app is no longer in apps.json.");
            return;
        };
        if app.icon_path.is_none() {
            self.notify(
                StatusKind::Info,
                "That app already uses its automatic icon.",
            );
            return;
        }

        let previous = app.icon_path.clone();
        if let Some(entry) = self.manifest.app_mut(app_id) {
            entry.icon_path = None;
        }
        self.auto_icons.remove(app_id);
        self.pending_icons.remove(app_id);
        self.icon_cache.clear();

        match self.paths.save_manifest(&self.manifest) {
            Ok(()) => {
                self.queue_app_icon(app_id);
                self.notify(
                    StatusKind::Success,
                    format!("Using the automatic icon for {}.", app.name),
                );
            }
            Err(error) => {
                if let Some(entry) = self.manifest.app_mut(app_id) {
                    entry.icon_path = previous;
                }
                self.icon_cache.clear();
                self.notify(StatusKind::Error, error);
            }
        }
    }

    pub(crate) fn open_add_modal(&mut self) {
        self.settings_modal = None;
        self.add_modal = Some(ui::add_app::AddAppModal::new());
    }

    pub(crate) fn open_settings_modal(&mut self) {
        self.add_modal = None;
        self.settings_modal = Some(ui::settings::SettingsModal::new(&self.settings));
    }

    pub(crate) fn add_apps(&mut self, drafts: Vec<ui::add_app::NewAppDraft>) {
        if drafts.is_empty() {
            return;
        }

        let mut ids: BTreeSet<String> = self.manifest.ids().into_iter().collect();
        let mut added = 0usize;
        let mut duplicates = 0usize;
        let mut added_ids = Vec::new();

        for draft in drafts {
            let mut candidate = draft.candidate;
            candidate.venv_path = draft.venv_path;
            let candidate_path = candidate
                .path
                .canonicalize()
                .unwrap_or_else(|_| candidate.path.clone());

            if self.manifest.apps.iter().any(|app| {
                [app.windows.as_ref(), app.linux.as_ref()]
                    .into_iter()
                    .flatten()
                    .map(|target| {
                        self.paths
                            .resolve_launch_path(&app.id, target.path())
                            .canonicalize()
                            .unwrap_or_else(|_| {
                                self.paths.resolve_launch_path(&app.id, target.path())
                            })
                    })
                    .any(|path| path == candidate_path)
            }) {
                duplicates += 1;
                continue;
            }

            let requested_name = draft.name.trim();
            let name = if requested_name.is_empty() {
                candidate.name.clone()
            } else {
                requested_name.to_string()
            };
            let category = if draft.category.trim().is_empty() {
                "Other".to_string()
            } else {
                draft.category.trim().to_string()
            };
            let id = unique_id(&slugify(&name), &ids);
            ids.insert(id.clone());

            let entry = scan::entry_from_candidate(&candidate, id, name, category);
            added_ids.push(entry.id.clone());
            self.manifest.apps.push(entry);
            added += 1;
        }

        self.sync_runtime();
        self.add_modal = None;

        if added > 0 {
            match self.paths.save_manifest(&self.manifest) {
                Ok(()) => {
                    let mut message = format!("Added {added} app(s).");
                    if duplicates > 0 {
                        message.push_str(&format!(" Skipped {duplicates} duplicate(s)."));
                    }
                    self.notify(StatusKind::Success, message);
                }
                Err(error) => self.notify(StatusKind::Error, error),
            }
        } else if duplicates > 0 {
            self.notify(
                StatusKind::Info,
                format!("All {duplicates} selected app(s) were already registered."),
            );
        }

        if !added_ids.is_empty() {
            let apps = added_ids
                .iter()
                .filter_map(|id| self.manifest.app(id).cloned())
                .collect();
            self.queue_icons(apps);
        }
    }

    pub(crate) fn apply_settings(&mut self, settings: Settings) -> bool {
        let settings = settings.normalized();
        self.settings = settings;
        match self.paths.save_settings(&self.settings) {
            Ok(()) => true,
            Err(error) => {
                self.notify(StatusKind::Error, error);
                false
            }
        }
    }

    pub(crate) fn start_sync(&mut self, settings: Settings) {
        self.apply_settings(settings.clone());
        let url = settings.remote_manifest_url.trim().to_string();
        if url.is_empty() {
            self.notify(StatusKind::Error, "Add a remote apps.json URL first.");
            return;
        }
        if self.worker_busy {
            return;
        }
        if self.worker_dead {
            self.notify(
                StatusKind::Error,
                "The background worker is offline. Restart Prisma Dock.",
            );
            return;
        }

        self.worker_busy = true;
        if let Some(modal) = self.settings_modal.as_mut() {
            modal.sync_status = Some((true, "Syncing remote apps.json…".to_string()));
        }
        self.notify(StatusKind::Info, "Syncing remote apps.json…");

        if let Err(error) = self.worker.send(Command::SyncManifest {
            url,
            token: settings.token_option(),
        }) {
            self.worker_busy = false;
            self.worker_dead = true;
            if let Some(modal) = self.settings_modal.as_mut() {
                modal.sync_status = Some((false, error.clone()));
            }
            self.notify(StatusKind::Error, error);
        }
    }

    pub(crate) fn reload_manifest(&mut self) {
        if self.worker_busy {
            self.notify(
                StatusKind::Info,
                "Wait for the current background task to finish before reloading.",
            );
            return;
        }

        let loaded = self.paths.load_manifest();
        if let Some(error) = &loaded.error
            && !loaded.first_run
        {
            self.notify(StatusKind::Error, error.clone());
            return;
        }

        let load_error = loaded.error;
        self.manifest = loaded.manifest;
        self.runtime.clear();
        self.sync_runtime();
        self.icon_cache.clear();
        self.auto_icons.clear();
        self.pending_icons.clear();
        self.add_modal = None;
        self.settings_modal = None;

        if let Some(error) = load_error {
            self.notify(StatusKind::Error, error);
        } else if loaded.first_run {
            self.notify(
                StatusKind::Success,
                format!(
                    "Created data/apps.json and discovered {} local app(s).",
                    loaded.discovered
                ),
            );
        } else {
            self.notify(
                StatusKind::Success,
                format!("Reloaded {} app(s).", self.manifest.apps.len()),
            );
        }
        self.queue_missing_icons();
    }

    pub(crate) fn edit_manifest(&mut self) {
        if !self.paths.manifest_path.is_file()
            && let Err(error) = self.paths.save_manifest(&self.manifest)
        {
            self.notify(StatusKind::Error, error);
            return;
        }

        match launch::open_path(&self.paths.manifest_path) {
            Ok(()) => self.notify(
                StatusKind::Info,
                "Opened apps.json. Use More → Reload manifest after editing.",
            ),
            Err(error) => self.notify(StatusKind::Error, error),
        }
    }

    pub(crate) fn open_data_folder(&mut self) {
        let _ = self.paths.ensure_dirs();
        match launch::open_path(&self.paths.data_dir) {
            Ok(()) => self.notify(StatusKind::Info, "Opened the data folder."),
            Err(error) => self.notify(StatusKind::Error, error),
        }
    }

    pub(crate) fn needs_repaint(&self) -> bool {
        self.worker_busy || !self.pending_icons.is_empty()
    }

    fn poll_worker(&mut self) {
        loop {
            match self.worker.try_recv() {
                Ok(event) => self.handle_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.worker_dead {
                        self.worker_dead = true;
                        self.worker_busy = false;
                        self.checking_total = 0;
                        self.check_index = 0;
                        self.active_install_cancel = None;
                        for runtime in self.runtime.values_mut() {
                            runtime.install = InstallState::Idle;
                            if matches!(runtime.check, CheckState::Checking) {
                                runtime.check = CheckState::Failed(
                                    "The background worker stopped unexpectedly.".to_string(),
                                );
                            }
                        }
                        if let Some(modal) = self.settings_modal.as_mut() {
                            modal.sync_status = Some((
                                false,
                                "Sync interrupted: the background worker stopped.".to_string(),
                            ));
                        }
                        self.notify(
                            StatusKind::Error,
                            "The background worker stopped unexpectedly. Restart Prisma Dock.",
                        );
                    }
                    break;
                }
            }
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::ReposDiscovered { repos, errors } => {
                let added = discover::append_github_apps(&mut self.manifest, repos);
                let added_count = added.len();
                for (id, release) in added {
                    self.runtime.insert(
                        id,
                        AppRuntime {
                            check: CheckState::Ok,
                            install: InstallState::Idle,
                            release: Some(release),
                        },
                    );
                }
                self.discovery_added = self.discovery_added.saturating_add(added_count);

                let mut warnings = errors;
                if added_count > 0 {
                    self.sync_runtime();
                    if let Err(error) = self.paths.save_manifest(&self.manifest) {
                        warnings.push(format!("Could not save apps.json: {error}"));
                        self.notify(StatusKind::Error, error);
                    } else {
                        self.notify(
                            StatusKind::Success,
                            format!("Added {added_count} new app(s) from GitHub."),
                        );
                    }
                }
                if !warnings.is_empty() {
                    self.discovery_warning = Some(warnings.join("; "));
                }
            }
            Event::CheckStarted {
                app_id,
                index,
                total,
            } => {
                self.check_index = index + 1;
                self.checking_total = total;
                if let Some(runtime) = self.runtime.get_mut(&app_id) {
                    runtime.check = CheckState::Checking;
                }
                if let Some(app) = self.manifest.app(&app_id) {
                    self.notify(
                        StatusKind::Info,
                        format!("Checking {} ({}/{})…", app.name, index + 1, total),
                    );
                }
            }
            Event::CheckResult { app_id, result } => {
                if let Some(runtime) = self.runtime.get_mut(&app_id) {
                    match result {
                        Ok(release) => {
                            runtime.check = CheckState::Ok;
                            runtime.release = Some(release);
                        }
                        Err(error) => {
                            runtime.check = CheckState::Failed(error);
                            runtime.release = None;
                        }
                    }
                }
            }
            Event::CheckAllDone => {
                self.worker_busy = false;
                self.checking_total = 0;
                self.check_index = 0;
                let updates = self.update_count();
                let failures = self
                    .runtime
                    .values()
                    .filter(|runtime| matches!(runtime.check, CheckState::Failed(_)))
                    .count();
                let missing_assets = self
                    .manifest
                    .apps
                    .iter()
                    .filter(|app| {
                        self.runtime.get(&app.id).is_some_and(|runtime| {
                            runtime.release.as_ref().is_some_and(|release| {
                                github::is_newer(
                                    &release.version,
                                    app.installed_version.as_deref().unwrap_or(""),
                                ) && release.selected.is_none()
                            })
                        })
                    })
                    .count();
                let mut discovery_suffix = String::new();
                if self.discovery_added > 0 {
                    discovery_suffix.push_str(&format!(
                        " Added {} new app(s) from GitHub.",
                        self.discovery_added
                    ));
                }
                if let Some(warning) = &self.discovery_warning {
                    discovery_suffix.push_str(&format!(" GitHub discovery: {warning}"));
                }

                if updates > 0 {
                    let suffix = if failures > 0 {
                        format!(" {failures} repo(s) could not be checked.")
                    } else {
                        String::new()
                    };
                    self.notify(
                        StatusKind::Info,
                        format!("{updates} update(s) available.{suffix}{discovery_suffix}"),
                    );
                } else if missing_assets > 0 {
                    self.notify(
                        StatusKind::Info,
                        format!(
                            "{missing_assets} newer release(s) have no compatible {} asset.{discovery_suffix}",
                            self.platform.label()
                        ),
                    );
                } else if failures > 0 {
                    self.notify(
                        StatusKind::Error,
                        format!(
                            "No updates found, but {failures} repo(s) could not be checked.{discovery_suffix}"
                        ),
                    );
                } else if self.discovery_added > 0 {
                    self.notify(StatusKind::Success, discovery_suffix.trim().to_string());
                } else if self.discovery_warning.is_some() {
                    self.notify(
                        StatusKind::Info,
                        format!("All apps are up to date.{discovery_suffix}"),
                    );
                } else {
                    self.notify(StatusKind::Success, "All apps are up to date.");
                }
            }
            Event::DownloadProgress {
                app_id,
                downloaded,
                total,
            } => {
                if let Some(runtime) = self.runtime.get_mut(&app_id) {
                    runtime.install = InstallState::Downloading { downloaded, total };
                }
            }
            Event::Installing { app_id } => {
                if let Some(runtime) = self.runtime.get_mut(&app_id) {
                    runtime.install = InstallState::Installing;
                }
                if let Some(app) = self.manifest.app(&app_id) {
                    self.notify(StatusKind::Info, format!("Installing {}…", app.name));
                }
            }
            Event::InstallResult { app_id, result } => {
                self.worker_busy = false;
                self.active_install_cancel = None;
                match result {
                    Ok(artifact) => {
                        if let Some(app) = self.manifest.app_mut(&app_id) {
                            app.installed_version = Some(artifact.version.clone());
                            app.set_binary_target_for(
                                self.platform,
                                artifact.relative_path.clone(),
                            );
                        }
                        if let Some(runtime) = self.runtime.get_mut(&app_id) {
                            runtime.install = InstallState::Idle;
                        }
                        self.auto_icons.remove(&app_id);
                        self.pending_icons.remove(&app_id);
                        self.icon_cache.clear();

                        match self.paths.save_manifest(&self.manifest) {
                            Ok(()) => {
                                let name = self
                                    .manifest
                                    .app(&app_id)
                                    .map(|app| app.name.clone())
                                    .unwrap_or_else(|| artifact.app_id.clone());
                                self.notify(
                                    StatusKind::Success,
                                    format!(
                                        "Installed {name} v{} ({}).",
                                        artifact.version, artifact.asset_name
                                    ),
                                );
                            }
                            Err(error) => {
                                self.notify(
                                    StatusKind::Error,
                                    format!(
                                        "Installed the app, but could not save apps.json: {error}"
                                    ),
                                );
                            }
                        }
                        self.queue_app_icon(&app_id);
                    }
                    Err(error) => {
                        if let Some(runtime) = self.runtime.get_mut(&app_id) {
                            runtime.install = if error == CANCELED_MESSAGE {
                                InstallState::Idle
                            } else {
                                InstallState::Failed(error.clone())
                            };
                        }
                        if error == CANCELED_MESSAGE {
                            self.notify(StatusKind::Info, error);
                        } else {
                            self.notify(StatusKind::Error, error);
                        }
                    }
                }
            }
            Event::IconReady { app_id, path } => {
                self.pending_icons.remove(&app_id);
                if self
                    .manifest
                    .app(&app_id)
                    .is_some_and(|app| app.icon_path.is_none())
                {
                    match path {
                        Some(path) => {
                            self.auto_icons.insert(app_id, path);
                        }
                        None => {
                            self.auto_icons.remove(&app_id);
                        }
                    }
                }
            }
            Event::SyncResult { result } => {
                self.worker_busy = false;
                match result {
                    Ok(remote) => {
                        let backup_warning = self.paths.backup_manifest().err();
                        let (imported, local_only, skipped) =
                            merge_remote_manifest(&self.manifest, remote);
                        self.manifest.apps = imported.apps;
                        self.runtime.clear();
                        self.sync_runtime();
                        self.icon_cache.clear();
                        self.auto_icons.clear();
                        self.pending_icons.clear();
                        self.add_modal = None;

                        let save_error = self.paths.save_manifest(&self.manifest).err();
                        let mut message =
                            format!("Synced {} app(s) from remote", self.manifest.apps.len());
                        if local_only > 0 {
                            message.push_str(&format!("; kept {local_only} local-only app(s)"));
                        }
                        if skipped > 0 {
                            message.push_str(&format!("; skipped {skipped} invalid entry(ies)"));
                        }
                        message.push('.');

                        if let Some(warning) = backup_warning {
                            message.push_str(&format!(" Backup warning: {warning}"));
                        }
                        if let Some(error) = save_error {
                            message.push_str(&format!(" Could not save: {error}"));
                            self.notify(StatusKind::Error, message.clone());
                            if let Some(modal) = self.settings_modal.as_mut() {
                                modal.sync_status = Some((false, message));
                            }
                        } else {
                            self.notify(StatusKind::Success, message.clone());
                            if let Some(modal) = self.settings_modal.as_mut() {
                                modal.sync_status = Some((true, message));
                            }
                        }
                        self.queue_missing_icons();
                    }
                    Err(error) => {
                        self.notify(StatusKind::Error, error.clone());
                        if let Some(modal) = self.settings_modal.as_mut() {
                            modal.sync_status = Some((false, error));
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn icon_for(
        &mut self,
        ctx: &egui::Context,
        app: &AppEntry,
    ) -> Option<TextureHandle> {
        if let Some(icon_path) = app.icon_path.as_deref() {
            let path = self.paths.resolve_icon_path(icon_path);
            if let Some(texture) = self.load_icon_texture(ctx, &app.id, &path) {
                return Some(texture);
            }
        }

        let path = self.auto_icons.get(&app.id)?.clone();
        self.load_icon_texture(ctx, &app.id, &path)
    }

    fn load_icon_texture(
        &mut self,
        ctx: &egui::Context,
        app_id: &str,
        path: &Path,
    ) -> Option<TextureHandle> {
        let key = path.to_string_lossy().into_owned();
        if let Some(cached) = self.icon_cache.get(&key) {
            return cached.clone();
        }

        let loaded = load_custom_icon(ctx, app_id, path);
        self.icon_cache.insert(key, loaded.clone());
        loaded
    }
}

impl eframe::App for PrismaDockApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();

        if !self.auto_check_attempted {
            self.auto_check_attempted = true;
            if self.settings.auto_check_updates && !self.worker_busy {
                self.start_check_all();
            }
        }

        if self.needs_repaint() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.draw(&ctx, root);
    }
}

fn merge_remote_manifest(local: &Manifest, remote: Manifest) -> (Manifest, usize, usize) {
    let local_by_id: HashMap<String, AppEntry> = local
        .apps
        .iter()
        .cloned()
        .map(|app| (app.id.clone(), app))
        .collect();
    let mut seen = HashSet::new();
    let mut merged = Vec::new();
    let mut skipped = 0usize;

    for mut remote_app in remote.apps {
        if !valid_id(&remote_app.id) || !seen.insert(remote_app.id.clone()) {
            skipped += 1;
            continue;
        }

        if let Some(local_app) = local_by_id.get(&remote_app.id) {
            remote_app.installed_version = local_app.installed_version.clone();
            if remote_app.windows.is_none() {
                remote_app.windows = local_app.windows.clone();
            }
            if remote_app.linux.is_none() {
                remote_app.linux = local_app.linux.clone();
            }
            if remote_app.icon_path.is_none() {
                remote_app.icon_path = local_app.icon_path.clone();
            }
        }

        merged.push(remote_app);
    }

    let mut local_only = 0usize;
    for app in &local.apps {
        if !seen.contains(&app.id) {
            merged.push(app.clone());
            local_only += 1;
        }
    }

    (Manifest { apps: merged }, local_only, skipped)
}

fn embedded_icon_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let icon = PrismaDockApp::app_icon();
    let image =
        ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
    Some(ctx.load_texture("prisma-dock-mark", image, TextureOptions::LINEAR))
}

fn load_custom_icon(ctx: &egui::Context, app_id: &str, path: &Path) -> Option<TextureHandle> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 10 * 1024 * 1024 {
        return None;
    }

    let image = image::open(path).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 || width > 2048 || height > 2048 {
        return None;
    }

    let color_image =
        ColorImage::from_rgba_unmultiplied([width as usize, height as usize], image.as_raw());
    Some(ctx.load_texture(
        format!("app-icon-{app_id}"),
        color_image,
        TextureOptions::LINEAR,
    ))
}

fn fallback_icon() -> egui::IconData {
    let size = 96u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];

    for y in 0..size {
        for x in 0..size {
            let fx = x as f32;
            let fy = y as f32;
            let index = ((y * size + x) * 4) as usize;
            let rounded = rounded_rect_inside(fx, fy, 0.0, 0.0, size as f32, size as f32, 18.0);
            if !rounded {
                continue;
            }

            let mut color = [10.0, 13.0, 22.0, 255.0];
            let tri = triangle_inside(fx, fy, size as f32);
            if tri {
                let t = ((fx / size as f32) * 0.5 + (fy / size as f32) * 0.5).clamp(0.0, 1.0);
                color = [
                    139.0 + (79.0 - 139.0) * t,
                    124.0 + (209.0 - 124.0) * t,
                    246.0 + (197.0 - 246.0) * t,
                    255.0,
                ];
            }
            rgba[index] = color[0] as u8;
            rgba[index + 1] = color[1] as u8;
            rgba[index + 2] = color[2] as u8;
            rgba[index + 3] = color[3] as u8;
        }
    }

    egui::IconData {
        rgba,
        width: size,
        height: size,
    }
}

fn rounded_rect_inside(
    x: f32,
    y: f32,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    radius: f32,
) -> bool {
    if x < left || x > right || y < top || y > bottom {
        return false;
    }
    let nearest_x = x.clamp(left + radius, right - radius);
    let nearest_y = y.clamp(top + radius, bottom - radius);
    let dx = x - nearest_x;
    let dy = y - nearest_y;
    dx * dx + dy * dy <= radius * radius
}

fn triangle_inside(x: f32, y: f32, size: f32) -> bool {
    let cx = size * 0.5;
    let top = size * 0.20;
    let left = size * 0.22;
    let right = size * 0.78;
    let bottom = size * 0.78;

    let sign = |x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32| {
        (x1 - x3) * (y2 - y3) - (x2 - x3) * (y1 - y3)
    };
    let d1 = sign(x, y, cx, top, left, bottom);
    let d2 = sign(x, y, left, bottom, right, bottom);
    let d3 = sign(x, y, right, bottom, cx, top);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LaunchTarget;

    #[test]
    fn merges_remote_manifest_and_preserves_local_install_state() {
        let local = Manifest {
            apps: vec![AppEntry {
                id: "tool".to_string(),
                name: "Tool".to_string(),
                description: "old".to_string(),
                category: "Other".to_string(),
                repo: None,
                installed_version: Some("1.0.0".to_string()),
                icon_path: None,
                windows: None,
                linux: Some(LaunchTarget::Binary {
                    path: "Tool.AppImage".to_string(),
                }),
            }],
        };
        let remote = Manifest {
            apps: vec![
                AppEntry {
                    id: "tool".to_string(),
                    name: "Tool".to_string(),
                    description: "new".to_string(),
                    category: "Tools".to_string(),
                    repo: Some("owner/tool".to_string()),
                    installed_version: None,
                    icon_path: None,
                    windows: None,
                    linux: None,
                },
                AppEntry {
                    id: "new".to_string(),
                    name: "New".to_string(),
                    description: String::new(),
                    category: "Other".to_string(),
                    repo: None,
                    installed_version: Some("2.0.0".to_string()),
                    icon_path: None,
                    windows: None,
                    linux: None,
                },
            ],
        };

        let (merged, local_only, skipped) = merge_remote_manifest(&local, remote);
        assert_eq!(merged.apps.len(), 2);
        assert_eq!(local_only, 0);
        assert_eq!(skipped, 0);
        let tool = merged.app("tool").unwrap();
        assert_eq!(tool.description, "new");
        assert_eq!(tool.installed_version.as_deref(), Some("1.0.0"));
        assert!(tool.linux.is_some());
    }

    #[test]
    fn keeps_local_only_apps() {
        let local = Manifest {
            apps: vec![AppEntry {
                id: "local".to_string(),
                name: "Local".to_string(),
                description: String::new(),
                category: "Other".to_string(),
                repo: None,
                installed_version: None,
                icon_path: None,
                windows: None,
                linux: None,
            }],
        };
        let (merged, local_only, _) = merge_remote_manifest(&local, Manifest::default());
        assert_eq!(merged.apps.len(), 1);
        assert_eq!(local_only, 1);
    }
}
