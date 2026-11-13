use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::{AppEntry, LaunchTarget, Platform};
use crate::naming::{humanize_file_stem, normalize_match_key, version_from_name};
use crate::paths::portable_path_string;

pub const CATEGORIES: &[&str] = &["Media", "Files", "Comics", "Images", "Tools", "Other"];

const MAX_CANDIDATES: usize = 120;
const MAX_VISITED: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    WindowsExe,
    LinuxAppImage,
    Python,
}

impl CandidateKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::WindowsExe => "Windows .exe",
            Self::LinuxAppImage => "Linux AppImage",
            Self::Python => "Python script",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScannedCandidate {
    pub name: String,
    pub path: PathBuf,
    pub kind: CandidateKind,
    pub version: String,
    pub venv_path: Option<PathBuf>,
    pub folder: PathBuf,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub candidates: Vec<ScannedCandidate>,
    pub truncated: bool,
}

pub fn scan_folder(root: &Path) -> Result<ScanResult, String> {
    if !root.is_dir() {
        return Err(format!("{} is not a folder.", root.display()));
    }

    let mut result = ScanResult::default();
    let mut seen = HashSet::new();
    let mut visited = 0usize;
    walk(root, 0, 3, &mut seen, &mut visited, &mut result)?;

    result.candidates.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(result)
}

fn walk(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    seen: &mut HashSet<PathBuf>,
    visited: &mut usize,
    result: &mut ScanResult,
) -> Result<(), String> {
    if result.candidates.len() >= MAX_CANDIDATES {
        result.truncated = true;
        return Ok(());
    }
    if *visited >= MAX_VISITED {
        result.truncated = true;
        return Ok(());
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if depth == 0 => {
            return Err(format!("Could not scan {}: {error}", dir.display()));
        }
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        if *visited >= MAX_VISITED || result.candidates.len() >= MAX_CANDIDATES {
            result.truncated = true;
            break;
        }

        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            if depth >= max_depth {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy().to_ascii_lowercase();
            if name.starts_with('.') || is_skipped_dir(&name) {
                continue;
            }
            walk(&path, depth + 1, max_depth, seen, visited, result)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }
        *visited += 1;

        let Some(candidate) = candidate_from_path_inner(&path, seen) else {
            continue;
        };
        result.candidates.push(candidate);
    }

    Ok(())
}

fn is_skipped_dir(name: &str) -> bool {
    matches!(
        name,
        "target" | "node_modules" | "__pycache__" | "venv" | "env" | "site-packages" | ".git"
    )
}

pub fn candidate_from_path(path: &Path) -> Result<ScannedCandidate, String> {
    let path = absolute_path(path);
    if !path.is_file() {
        return Err(format!("{} is not a file.", path.display()));
    }
    let mut seen = HashSet::new();
    candidate_from_path_inner(&path, &mut seen)
        .ok_or_else(|| "Choose a .exe, .AppImage, .py, or .pyw file.".to_string())
}

fn candidate_from_path_inner(path: &Path, seen: &mut HashSet<PathBuf>) -> Option<ScannedCandidate> {
    if !seen.insert(path.to_path_buf()) {
        return None;
    }

    let kind = classify(path)?;
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();

    if kind == CandidateKind::Python && is_ignored_python_file(stem) {
        return None;
    }

    let name_stem = if kind == CandidateKind::Python && is_generic_python_stem(stem) {
        path.parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or(stem)
    } else {
        stem
    };

    let name = humanize_file_stem(name_stem);
    let folder = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let venv_path = (kind == CandidateKind::Python)
        .then(|| find_venv(&folder))
        .flatten();

    Some(ScannedCandidate {
        name,
        path: path.to_path_buf(),
        kind,
        version: version_from_name(stem).unwrap_or_default(),
        venv_path,
        folder,
    })
}

fn classify(path: &Path) -> Option<CandidateKind> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "exe" => Some(CandidateKind::WindowsExe),
        "appimage" => Some(CandidateKind::LinuxAppImage),
        "py" | "pyw" => Some(CandidateKind::Python),
        _ => None,
    }
}

fn is_ignored_python_file(stem: &str) -> bool {
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "setup" | "conftest" | "__init__" | "__main__"
    )
}

fn is_generic_python_stem(stem: &str) -> bool {
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "main" | "app" | "gui" | "run" | "launcher" | "start"
    )
}

