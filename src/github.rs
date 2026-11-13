use std::cmp::Ordering;
use std::collections::HashSet;
use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, HeaderMap};
use serde::Deserialize;

use crate::model::{Manifest, Platform};

pub const USER_AGENT: &str = concat!("prisma-dock/", env!("CARGO_PKG_VERSION"));
pub const DISCOVERY_OWNER: &str = "asdw2747";
pub const DISCOVERY_EXCLUDED_REPO: &str = "prisma-dock";
pub const NO_RELEASES_MESSAGE: &str =
    "No releases found. Check the repo name, or add a token for a private repository.";
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ERROR_BYTES: u64 = 64 * 1024;
const MAX_REPO_PAGES: usize = 20;

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub version: String,
    pub tag: String,
    pub html_url: String,
    pub name: Option<String>,
    pub selected: Option<AssetInfo>,
}

#[derive(Debug, Clone)]
pub struct RepoSummary {
    pub name: String,
    pub full_name: String,
    pub description: Option<String>,
    pub fork: bool,
    pub archived: bool,
    pub private: bool,
    pub owner: String,
}

#[derive(Debug, Clone)]
pub struct DiscoveredRepo {
    pub name: String,
    pub full_name: String,
    pub description: Option<String>,
    pub release: ReleaseInfo,
}

#[derive(Debug, Default)]
pub struct DiscoveryOutcome {
    pub discovered: Vec<DiscoveredRepo>,
    pub errors: Vec<String>,
    pub candidates: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetInfo {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Deserialize)]
struct GithubRepo {
    name: String,
    full_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    fork: bool,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    private: bool,
    owner: GithubOwner,
}

#[derive(Deserialize)]
struct GithubOwner {
    login: String,
}

pub fn api_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("Could not create GitHub client: {error}"))
}

pub fn download_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(1800))
        .build()
        .map_err(|error| format!("Could not create download client: {error}"))
}

pub fn fetch_latest(
    client: &Client,
    repo: &str,
    token: Option<&str>,
) -> Result<ReleaseInfo, String> {
    fetch_latest_optional(client, repo, token)?.ok_or_else(|| NO_RELEASES_MESSAGE.to_string())
}

pub fn fetch_latest_optional(
    client: &Client,
    repo: &str,
    token: Option<&str>,
) -> Result<Option<ReleaseInfo>, String> {
    let (owner, name) = parse_repo(repo)?;
    let url = format!("https://api.github.com/repos/{owner}/{name}/releases/latest");

    let mut request = client
        .get(url)
        .header(ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
        .map_err(|error| format!("GitHub request failed: {error}"))?;
    let status = response.status();
    let headers = response.headers().clone();

    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }

    if !status.is_success() {
        let body = read_limited(response, MAX_ERROR_BYTES).unwrap_or_default();
        return Err(github_error(status, &headers, &body));
    }

    let release: GithubRelease = response
        .json()
        .map_err(|error| format!("GitHub returned an invalid release: {error}"))?;

    let assets: Vec<AssetInfo> = release
        .assets
        .into_iter()
        .map(|asset| AssetInfo {
            name: asset.name,
            browser_download_url: asset.browser_download_url,
            size: asset.size,
        })
        .collect();

    let selected = select_asset(&assets, Platform::current());
    let tag = release.tag_name;
    let version = clean_version(&tag);

    Ok(Some(ReleaseInfo {
        version,
        tag,
        html_url: release.html_url,
        name: release.name,
        selected,
    }))
}

pub fn list_user_repos(
    client: &Client,
    owner: &str,
    token: Option<&str>,
) -> Result<Vec<RepoSummary>, String> {
    if !valid_repo_component(owner) {
        return Err(format!("Invalid GitHub owner {owner:?}."));
    }

    let mut repos = Vec::new();
    for page in 1..=MAX_REPO_PAGES {
        let url = format!(
            "https://api.github.com/users/{owner}/repos?per_page=100&page={page}&type=owner&sort=updated"
        );
        let mut request = client
            .get(url)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .map_err(|error| format!("GitHub repo listing failed: {error}"))?;
        let status = response.status();
        let headers = response.headers().clone();
        if !status.is_success() {
            let body = read_limited(response, MAX_ERROR_BYTES).unwrap_or_default();
            if status == StatusCode::NOT_FOUND {
                return Err(format!("GitHub user {owner} was not found."));
            }
            return Err(github_error(status, &headers, &body));
        }

        let page_items: Vec<GithubRepo> = response
            .json()
            .map_err(|error| format!("GitHub returned an invalid repo list: {error}"))?;
        let page_len = page_items.len();
        repos.extend(page_items.into_iter().map(|repo| RepoSummary {
            name: repo.name,
            full_name: repo.full_name,
            description: repo.description,
            fork: repo.fork,
            archived: repo.archived,
            private: repo.private,
            owner: repo.owner.login,
        }));

        if page_len < 100 {
            break;
        }
    }

    Ok(repos)
}

