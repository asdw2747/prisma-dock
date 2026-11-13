use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use reqwest::header::ACCEPT;

use crate::github::AssetInfo;
use crate::model::valid_id;
use crate::naming::normalize_match_key;

const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_SINGLE_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 50_000;
pub const CANCELED_MESSAGE: &str = "Installation canceled.";

#[derive(Clone)]
pub struct InstallRequest {
    pub app_id: String,
    pub app_name: String,
    pub version: String,
    pub asset: AssetInfo,
    pub token: Option<String>,
    pub apps_dir: PathBuf,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub struct InstalledArtifact {
    pub app_id: String,
    pub version: String,
    pub relative_path: String,
    pub asset_name: String,
}

#[derive(Debug, Clone, Copy)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub enum InstallEvent {
    Download(DownloadProgress),
    Extracting,
}

pub fn install(
    client: &Client,
    request: &InstallRequest,
    mut on_event: impl FnMut(InstallEvent),
) -> Result<InstalledArtifact, String> {
    if !valid_id(&request.app_id) {
        return Err(format!("Invalid app id {:?}.", request.app_id));
    }
    if request.asset.size > MAX_DOWNLOAD_BYTES {
        return Err(format!(
            "Release asset is too large ({} bytes); the limit is {} bytes.",
            request.asset.size, MAX_DOWNLOAD_BYTES
        ));
    }

    let app_dir = request.apps_dir.join(&request.app_id);
    fs::create_dir_all(&app_dir)
        .map_err(|error| format!("Could not create {}: {error}", app_dir.display()))?;

    let file_name = safe_asset_filename(&request.asset.name);
    let download_url = reqwest::Url::parse(&request.asset.browser_download_url)
        .map_err(|error| format!("Invalid release asset URL: {error}"))?;
    if download_url.scheme() != "https" {
        return Err("Refusing a release asset that does not use HTTPS.".to_string());
    }
    let download_host = download_url.host_str().unwrap_or_default().to_string();
    if !is_allowed_download_host(&download_host) {
        return Err(format!(
            "Refusing a release asset from unexpected host {download_host:?}."
        ));
    }

    let mut http = client
        .get(download_url)
        .header(ACCEPT, "application/octet-stream");
    if let Some(token) = &request.token
        && is_github_host(&download_host)
    {
        http = http.bearer_auth(token);
    }

    let mut response = http
        .send()
        .map_err(|error| format!("Download failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        let mut body = String::new();
        let _ = response.by_ref().take(64 * 1024).read_to_string(&mut body);
        return Err(format!(
            "Download failed with HTTP {status}{}",
            if body.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", body.trim())
            }
        ));
    }

    let content_length = response.content_length();
    let total = content_length.or_else(|| (request.asset.size > 0).then_some(request.asset.size));
    if let Some(length) = content_length
        && length > MAX_DOWNLOAD_BYTES
    {
        return Err(format!(
            "Release asset is too large ({length} bytes); the limit is {MAX_DOWNLOAD_BYTES} bytes."
        ));
    }

    ensure_not_cancelled(&request.cancel)?;
    let staging = StagingDir::create(&app_dir, &request.app_id)?;
    let destination = staging.path().join(&file_name);
    let partial = staging.path().join(format!(".{file_name}.part"));
    let mut output = File::create(&partial)
        .map_err(|error| format!("Could not create {}: {error}", partial.display()))?;

    let mut downloaded = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    on_event(InstallEvent::Download(DownloadProgress {
        downloaded,
        total,
    }));

    loop {
        ensure_not_cancelled(&request.cancel)?;
        let read = match response.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => return Err(format!("Download interrupted: {error}")),
        };
        let next_downloaded = downloaded.saturating_add(read as u64);
        if next_downloaded > MAX_DOWNLOAD_BYTES {
            return Err(format!(
                "Download exceeded the {MAX_DOWNLOAD_BYTES} byte safety limit."
            ));
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("Could not write download: {error}"))?;
        downloaded = next_downloaded;
        if last_emit.elapsed() >= Duration::from_millis(100)
            || total.is_some_and(|total| downloaded >= total)
        {
            on_event(InstallEvent::Download(DownloadProgress {
                downloaded,
                total,
            }));
            last_emit = Instant::now();
        }
    }

    ensure_not_cancelled(&request.cancel)?;
    if let Some(length) = content_length
        && downloaded != length
    {
        return Err(format!(
            "Download was incomplete: received {downloaded} of {length} bytes."
        ));
    }
    if content_length.is_none() && request.asset.size > 0 && downloaded != request.asset.size {
        return Err(format!(
            "Download size mismatch: received {downloaded} bytes, expected {}.",
            request.asset.size
        ));
    }

    output
        .sync_all()
        .map_err(|error| format!("Could not flush download: {error}"))?;
    drop(output);
    ensure_not_cancelled(&request.cancel)?;
    fs::rename(&partial, &destination)
        .map_err(|error| format!("Could not finalize download: {error}"))?;
    on_event(InstallEvent::Download(DownloadProgress {
        downloaded,
        total: total.or(Some(downloaded)),
    }));

    let lower_name = file_name.to_ascii_lowercase();
    let is_archive = lower_name.ends_with(".zip")
        || lower_name.ends_with(".tar.gz")
        || lower_name.ends_with(".tgz");

    let target = if is_archive {
        on_event(InstallEvent::Extracting);
        ensure_not_cancelled(&request.cancel)?;
        if lower_name.ends_with(".zip") {
            extract_zip(&destination, staging.path(), &request.cancel)?;
        } else {
            extract_tar_gz(&destination, staging.path(), &request.cancel)?;
        }
        ensure_not_cancelled(&request.cancel)?;
        fs::remove_file(&destination)
            .map_err(|error| format!("Could not remove extracted archive: {error}"))?;
        find_launcher(staging.path(), &request.app_name, &file_name)?
    } else {
        destination.clone()
    };

    if !target.is_file() {
        return Err(format!(
            "The installed launcher {} does not exist.",
            target.display()
        ));
    }

    #[cfg(unix)]
    make_executable(&target)?;
    ensure_not_cancelled(&request.cancel)?;

    let relative = target
        .strip_prefix(staging.path())
        .unwrap_or(&target)
        .to_path_buf();
    let final_dir = unique_install_dir(&app_dir, &request.version)?;
    staging.commit(&final_dir)?;
    let final_target = final_dir.join(&relative);
    let relative_path = final_target
        .strip_prefix(&app_dir)
        .unwrap_or(&final_target)
        .to_path_buf();

    Ok(InstalledArtifact {
        app_id: request.app_id.clone(),
        version: request.version.clone(),
        relative_path: relative_path.to_string_lossy().into_owned(),
        asset_name: file_name,
    })
}

fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err(CANCELED_MESSAGE.to_string())
    } else {
        Ok(())
    }
}

struct StagingDir {
    path: PathBuf,
    keep: bool,
}

impl StagingDir {
    fn create(app_dir: &Path, app_id: &str) -> Result<Self, String> {
        for attempt in 0..10u32 {
            let path = app_dir.join(format!(
                ".staging-{app_id}-{}-{}-{attempt}",
                std::process::id(),
                timestamp_nonce()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self { path, keep: false });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!("Could not create staging directory: {error}"));
                }
            }
        }
        Err("Could not allocate a unique staging directory.".to_string())
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn commit(mut self, final_dir: &Path) -> Result<(), String> {
        fs::rename(&self.path, final_dir)
            .map_err(|error| format!("Could not activate the staged update: {error}"))?;
        self.keep = true;

        #[cfg(unix)]
        {
            if let Ok(directory) = File::open(final_dir) {
                let _ = directory.sync_all();
            }
            if let Some(parent) = final_dir.parent()
                && let Ok(directory) = File::open(parent)
            {
                let _ = directory.sync_all();
            }
        }

        Ok(())
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn timestamp_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn sanitize_version(version: &str) -> String {
    let value: String = version
        .chars()
        .take(32)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let value = value.trim_matches(['.', ' ']).to_string();
    if value.is_empty() {
        "unknown".to_string()
    } else {
        value
    }
}

fn unique_install_dir(app_dir: &Path, version: &str) -> Result<PathBuf, String> {
    let version = sanitize_version(version);
    for attempt in 0..20u32 {
        let name = format!(
            "v{version}-{}-{}-{attempt}",
            std::process::id(),
            timestamp_nonce()
        );
        let path = app_dir.join(name);
        if !path.exists() {
            return Ok(path);
        }
    }
    Err("Could not allocate a unique install directory.".to_string())
}

fn copy_entry_with_limit(
    reader: &mut impl Read,
    writer: &mut impl Write,
    cancel: &AtomicBool,
    limit: u64,
) -> Result<u64, String> {
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;

    loop {
        ensure_not_cancelled(cancel)?;
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("Could not read archive entry: {error}"))?;
        if read == 0 {
            return Ok(total);
        }
        total = total.saturating_add(read as u64);
        if total > limit {
            return Err("Archive entry exceeds the extraction limit.".to_string());
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|error| format!("Could not write extracted file: {error}"))?;
    }
}

