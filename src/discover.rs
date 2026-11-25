use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::github::{DISCOVERY_EXCLUDED_REPO, DiscoveredRepo, ReleaseInfo};
use crate::model::{AppEntry, Manifest, Platform};
use crate::naming::{normalize_match_key, slugify, unique_id, version_from_name};
use crate::paths::portable_path_string;

struct KnownApp {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    category: &'static str,
    repo: &'static str,
    version: &'static str,
    aliases: &'static [&'static str],
}

const KNOWN_APPS: &[KnownApp] = &[
    KnownApp {
        id: "comiccrate",
        name: "ComicCrate",
        description: "Convert and read comic archives with a fast native reader.",
        category: "Comics",
        repo: "asdw2747/comic-crate",
        version: "1.0.1",
        aliases: &["comiccrate", "comic-crate", "comic_crate"],
    },
    KnownApp {
        id: "reelrename",
        name: "ReelRename",
        description: "Rename messy Plex and Jellyfin media using TMDB metadata.",
        category: "Media",
        repo: "asdw2747/reel-rename",
        version: "1.0.0",
        aliases: &["reelrename", "reel-rename", "reel_rename"],
    },
    KnownApp {
        id: "haunted-files",
        name: "Haunted Files",
        description: "Browse your filesystem as an atmospheric, read-only abandoned room.",
        category: "Files",
        repo: "asdw2747/haunted-files-py",
        version: "1.0.0",
        aliases: &["hauntedfiles", "haunted-files", "haunted_files"],
    },
    KnownApp {
        id: "jxlerate",
        name: "JXLerate",
        description: "Losslessly archive and optimize images, with JPEG XL support.",
        category: "Media",
        repo: "asdw2747/JXLerate",
        version: "2.4.4",
        aliases: &["jxlerate", "jxl-rate", "jxl_rate"],
    },
    KnownApp {
        id: "matrix-track",
        name: "MatrixTrack",
        description: "Multi-channel audio visualizer for surround mapping and phase checks.",
        category: "Audio",
        repo: "asdw2747/MatrixTrack",
        version: "1.1.0",
        aliases: &["matrixtrack", "matrix-track", "matrix_track"],
    },
    KnownApp {
        id: "surroundtag",
        name: "SurroundTag",
        description: "Tag and embed cover art in 5.1 Matroska audio without re-encoding.",
        category: "Audio",
        repo: "asdw2747/surround-tag",
        version: "1.0.0",
        aliases: &["surroundtag", "surround-tag", "surround_tag"],
    },
];

pub fn starter_manifest(base: &Path, platform: Platform) -> (Manifest, usize) {
    let mut apps = Vec::with_capacity(KNOWN_APPS.len());
    let mut discovered = 0usize;
    let roots = discovery_roots(base);
    let mut seen_roots = HashSet::new();

    for known in KNOWN_APPS {
        let mut app = AppEntry {
            id: known.id.to_string(),
            name: known.name.to_string(),
            description: known.description.to_string(),
            category: known.category.to_string(),
            repo: Some(known.repo.to_string()),
            installed_version: Some(known.version.to_string()),
            icon_path: None,
            windows: None,
            linux: None,
        };

        if platform != Platform::Other {
            let aliases: Vec<&str> = known.aliases.to_vec();
            if let Some(found) = discover_in_roots(&roots, &aliases, platform, &mut seen_roots) {
                app.set_binary_target_for(platform, found.path);
                if let Some(version) = found.version {
                    app.installed_version = Some(version);
                }
                discovered += 1;
            }
        }

        apps.push(app);
    }

    (Manifest { apps }, discovered)
}