/// Filter the public repo list down to new, release-backed apps.
///
/// The release lookup is injected so the filtering and deduplication logic can
/// be tested without network access. The caller is expected to provide
/// `existing_repos` in any casing; keys are normalized here.
pub fn discover_repos<F>(
    repos: Vec<RepoSummary>,
    owner: &str,
    existing_repos: &HashSet<String>,
    mut fetch_release: F,
) -> DiscoveryOutcome
where
    F: FnMut(&str) -> Result<Option<ReleaseInfo>, String>,
{
    let mut outcome = DiscoveryOutcome::default();
    let mut seen: HashSet<String> = existing_repos
        .iter()
        .map(|repo| repo.trim().to_ascii_lowercase())
        .filter(|repo| !repo.is_empty())
        .collect();

    for repo in repos {
        if !repo.owner.eq_ignore_ascii_case(owner)
            || repo.fork
            || repo.archived
            || repo.private
            || repo.name.eq_ignore_ascii_case(DISCOVERY_EXCLUDED_REPO)
        {
            continue;
        }

        let repo_key = repo.full_name.trim().to_ascii_lowercase();
        if repo_key.is_empty() || !seen.insert(repo_key) {
            continue;
        }

        outcome.candidates += 1;
        match fetch_release(&repo.full_name) {
            Ok(Some(release)) => outcome.discovered.push(DiscoveredRepo {
                name: repo.name,
                full_name: repo.full_name,
                description: repo.description,
                release,
            }),
            Ok(None) => {}
            Err(error) => outcome.errors.push(format!("{}: {error}", repo.full_name)),
        }
    }

    outcome
}

pub fn fetch_remote_manifest(
    client: &Client,
    url: &str,
    token: Option<&str>,
) -> Result<Manifest, String> {
    let url = normalize_manifest_url(url, token.is_some())?;
    let host = url.host_str().unwrap_or_default().to_string();
    let accept = if host == "api.github.com" {
        "application/vnd.github.raw"
    } else {
        "application/json"
    };

    let mut request = client.get(url.clone()).header(ACCEPT, accept);
    if let Some(token) = token
        && is_github_host(&host)
    {
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
        .map_err(|error| format!("Could not fetch remote apps.json: {error}"))?;
    let status = response.status();
    let headers = response.headers().clone();
    if !status.is_success() {
        let body = read_limited(response, MAX_ERROR_BYTES).unwrap_or_default();
        if status == StatusCode::NOT_FOUND {
            return Err(
                "Remote apps.json was not found. Check the URL, or add a token for a private repository."
                    .to_string(),
            );
        }
        return Err(github_error(status, &headers, &body));
    }

    let text = read_limited(response, MAX_MANIFEST_BYTES + 1)?;
    if text.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("Remote apps.json is larger than 8 MB.".to_string());
    }

    serde_json::from_str::<Manifest>(&text)
        .map_err(|error| format!("Remote apps.json is not valid JSON: {error}"))
}

fn normalize_manifest_url(input: &str, has_token: bool) -> Result<reqwest::Url, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Add a remote apps.json URL first.".to_string());
    }

    let mut url = reqwest::Url::parse(trimmed).map_err(|error| format!("Invalid URL: {error}"))?;
    if url.scheme() != "https" {
        return Err("The remote manifest URL must use HTTPS.".to_string());
    }

    // Accept normal GitHub "blob" links by converting them to a raw URL. When a
    // token is configured, use the contents API instead so private repos work.
    if url.host_str() == Some("github.com") {
        let segments: Vec<String> = url
            .path_segments()
            .map(|segments| segments.map(ToString::to_string).collect())
            .unwrap_or_default();
        if let Some(blob_index) = segments.iter().position(|segment| segment == "blob")
            && blob_index >= 2
            && segments.len() > blob_index + 2
        {
            let owner = &segments[0];
            let repo = &segments[1];
            let branch = &segments[blob_index + 1];
            let path = segments[blob_index + 2..].join("/");
            url = if has_token {
                let mut api = reqwest::Url::parse(&format!(
                    "https://api.github.com/repos/{owner}/{repo}/contents/{path}"
                ))
                .map_err(|error| format!("Invalid URL: {error}"))?;
                api.query_pairs_mut().append_pair("ref", branch);
                api
            } else {
                reqwest::Url::parse(&format!(
                    "https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{path}"
                ))
                .map_err(|error| format!("Invalid URL: {error}"))?
            };
        }
    }

    Ok(url)
}