fn safe_asset_filename(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download.bin".to_string());

    let cleaned: String = base
        .chars()
        .map(|ch| {
            if ch == '/'
                || ch == '\\'
                || ch.is_control()
                || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*')
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(['.', ' ']).to_string();
    let cleaned = if is_windows_reserved_name(&cleaned) {
        format!("_{cleaned}")
    } else {
        cleaned
    };
    if cleaned.is_empty() {
        "download.bin".to_string()
    } else {
        cleaned
    }
}

fn is_windows_reserved_name(name: &str) -> bool {
    let stem = Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
}

fn is_github_host(host: &str) -> bool {
    matches!(
        host.to_ascii_lowercase().as_str(),
        "github.com" | "api.github.com" | "objects.githubusercontent.com" | "codeload.github.com"
    ) || host
        .to_ascii_lowercase()
        .ends_with(".githubusercontent.com")
}

fn is_allowed_download_host(host: &str) -> bool {
    is_github_host(host)
}

fn extract_zip(archive_path: &Path, destination: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|error| format!("Could not open {}: {error}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("Could not read zip archive: {error}"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("Archive has too many entries.".to_string());
    }
    let mut total_bytes = 0u64;

    for index in 0..archive.len() {
        ensure_not_cancelled(cancel)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not read zip entry: {error}"))?;

        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        if enclosed.as_os_str().is_empty() {
            continue;
        }

        if entry.is_dir() {
            fs::create_dir_all(destination.join(enclosed))
                .map_err(|error| format!("Could not create extracted folder: {error}"))?;
            continue;
        }

        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            continue;
        }

        let size = entry.size();
        if size > MAX_SINGLE_FILE_BYTES {
            continue;
        }

        let output_path = destination.join(enclosed);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create extracted folder: {error}"))?;
        }
        let mode = entry.unix_mode();
        let mut output = File::create(&output_path)
            .map_err(|error| format!("Could not create {}: {error}", output_path.display()))?;
        let copied = copy_entry_with_limit(&mut entry, &mut output, cancel, MAX_SINGLE_FILE_BYTES)?;
        output
            .sync_all()
            .map_err(|error| format!("Could not flush extracted file: {error}"))?;
        drop(output);
        total_bytes = total_bytes.saturating_add(copied);
        if total_bytes > MAX_EXTRACTED_BYTES {
            return Err("Archive expands beyond the 4 GB extraction limit.".to_string());
        }
        apply_unix_mode(&output_path, mode);
    }

    Ok(())
}

fn extract_tar_gz(
    archive_path: &Path,
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let file = File::open(archive_path)
        .map_err(|error| format!("Could not open {}: {error}", archive_path.display()))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("Could not read tar archive: {error}"))?;
    let mut total_bytes = 0u64;
    let mut count = 0usize;

    for entry in entries {
        ensure_not_cancelled(cancel)?;
        count += 1;
        if count > MAX_ARCHIVE_ENTRIES {
            return Err("Archive has too many entries.".to_string());
        }

        let mut entry = entry.map_err(|error| format!("Could not read tar entry: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("Could not read tar entry path: {error}"))?
            .to_path_buf();

        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            continue;
        }

        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(destination.join(&path))
                .map_err(|error| format!("Could not create extracted folder: {error}"))?;
            continue;
        }

        if !entry.header().entry_type().is_file() {
            continue;
        }

        let size = entry.size();
        if size > MAX_SINGLE_FILE_BYTES {
            continue;
        }

        let output_path = destination.join(&path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create extracted folder: {error}"))?;
        }
        let mode = entry.header().mode().ok();
        let mut output = File::create(&output_path)
            .map_err(|error| format!("Could not create {}: {error}", output_path.display()))?;
        let copied = copy_entry_with_limit(&mut entry, &mut output, cancel, MAX_SINGLE_FILE_BYTES)?;
        output
            .sync_all()
            .map_err(|error| format!("Could not flush extracted file: {error}"))?;
        drop(output);
        total_bytes = total_bytes.saturating_add(copied);
        if total_bytes > MAX_EXTRACTED_BYTES {
            return Err("Archive expands beyond the 4 GB extraction limit.".to_string());
        }
        apply_unix_mode(&output_path, mode);
    }

    Ok(())
}