/// Append newly discovered GitHub apps without modifying any existing entry.
///
/// Duplicate and excluded repos are re-checked here so a manifest change
/// between discovery and UI handling cannot introduce duplicates. Returns the
/// new app IDs paired with their cached release metadata.
pub fn append_github_apps(
    manifest: &mut Manifest,
    discovered: Vec<DiscoveredRepo>,
) -> Vec<(String, ReleaseInfo)> {
    let mut existing_repos: HashSet<String> = manifest
        .apps
        .iter()
        .filter_map(|app| app.repo.as_deref())
        .map(|repo| repo.trim().to_ascii_lowercase())
        .filter(|repo| !repo.is_empty())
        .collect();
    let mut ids: BTreeSet<String> = manifest.apps.iter().map(|app| app.id.clone()).collect();
    let mut added = Vec::new();

    for repo in discovered {
        if repo.name.eq_ignore_ascii_case(DISCOVERY_EXCLUDED_REPO) {
            continue;
        }

        let repo_key = repo.full_name.trim().to_ascii_lowercase();
        if repo_key.is_empty() || !existing_repos.insert(repo_key) {
            continue;
        }

        let name = if repo.name.trim().is_empty() {
            "New App".to_string()
        } else {
            repo.name.clone()
        };
        let id = unique_id(&slugify(&name), &ids);
        ids.insert(id.clone());

        manifest.apps.push(AppEntry {
            id: id.clone(),
            name,
            description: repo.description.unwrap_or_default(),
            category: "Other".to_string(),
            repo: Some(repo.full_name),
            installed_version: None,
            icon_path: None,
            windows: None,
            linux: None,
        });
        added.push((id, repo.release));
    }

    added
}

struct DiscoveryFound {
    path: String,
    version: Option<String>,
}

#[derive(Clone)]
struct SearchRoot {
    path: PathBuf,
    depth: usize,
    portable: bool,
}

fn discovery_roots(base: &Path) -> Vec<SearchRoot> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();

    let mut add = |path: PathBuf, depth: usize, portable: bool| {
        if path.is_dir() && seen.insert(path.clone()) {
            roots.push(SearchRoot {
                path,
                depth,
                portable,
            });
        }
    };

    add(base.join("apps"), 3, true);

    if let Some(home) = dirs::home_dir() {
        add(home.join("Desktop"), 2, false);
        add(home.join("Downloads"), 2, false);
        add(home.join("Applications"), 2, false);
        add(home.join(".local").join("bin"), 0, false);
        add(home.join("bin"), 1, false);
    }

    if let Some(desktop) = dirs::desktop_dir() {
        add(desktop, 2, false);
    }
    if let Some(downloads) = dirs::download_dir() {
        add(downloads, 2, false);
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(local) = dirs::data_local_dir() {
            add(local.join("Programs"), 3, false);
        }
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            add(PathBuf::from(program_files), 2, false);
        }
        if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
            add(PathBuf::from(program_files_x86), 2, false);
        }
    }

    roots
}

fn discover_in_roots(
    roots: &[SearchRoot],
    aliases: &[&str],
    platform: Platform,
    seen_paths: &mut HashSet<PathBuf>,
) -> Option<DiscoveryFound> {
    let alias_keys: Vec<String> = aliases
        .iter()
        .map(|alias| normalize_match_key(alias))
        .filter(|alias| !alias.is_empty())
        .collect();

    let mut best: Option<Candidate> = None;
    for root in roots {
        let portable_root = root.portable;
        let mut visited = 0usize;
        walk_dirs(
            root,
            &root.path,
            root.depth,
            &alias_keys,
            platform,
            portable_root,
            seen_paths,
            &mut visited,
            &mut best,
        );
    }

    best.map(|candidate| DiscoveryFound {
        path: portable_path_string(&candidate.path),
        version: candidate.version,
    })
}

struct Candidate {
    path: PathBuf,
    version: Option<String>,
    score: u64,
    mtime: SystemTime,
    depth: usize,
}