fn is_github_host(host: &str) -> bool {
    matches!(
        host,
        "github.com" | "api.github.com" | "raw.githubusercontent.com" | "codeload.github.com"
    ) || host.ends_with(".githubusercontent.com")
}

fn read_limited(mut response: reqwest::blocking::Response, limit: u64) -> Result<String, String> {
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read GitHub response: {error}"))?;
    String::from_utf8(bytes).map_err(|error| format!("GitHub returned non-UTF-8 text: {error}"))
}

fn github_error(status: StatusCode, headers: &HeaderMap, body: &str) -> String {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().to_string());

    if status == StatusCode::NOT_FOUND {
        return NO_RELEASES_MESSAGE.to_string();
    }

    if status == StatusCode::UNAUTHORIZED {
        return "GitHub rejected the token. Check it in Settings.".to_string();
    }

    if status == StatusCode::FORBIDDEN
        && headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            == Some("0")
    {
        return format!(
            "GitHub rate limit reached. Add a personal access token in Settings.{}",
            reset_hint(headers)
        );
    }

    if message.is_empty() {
        format!("GitHub returned HTTP {status}.")
    } else {
        format!("GitHub returned HTTP {status}: {message}")
    }
}

fn reset_hint(headers: &HeaderMap) -> String {
    let Some(reset) = headers
        .get("x-ratelimit-reset")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return String::new();
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    if reset > now {
        let minutes = (reset - now).div_ceil(60);
        format!(" Try again in about {minutes} minute(s).")
    } else {
        String::new()
    }
}

pub fn parse_repo(repo: &str) -> Result<(String, String), String> {
    let trimmed = repo.trim();
    let mut parts = trimmed.split('/');
    let owner = parts.next().unwrap_or_default().trim();
    let name = parts.next().unwrap_or_default().trim();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return Err(format!(
            "Invalid GitHub repo {repo:?}; expected owner/repo."
        ));
    }
    if !valid_repo_component(owner) || !valid_repo_component(name) {
        return Err(format!(
            "Invalid GitHub repo {repo:?}; expected owner/repo."
        ));
    }
    Ok((owner.to_string(), name.to_string()))
}

fn valid_repo_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

pub fn clean_version(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(rest) = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        && rest.chars().next().is_some_and(|ch| ch.is_ascii_digit())
    {
        return rest.to_string();
    }
    trimmed.to_string()
}

pub fn is_newer(latest: &str, installed: &str) -> bool {
    let latest = clean_version(latest);
    if latest.is_empty() {
        return false;
    }

    let installed = installed.trim();
    if installed.is_empty()
        || installed.eq_ignore_ascii_case("local")
        || installed.eq_ignore_ascii_case("unknown")
    {
        return true;
    }

    match (parse_version(&latest), parse_version(installed)) {
        (Some(latest), Some(installed)) => compare_parsed(&latest, &installed) == Ordering::Greater,
        _ => !latest.eq_ignore_ascii_case(&clean_version(installed)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedVersion {
    core: Vec<u64>,
    prerelease: Option<Vec<String>>,
}

fn parse_version(value: &str) -> Option<ParsedVersion> {
    let cleaned = clean_version(value);
    let (core_text, prerelease) = match cleaned.split_once('-') {
        Some((core, pre)) => {
            let pre = pre.split('+').next().unwrap_or(pre);
            (core, Some(pre))
        }
        None => (cleaned.split('+').next().unwrap_or(&cleaned), None),
    };

    let mut core = Vec::new();
    for part in core_text.split('.') {
        if part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        core.push(part.parse::<u64>().ok()?);
    }
    if core.is_empty() {
        return None;
    }

    let prerelease = prerelease
        .filter(|pre| !pre.is_empty())
        .map(|pre| pre.split('.').map(ToString::to_string).collect());

    Some(ParsedVersion { core, prerelease })
}

fn compare_parsed(a: &ParsedVersion, b: &ParsedVersion) -> Ordering {
    let max_len = a.core.len().max(b.core.len());
    for index in 0..max_len {
        let a_part = a.core.get(index).copied().unwrap_or(0);
        let b_part = b.core.get(index).copied().unwrap_or(0);
        match a_part.cmp(&b_part) {
            Ordering::Equal => {}
            other => return other,
        }
    }

    match (&a.prerelease, &b.prerelease) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a_pre), Some(b_pre)) => compare_prerelease(a_pre, b_pre),
    }
}