fn find_launcher(app_dir: &Path, app_name: &str, asset_name: &str) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    collect_launcher_files(app_dir, 0, 4, &mut candidates)?;
    if candidates.is_empty() {
        return Err(format!(
            "The downloaded archive did not contain a usable {} launcher.",
            if cfg!(windows) {
                ".exe"
            } else {
                "AppImage or executable"
            }
        ));
    }

    let app_key = normalize_match_key(app_name);
    let asset_key = normalize_match_key(
        Path::new(asset_name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(asset_name),
    );

    #[cfg(target_os = "windows")]
    {
        candidates.retain(|candidate| {
            candidate
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        });
    }
    #[cfg(target_os = "linux")]
    {
        candidates.retain(|candidate| {
            let extension = candidate
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if extension == "appimage" {
                return true;
            }
            if matches!(
                extension.as_str(),
                "sh" | "so" | "desktop" | "py" | "pyw" | "txt" | "md" | "json"
            ) {
                return false;
            }
            is_executable(&candidate.path) && candidate.path.is_file()
        });
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        candidates.clear();
    }

    if candidates.is_empty() {
        return Err(format!(
            "The downloaded archive did not contain a usable {} launcher.",
            if cfg!(windows) { ".exe" } else { "executable" }
        ));
    }

    candidates.sort_by(|a, b| {
        let score = |candidate: &LauncherCandidate| {
            let stem = candidate
                .path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("");
            let key = normalize_match_key(stem);
            let mut score = 0i64;
            if key.contains(&app_key) {
                score += 30;
            }
            if key.contains(&asset_key) {
                score += 10;
            }
            if candidate.depth == 0 {
                score += 8;
            }
            let lower = candidate.path.to_string_lossy().to_ascii_lowercase();
            if lower.contains("setup") || lower.contains("installer") || lower.contains("unins") {
                score -= 40;
            }
            if lower.ends_with(".appimage") || lower.ends_with(".exe") {
                score += 10;
            }
            score
        };
        score(b)
            .cmp(&score(a))
            .then_with(|| a.depth.cmp(&b.depth))
            .then_with(|| a.path.as_os_str().len().cmp(&b.path.as_os_str().len()))
    });

    Ok(candidates.remove(0).path)
}

struct LauncherCandidate {
    path: PathBuf,
    depth: usize,
}

fn collect_launcher_files(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    output: &mut Vec<LauncherCandidate>,
) -> Result<(), String> {
    if output.len() >= 20_000 {
        return Ok(());
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if depth == 0 => {
            return Err(format!("Could not inspect extracted app: {error}"));
        }
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if depth < max_depth {
                let name = entry.file_name();
                let name = name.to_string_lossy().to_ascii_lowercase();
                if name.starts_with('.')
                    || matches!(
                        name.as_str(),
                        "__macosx" | "__pycache__" | "node_modules" | "target" | ".git"
                    )
                {
                    continue;
                }
                collect_launcher_files(&path, depth + 1, max_depth, output)?;
            }
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        output.push(LauncherCandidate { path, depth });
    }

    Ok(())
}

#[cfg(unix)]
fn apply_unix_mode(path: &Path, mode: Option<u32>) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        let mode = mode & 0o777;
        if mode != 0 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
        }
    }
}

#[cfg(not(unix))]
fn apply_unix_mode(_path: &Path, _mode: Option<u32>) {}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("Could not mark {} executable: {error}", path.display()))
}