#[allow(clippy::too_many_arguments)]
fn walk_dirs(
    root: &SearchRoot,
    dir: &Path,
    depth_left: usize,
    alias_keys: &[String],
    platform: Platform,
    portable_root: bool,
    seen_paths: &mut HashSet<PathBuf>,
    visited: &mut usize,
    best: &mut Option<Candidate>,
) {
    if *visited >= 6000 {
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        if *visited >= 6000 {
            return;
        }

        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            if depth_left == 0 {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || is_skipped_dir(&name) {
                continue;
            }
            walk_dirs(
                root,
                &path,
                depth_left - 1,
                alias_keys,
                platform,
                portable_root,
                seen_paths,
                visited,
                best,
            );
            continue;
        }

        if !file_type.is_file() {
            continue;
        }
        *visited += 1;

        let name = entry.file_name();
        let name = name.to_string_lossy();
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(&name);

        let normalized_stem = normalize_match_key(stem);
        let matches_alias = alias_keys
            .iter()
            .any(|alias| normalized_stem.contains(alias.as_str()));
        if !matches_alias {
            continue;
        }

        let Some(kind_score) = candidate_kind_score(&path, platform) else {
            continue;
        };
        if !seen_paths.insert(path.clone()) {
            continue;
        }

        let version = version_from_name(stem);
        let mut score = kind_score;
        if portable_root {
            score += 100;
        }
        if version.is_some() {
            score += 10;
        }
        if depth_left == root.depth {
            score += 5;
        }
        if name.to_ascii_lowercase().contains("portable") {
            score += 2;
        }

        let mtime = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH);
        let candidate = Candidate {
            path,
            version,
            score,
            mtime,
            depth: root.depth - depth_left,
        };

        let replace = match best {
            Some(current) => {
                candidate.score > current.score
                    || (candidate.score == current.score && candidate.mtime > current.mtime)
                    || (candidate.score == current.score
                        && candidate.mtime == current.mtime
                        && candidate.depth < current.depth)
            }
            None => true,
        };
        if replace {
            *best = Some(candidate);
        }
    }
}

fn is_skipped_dir(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "node_modules" | "target" | "__pycache__" | "venv" | "env" | "site-packages"
    )
}