fn compare_prerelease(a: &[String], b: &[String]) -> Ordering {
    let max_len = a.len().max(b.len());
    for index in 0..max_len {
        match (a.get(index), b.get(index)) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a_part), Some(b_part)) => {
                let a_num = a_part.parse::<u64>().ok();
                let b_num = b_part.parse::<u64>().ok();
                let ordering = match (a_num, b_num) {
                    (Some(a_num), Some(b_num)) => a_num.cmp(&b_num),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => a_part.cmp(b_part),
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
    Ordering::Equal
}

pub fn select_asset(assets: &[AssetInfo], platform: Platform) -> Option<AssetInfo> {
    assets
        .iter()
        .filter_map(|asset| asset_score(&asset.name, platform).map(|score| (score, asset)))
        .max_by(|(a_score, a_asset), (b_score, b_asset)| {
            a_score
                .cmp(b_score)
                .then_with(|| b_asset.name.len().cmp(&a_asset.name.len()))
        })
        .map(|(_, asset)| asset.clone())
}

fn asset_score(name: &str, platform: Platform) -> Option<u64> {
    let lower = name.to_ascii_lowercase();
    let excluded = [
        "sha256",
        "sha512",
        "checksum",
        "checksums",
        ".sig",
        ".asc",
        ".sbom",
        "source code",
        "source-code",
        ".zsync",
        ".blockmap",
    ];
    if excluded.iter().any(|token| lower.contains(token)) {
        return None;
    }

    let extension_excluded = [".txt", ".md", ".json", ".sha256", ".sig", ".asc"];
    if extension_excluded
        .iter()
        .any(|extension| lower.ends_with(extension))
    {
        return None;
    }

    let mac = lower.contains("macos") || lower.contains("darwin") || lower.contains("osx");
    if mac {
        return None;
    }

    let mut score: u64 = match platform {
        Platform::Linux => {
            if lower.contains("windows") || lower.contains("win64") || lower.contains("win32") {
                return None;
            }
            if lower.ends_with(".appimage") {
                100
            } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
                80
            } else {
                return None;
            }
        }
        Platform::Windows => {
            if lower.contains("linux") {
                return None;
            }
            if lower.ends_with(".exe") {
                100
            } else if lower.ends_with(".zip") {
                80
            } else {
                return None;
            }
        }
        Platform::Other => return None,
    };

    if platform == Platform::Linux && lower.contains("linux") {
        score += 12;
    }
    if platform == Platform::Windows
        && (lower.contains("windows") || lower.contains("win64") || lower.contains("win32"))
    {
        score += 12;
    }

    if lower.contains("setup") || lower.contains("installer") {
        score = score.saturating_sub(20);
    }

    score += architecture_score(&lower)?;
    Some(score)
}