fn find_venv(folder: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    let mut current = Some(folder);
    for _ in 0..3 {
        let Some(dir) = current else {
            break;
        };
        candidates.push(dir.join(".venv"));
        candidates.push(dir.join("venv"));
        candidates.push(dir.join("env"));
        current = dir.parent();
    }

    candidates
        .into_iter()
        .find(|venv| venv_python(venv).is_file())
}

fn venv_python(venv: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        venv.join("Scripts").join("python.exe")
    }
    #[cfg(not(target_os = "windows"))]
    {
        venv.join("bin").join("python")
    }
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

pub fn infer_category(name: &str) -> &'static str {
    let key = normalize_match_key(name);
    if key.contains("comic") {
        "Comics"
    } else if key.contains("haunted") || key.contains("file") {
        "Files"
    } else if key.contains("reel")
        || key.contains("rename")
        || key.contains("jxl")
        || key.contains("image")
        || key.contains("media")
        || key.contains("video")
        || key.contains("audio")
    {
        "Media"
    } else {
        "Other"
    }
}

pub fn entry_from_candidate(
    candidate: &ScannedCandidate,
    id: String,
    name: String,
    category: String,
) -> AppEntry {
    let platform = Platform::current();
    let mut entry = AppEntry {
        id,
        name,
        description: format!("Added from {}", candidate.folder.display()),
        category,
        repo: None,
        installed_version: (!candidate.version.is_empty()).then(|| candidate.version.clone()),
        icon_path: None,
        windows: None,
        linux: None,
    };

    match candidate.kind {
        CandidateKind::WindowsExe => {
            entry.set_binary_target_for(Platform::Windows, portable_path_string(&candidate.path));
        }
        CandidateKind::LinuxAppImage => {
            entry.set_binary_target_for(Platform::Linux, portable_path_string(&candidate.path));
        }
        CandidateKind::Python => {
            let venv_path = candidate
                .venv_path
                .as_ref()
                .map(|path| portable_path_string(path));
            entry.set_target_for(
                platform,
                LaunchTarget::Python {
                    script_path: portable_path_string(&candidate.path),
                    venv_path,
                    windowless: candidate
                        .path
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("pyw")),
                },
            );
        }
    }

    entry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_known_launchable_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join("ComicCrate-1.0.1-x86_64.AppImage"), b"app").unwrap();
        std::fs::write(root.join("notes.txt"), b"ignore").unwrap();
        std::fs::write(root.join("setup.py"), b"ignore").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("HauntedFiles.exe"), b"exe").unwrap();

        let result = scan_folder(root).unwrap();
        assert_eq!(result.candidates.len(), 2);
        assert!(result.candidates.iter().any(|c| c.name == "ComicCrate"));
        assert!(result.candidates.iter().any(|c| c.name == "HauntedFiles"));
        assert!(
            !result
                .candidates
                .iter()
                .any(|c| c.path.ends_with("setup.py"))
        );
    }

    #[test]
    fn detects_venv_for_python_script() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join(".venv").join("bin")).unwrap();
        std::fs::create_dir_all(root.join(".venv").join("Scripts")).unwrap();
        #[cfg(target_os = "windows")]
        std::fs::write(root.join(".venv").join("Scripts").join("python.exe"), b"").unwrap();
        #[cfg(not(target_os = "windows"))]
        std::fs::write(root.join(".venv").join("bin").join("python"), b"").unwrap();
        let script = root.join("tool.py");
        std::fs::write(&script, b"print('hi')").unwrap();

        let candidate = candidate_from_path(&script).unwrap();
        assert!(candidate.venv_path.is_some());
        assert_eq!(candidate.name, "Tool");
    }

    #[test]
    fn rejects_unsupported_extensions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("readme.txt");
        std::fs::write(&path, b"hi").unwrap();
        assert!(candidate_from_path(&path).is_err());
    }

    #[test]
    fn windowless_python_scripts_are_marked() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("tool.pyw");
        std::fs::write(&path, b"print('hi')").unwrap();

        let candidate = candidate_from_path(&path).unwrap();
        let entry = entry_from_candidate(
            &candidate,
            "tool".to_string(),
            "Tool".to_string(),
            "Tools".to_string(),
        );
        match entry.target_for(Platform::current()) {
            Some(LaunchTarget::Python { windowless, .. }) => assert!(*windowless),
            other => panic!("unexpected target: {other:?}"),
        }
    }

    #[test]
    fn infers_categories() {
        assert_eq!(infer_category("Comic Crate"), "Comics");
        assert_eq!(infer_category("Haunted Files"), "Files");
        assert_eq!(infer_category("Reel Rename"), "Media");
        assert_eq!(infer_category("Calculator"), "Other");
    }
}