#[cfg(target_os = "linux")]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    #[test]
    fn sanitizes_asset_names() {
        assert_eq!(safe_asset_filename("app.AppImage"), "app.AppImage");
        assert_eq!(safe_asset_filename("../../evil.exe"), "evil.exe");
        assert_eq!(safe_asset_filename(".."), "download.bin");
        assert_eq!(safe_asset_filename("bad\nname.zip"), "bad_name.zip");
        assert_eq!(safe_asset_filename("bad:name?.zip"), "bad_name_.zip");
        assert_eq!(safe_asset_filename("CON.zip"), "_CON.zip");
    }

    #[cfg(unix)]
    #[test]
    fn marks_downloaded_binaries_executable() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("Tool.AppImage");
        std::fs::write(&binary, b"fixture").unwrap();
        std::fs::set_permissions(&binary, fs::Permissions::from_mode(0o644)).unwrap();

        make_executable(&binary).unwrap();
        let mode = std::fs::metadata(&binary).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn finds_appimage_launcher() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("bundle")).unwrap();
        std::fs::write(root.join("bundle").join("ComicCrate.AppImage"), b"app").unwrap();
        std::fs::write(root.join("README.txt"), b"hi").unwrap();

        let found = if cfg!(windows) {
            std::fs::write(root.join("ComicCrate.exe"), b"exe").unwrap();
            find_launcher(root, "ComicCrate", "bundle.tar.gz").unwrap()
        } else {
            find_launcher(root, "ComicCrate", "bundle.tar.gz").unwrap()
        };

        assert!(found.is_file());
    }

    #[test]
    fn rejects_oversized_assets_before_downloading() {
        let temp = tempfile::tempdir().unwrap();
        let request = InstallRequest {
            app_id: "tool".to_string(),
            app_name: "Tool".to_string(),
            version: "1.0.0".to_string(),
            asset: AssetInfo {
                name: "Tool.zip".to_string(),
                browser_download_url:
                    "https://github.com/example/tool/releases/download/v1/Tool.zip".to_string(),
                size: MAX_DOWNLOAD_BYTES + 1,
            },
            token: None,
            apps_dir: temp.path().join("apps"),
            cancel: Arc::new(AtomicBool::new(false)),
        };

        let error = install(&Client::new(), &request, |_| {}).unwrap_err();
        assert!(error.contains("too large"), "{error}");
        assert!(!temp.path().join("apps").exists());
    }

    #[test]
    fn staging_directory_is_removed_unless_committed() {
        let temp = tempfile::tempdir().unwrap();
        let app_dir = temp.path().join("apps").join("tool");
        std::fs::create_dir_all(&app_dir).unwrap();

        let staging = StagingDir::create(&app_dir, "tool").unwrap();
        let abandoned = staging.path().to_path_buf();
        assert!(abandoned.is_dir());
        drop(staging);
        assert!(!abandoned.exists());

        let staging = StagingDir::create(&app_dir, "tool").unwrap();
        let final_dir = app_dir.join("v1.0.0-test");
        staging.commit(&final_dir).unwrap();
        assert!(final_dir.is_dir());
    }

    #[test]
    fn extraction_copy_enforces_limit_and_cancellation() {
        let cancel = AtomicBool::new(false);
        let mut input = Cursor::new(vec![0u8; 10]);
        let mut output = Vec::new();
        assert!(copy_entry_with_limit(&mut input, &mut output, &cancel, 5).is_err());
        assert!(output.len() <= 5);

        let cancel = AtomicBool::new(true);
        let mut input = Cursor::new(vec![0u8; 1]);
        let mut output = Vec::new();
        assert_eq!(
            copy_entry_with_limit(&mut input, &mut output, &cancel, 5),
            Err(CANCELED_MESSAGE.to_string())
        );
    }

    #[test]
    fn zip_extraction_rejects_path_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("evil.zip");
        let destination = temp.path().join("extract");
        std::fs::create_dir(&destination).unwrap();

        {
            let file = File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer.start_file("../evil.txt", options).unwrap();
            writer.write_all(b"owned").unwrap();
            writer.finish().unwrap();
        }

        extract_zip(&archive_path, &destination, &AtomicBool::new(false)).unwrap();

        assert!(!temp.path().join("evil.txt").exists());
        assert!(!destination.join("evil.txt").exists());
    }
}