fn architecture_score(lower: &str) -> Option<u64> {
    let arch = std::env::consts::ARCH;
    let matches_current = match arch {
        "x86_64" => ["x86_64", "x86-64", "amd64", "x64"]
            .iter()
            .any(|token| lower.contains(token)),
        "aarch64" => ["aarch64", "arm64"]
            .iter()
            .any(|token| lower.contains(token)),
        _ => true,
    };
    let other_arch = match arch {
        "x86_64" => [
            "aarch64", "arm64", "armv7", "armhf", "i686", "i386", "riscv", "ppc64",
        ]
        .iter()
        .any(|token| lower.contains(token)),
        "aarch64" => ["x86_64", "x86-64", "amd64", "i686", "i386"]
            .iter()
            .any(|token| lower.contains(token)),
        _ => false,
    };

    if other_arch {
        None
    } else if matches_current {
        Some(12)
    } else {
        Some(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> AssetInfo {
        AssetInfo {
            name: name.to_string(),
            browser_download_url: format!("https://github.com/o/r/releases/download/v1/{name}"),
            size: 10,
        }
    }

    fn repo(owner: &str, name: &str, fork: bool, archived: bool, private: bool) -> RepoSummary {
        RepoSummary {
            name: name.to_string(),
            full_name: format!("{owner}/{name}"),
            description: Some(format!("{name} description")),
            fork,
            archived,
            private,
            owner: owner.to_string(),
        }
    }

    fn release(version: &str) -> ReleaseInfo {
        ReleaseInfo {
            version: version.to_string(),
            tag: format!("v{version}"),
            html_url: format!("https://github.com/asdw2747/new-app/releases/tag/v{version}"),
            name: Some(format!("v{version}")),
            selected: Some(asset("NewApp-linux-x86_64.AppImage")),
        }
    }

    #[test]
    fn parses_owner_repo() {
        assert_eq!(
            parse_repo("asdw2747/JXLerate").unwrap(),
            ("asdw2747".to_string(), "JXLerate".to_string())
        );
        assert!(parse_repo("owner").is_err());
        assert!(parse_repo("owner/repo/extra").is_err());
        assert!(parse_repo("").is_err());
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("v1.0.1", "1.0.0"));
        assert!(!is_newer("v1.0.0", "1.0.0"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0.0", "2.0.0-beta.1"));
        assert!(!is_newer("2.0.0-beta.1", "2.0.0"));
        assert!(is_newer("1.0.0", "local"));
        assert!(is_newer("1.0.0", ""));
    }

    #[test]
    fn selects_linux_appimage_over_other_assets() {
        let assets = vec![
            asset("ComicCrate-1.0.1-windows-x64.zip"),
            asset("ComicCrate-1.0.1-x86_64.AppImage"),
            asset("ComicCrate-1.0.1-linux-x86_64.tar.gz"),
            asset("checksums-sha256.txt"),
            asset("source-code.zip"),
        ];
        let selected = select_asset(&assets, Platform::Linux).unwrap();
        assert_eq!(selected.name, "ComicCrate-1.0.1-x86_64.AppImage");
    }

    #[test]
    fn selects_windows_zip_when_no_exe_exists() {
        let assets = vec![
            asset("HauntedFiles-1.0.0-linux.tar.gz"),
            asset("haunted-files-windows-x64.zip"),
            asset("haunted-files.sha256"),
        ];
        let selected = select_asset(&assets, Platform::Windows).unwrap();
        assert_eq!(selected.name, "haunted-files-windows-x64.zip");
    }

    #[test]
    fn avoids_wrong_architecture_assets() {
        if std::env::consts::ARCH != "x86_64" {
            return;
        }
        let assets = vec![
            asset("Tool-1.0.0-aarch64.AppImage"),
            asset("Tool-1.0.0-x86_64.AppImage"),
        ];
        let selected = select_asset(&assets, Platform::Linux).unwrap();
        assert_eq!(selected.name, "Tool-1.0.0-x86_64.AppImage");
    }

    #[test]
    fn normalizes_github_blob_urls() {
        let url = normalize_manifest_url(
            "https://github.com/asdw2747/PrismaDock/blob/main/data/apps.json",
            false,
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://raw.githubusercontent.com/asdw2747/PrismaDock/main/data/apps.json"
        );

        let private = normalize_manifest_url(
            "https://github.com/asdw2747/PrismaDock/blob/main/data/apps.json",
            true,
        )
        .unwrap();
        assert_eq!(
            private.as_str(),
            "https://api.github.com/repos/asdw2747/PrismaDock/contents/data/apps.json?ref=main"
        );
    }

    #[test]
    fn rejects_http_remote_manifest_urls() {
        assert!(normalize_manifest_url("http://example.com/apps.json", false).is_err());
        assert!(normalize_manifest_url("https://example.com/apps.json", false).is_ok());
    }

    #[test]
    fn discovers_only_released_new_public_repos() {
        let repos = vec![
            repo("asdw2747", "new-app", false, false, false),
            repo("asdw2747", "no-release", false, false, false),
            repo("asdw2747", "Existing", false, false, false),
            repo("asdw2747", "prisma-dock", false, false, false),
            repo("asdw2747", "forked", true, false, false),
            repo("asdw2747", "archived-app", false, true, false),
            repo("asdw2747", "private-app", false, false, true),
            repo("someone-else", "outside", false, false, false),
        ];
        let existing = HashSet::from(["ASDW2747/existing".to_string()]);
        let mut queried = Vec::new();

        let outcome = discover_repos(repos, DISCOVERY_OWNER, &existing, |full_name| {
            queried.push(full_name.to_string());
            match full_name {
                "asdw2747/new-app" => Ok(Some(release("1.2.3"))),
                "asdw2747/no-release" => Ok(None),
                other => panic!("unexpected release lookup for {other}"),
            }
        });

        assert_eq!(outcome.candidates, 2);
        assert_eq!(outcome.errors.len(), 0);
        assert_eq!(outcome.discovered.len(), 1);
        let found = &outcome.discovered[0];
        assert_eq!(found.name, "new-app");
        assert_eq!(found.full_name, "asdw2747/new-app");
        assert_eq!(found.description.as_deref(), Some("new-app description"));
        assert_eq!(found.release.version, "1.2.3");
        assert_eq!(
            queried,
            vec![
                "asdw2747/new-app".to_string(),
                "asdw2747/no-release".to_string()
            ]
        );
    }
}
