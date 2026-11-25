use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use crate::github::{self, DiscoveredRepo, ReleaseInfo};
use crate::icons;
use crate::installer::{self, InstallEvent, InstallRequest, InstalledArtifact};
use crate::model::{AppEntry, Manifest, Platform};
use crate::paths::AppPaths;

#[derive(Debug, Clone)]
pub struct CheckRequest {
    pub app_id: String,
    pub repo: String,
}

pub enum Command {
    CheckAll {
        requests: Vec<CheckRequest>,
        known_repos: Vec<String>,
        token: Option<String>,
    },
    Install(InstallRequest),
    ExtractIcons {
        paths: AppPaths,
        platform: Platform,
        apps: Vec<AppEntry>,
    },
    SyncManifest {
        url: String,
        token: Option<String>,
    },
}

pub enum Event {
    ReposDiscovered {
        repos: Vec<DiscoveredRepo>,
        errors: Vec<String>,
    },
    CheckStarted {
        app_id: String,
        index: usize,
        total: usize,
    },
    CheckResult {
        app_id: String,
        result: Result<ReleaseInfo, String>,
    },
    CheckAllDone,
    DownloadProgress {
        app_id: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Installing {
        app_id: String,
    },
    InstallResult {
        app_id: String,
        result: Result<InstalledArtifact, String>,
    },
    IconReady {
        app_id: String,
        path: Option<std::path::PathBuf>,
    },
    SyncResult {
        result: Result<Manifest, String>,
    },
}

pub struct Worker {
    commands: Sender<Command>,
    events: Receiver<Event>,
}

impl Worker {
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        thread::Builder::new()
            .name("prisma-dock-worker".to_string())
            .spawn(move || worker_loop(command_rx, event_tx))
            .expect("failed to start Prisma Dock background worker");

        Self {
            commands: command_tx,
            events: event_rx,
        }
    }

    pub fn send(&self, command: Command) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "The background worker is no longer running.".to_string())
    }

    pub fn try_recv(&self) -> Result<Event, TryRecvError> {
        self.events.try_recv()
    }
}

fn worker_loop(commands: Receiver<Command>, events: Sender<Event>) {
    let api_client = github::api_client();
    let download_client = github::download_client();

    while let Ok(command) = commands.recv() {
        match command {
            Command::CheckAll {
                requests,
                known_repos,
                token,
            } => {
                let token = token.as_deref().filter(|value| !value.trim().is_empty());
                let mut discovered = Vec::new();
                let mut discovery_errors = Vec::new();

                match &api_client {
                    Ok(client) => {
                        match github::list_user_repos(client, github::DISCOVERY_OWNER, token) {
                            Ok(repos) => {
                                let existing: HashSet<String> = known_repos
                                    .iter()
                                    .map(|repo| repo.trim().to_ascii_lowercase())
                                    .filter(|repo| !repo.is_empty())
                                    .collect();
                                let outcome = github::discover_repos(
                                    repos,
                                    github::DISCOVERY_OWNER,
                                    &existing,
                                    |repo| github::fetch_latest_optional(client, repo, token),
                                );
                                discovered = outcome.discovered;
                                discovery_errors = outcome.errors;
                            }
                            Err(error) => discovery_errors.push(error),
                        }
                    }
                    Err(error) => discovery_errors.push(error.clone()),
                }

                if events
                    .send(Event::ReposDiscovered {
                        repos: discovered,
                        errors: discovery_errors,
                    })
                    .is_err()
                {
                    return;
                }

                let total = requests.len();
                for (index, request) in requests.into_iter().enumerate() {
                    if events
                        .send(Event::CheckStarted {
                            app_id: request.app_id.clone(),
                            index,
                            total,
                        })
                        .is_err()
                    {
                        return;
                    }

                    let result = match &api_client {
                        Ok(client) => github::fetch_latest(client, &request.repo, token),
                        Err(error) => Err(error.clone()),
                    };

                    if events
                        .send(Event::CheckResult {
                            app_id: request.app_id,
                            result,
                        })
                        .is_err()
                    {
                        return;
                    }
                }

                if events.send(Event::CheckAllDone).is_err() {
                    return;
                }
            }
            Command::Install(request) => {
                let app_id = request.app_id.clone();
                let result = match &download_client {
                    Ok(client) => installer::install(client, &request, |event| {
                        let event = match event {
                            InstallEvent::Download(progress) => Event::DownloadProgress {
                                app_id: app_id.clone(),
                                downloaded: progress.downloaded,
                                total: progress.total,
                            },
                            InstallEvent::Extracting => Event::Installing {
                                app_id: app_id.clone(),
                            },
                        };
                        let _ = events.send(event);
                    }),
                    Err(error) => Err(error.clone()),
                };

                if events
                    .send(Event::InstallResult {
                        app_id: request.app_id,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Command::ExtractIcons {
                paths,
                platform,
                apps,
            } => {
                for app in apps {
                    let path = icons::auto_icon_path(&paths, &app, platform);
                    if events
                        .send(Event::IconReady {
                            app_id: app.id,
                            path,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
            Command::SyncManifest { url, token } => {
                let result = match &api_client {
                    Ok(client) => github::fetch_remote_manifest(
                        client,
                        &url,
                        token.as_deref().filter(|value| !value.trim().is_empty()),
                    ),
                    Err(error) => Err(error.clone()),
                };
                if events.send(Event::SyncResult { result }).is_err() {
                    return;
                }
            }
        }
    }
}