fn candidate_kind_score(path: &Path, platform: Platform) -> Option<u64> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match platform {
        Platform::Windows => (extension == "exe").then_some(60),
        Platform::Linux => {
            if extension == "appimage" {
                Some(60)
            } else if (extension.is_empty() || extension == "bin") && is_executable(path) {
                Some(50)
            } else {
                None
            }
        }
        Platform::Other => None,
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_linux_appimage_in_expected_root() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("ComicCrate-1.0.1-x86_64.AppImage");
        std::fs::write(&file, b"not a real appimage").unwrap();

        let roots = vec![SearchRoot {
            path: temp.path().to_path_buf(),
            depth: 1,
            portable: false,
        }];
        let mut seen = HashSet::new();
        let found = discover_in_roots(
            &roots,
            &["comiccrate", "comic-crate"],
            Platform::Linux,
            &mut seen,
        )
        .expect("appimage should be discovered");

        assert_eq!(found.version.as_deref(), Some("1.0.1"));
        assert!(found.path.ends_with("ComicCrate-1.0.1-x86_64.AppImage"));
    }

    #[test]
    fn ignores_other_platform_assets() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("ComicCrate-windows-x64.zip"), b"zip").unwrap();
        std::fs::write(temp.path().join("ComicCrate.exe"), b"exe").unwrap();

        let roots = vec![SearchRoot {
            path: temp.path().to_path_buf(),
            depth: 1,
            portable: false,
        }];
        let mut seen = HashSet::new();
        assert!(discover_in_roots(&roots, &["comiccrate"], Platform::Linux, &mut seen).is_none());
    }

    #[test]
    fn starter_manifest_has_exact_six_apps_and_repos() {
        let temp = tempfile::tempdir().unwrap();
        let (manifest, _) = starter_manifest(temp.path(), Platform::Linux);
        assert_eq!(manifest.apps.len(), 6);

        let expected = [
            ("comiccrate", "ComicCrate", "asdw2747/comic-crate"),
            ("reelrename", "ReelRename", "asdw2747/reel-rename"),
            (
                "haunted-files",
                "Haunted Files",
                "asdw2747/haunted-files-py",
            ),
            ("jxlerate", "JXLerate", "asdw2747/JXLerate"),
            ("matrix-track", "MatrixTrack", "asdw2747/MatrixTrack"),
            ("surroundtag", "SurroundTag", "asdw2747/surround-tag"),
        ];
        for (app, (id, name, repo)) in manifest.apps.iter().zip(expected) {
            assert_eq!(app.id, id);
            assert_eq!(app.name, name);
            assert_eq!(app.repo.as_deref(), Some(repo));
        }

        let matrix_track = &manifest.apps[4];
        assert_eq!(matrix_track.category, "Audio");
        assert_eq!(
            matrix_track.description,
            "Multi-channel audio visualizer for surround mapping and phase checks."
        );

        let surround_tag = &manifest.apps[5];
        assert_eq!(surround_tag.category, "Audio");
        assert_eq!(
            surround_tag.description,
            "Tag and embed cover art in 5.1 Matroska audio without re-encoding."
        );
    }

    #[test]
    fn discovers_matrix_track_by_alias() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("MatrixTrack-1.1.0-x86_64.AppImage");
        std::fs::write(&file, b"not a real appimage").unwrap();

        let roots = vec![SearchRoot {
            path: temp.path().to_path_buf(),
            depth: 1,
            portable: false,
        }];
        let mut seen = HashSet::new();
        let found = discover_in_roots(
            &roots,
            &["matrixtrack", "matrix-track", "matrix_track"],
            Platform::Linux,
            &mut seen,
        )
        .expect("MatrixTrack should be discovered");

        assert_eq!(found.version.as_deref(), Some("1.1.0"));
        assert!(found.path.ends_with("MatrixTrack-1.1.0-x86_64.AppImage"));
    }

    fn discovered_repo(name: &str, full_name: &str, version: &str) -> DiscoveredRepo {
        DiscoveredRepo {
            name: name.to_string(),
            full_name: full_name.to_string(),
            description: Some("A newly published tool".to_string()),
            release: ReleaseInfo {
                version: version.to_string(),
                tag: format!("v{version}"),
                html_url: String::new(),
                name: None,
                selected: None,
            },
        }
    }

    #[test]
    fn append_github_apps_dedupes_and_ignores_prisma_dock() {
        let existing = AppEntry {
            id: "existing".to_string(),
            name: "Existing".to_string(),
            description: "A custom app".to_string(),
            category: "Tools".to_string(),
            repo: Some("asdw2747/Existing".to_string()),
            installed_version: Some("9.9.9".to_string()),
            icon_path: Some("data/icons/existing.png".to_string()),
            windows: None,
            linux: Some(crate::model::LaunchTarget::Binary {
                path: "/custom/Existing.AppImage".to_string(),
            }),
        };
        let before = serde_json::to_value(&existing).unwrap();
        let mut manifest = Manifest {
            apps: vec![existing],
        };

        let added = append_github_apps(
            &mut manifest,
            vec![
                discovered_repo("new-app", "asdw2747/new-app", "1.2.3"),
                discovered_repo("EXISTING", "ASDW2747/existing", "9.9.9"),
                discovered_repo("prisma-dock", "asdw2747/prisma-dock", "1.0.0"),
            ],
        );

        assert_eq!(added.len(), 1);
        assert_eq!(added[0].0, "new-app");
        assert_eq!(added[0].1.version, "1.2.3");
        assert_eq!(manifest.apps.len(), 2);
        assert_eq!(
            serde_json::to_value(&manifest.apps[0]).unwrap(),
            before,
            "existing entry must not be altered"
        );

        let app = &manifest.apps[1];
        assert_eq!(app.id, "new-app");
        assert_eq!(app.name, "new-app");
        assert_eq!(app.description, "A newly published tool");
        assert_eq!(app.category, "Other");
        assert_eq!(app.repo.as_deref(), Some("asdw2747/new-app"));
        assert_eq!(app.installed_version, None);
        assert_eq!(app.icon_path, None);
        assert_eq!(app.windows, None);
        assert_eq!(app.linux, None);
        assert!(!manifest.apps.iter().any(|app| app.name == "prisma-dock"));
    }
}
