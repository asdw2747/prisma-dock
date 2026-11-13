use std::collections::BTreeSet;

/// Lowercase, alphanumeric-only representation used for loose filename matching.
pub fn normalize_match_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Split a version-like token out of a filename.
///
/// Examples:
/// - `ComicCrate-1.0.1-x86_64.AppImage` -> `1.0.1`
/// - `JXLerate-v2.4.4-linux` -> `2.4.4`
/// - `haunted-files-x86_64.AppImage` -> `None`
pub fn version_from_name(name: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;

    for raw in name.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.')) {
        let token = raw.trim_matches('.');
        if token.is_empty() {
            continue;
        }
        let token = token
            .strip_prefix('v')
            .or_else(|| token.strip_prefix('V'))
            .unwrap_or(token);

        if !token.contains('.') {
            continue;
        }

        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2
            || parts
                .iter()
                .any(|part| part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()))
        {
            continue;
        }

        let candidate = parts.join(".");
        let score = parts.len();
        if best
            .as_ref()
            .is_none_or(|(best_score, _)| score > *best_score)
        {
            best = Some((score, candidate));
        }
    }

    best.map(|(_, version)| version)
}

/// Turn a filename stem into a readable app name.
pub fn humanize_file_stem(stem: &str) -> String {
    const NOISE_TOKENS: &[&str] = &[
        "x86",
        "x64",
        "64",
        "32",
        "amd64",
        "arm64",
        "aarch64",
        "armv7",
        "arm",
        "i386",
        "i686",
        "win",
        "win32",
        "win64",
        "windows",
        "linux",
        "macos",
        "darwin",
        "osx",
        "universal",
        "appimage",
        "exe",
        "dmg",
        "portable",
        "setup",
        "installer",
        "install",
        "release",
        "dist",
    ];

    let mut words = Vec::new();
    for raw in stem.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.')) {
        if raw.is_empty() || version_from_name(raw).is_some() {
            continue;
        }

        let lower = raw.to_ascii_lowercase();
        if NOISE_TOKENS.contains(&lower.as_str()) || lower == "v" {
            continue;
        }

        // Drop a bare version major such as "v2".
        if let Some(rest) = lower.strip_prefix('v')
            && !rest.is_empty()
            && rest.chars().all(|ch| ch.is_ascii_digit())
        {
            continue;
        }

        let word = if raw.chars().all(|ch| !ch.is_ascii_uppercase()) {
            let mut chars = raw.chars();
            match chars.next() {
                Some(first) => format!(
                    "{}{}",
                    first.to_ascii_uppercase(),
                    chars.as_str().to_ascii_lowercase()
                ),
                None => continue,
            }
        } else {
            raw.to_string()
        };
        words.push(word);
    }

    if words.is_empty() {
        let trimmed = stem.trim();
        if trimmed.is_empty() {
            return "New App".to_string();
        }
        let mut chars = trimmed.chars();
        return match chars.next() {
            Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
            None => "New App".to_string(),
        };
    }

    words.join(" ")
}

pub fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut last_dash = false;

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 64 {
            break;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        "app".to_string()
    } else {
        slug
    }
}

pub fn unique_id(base: &str, existing: &BTreeSet<String>) -> String {
    let base = slugify(base);
    if !existing.contains(&base) {
        return base;
    }

    for suffix in 2..1000 {
        let candidate = format!("{base}-{suffix}");
        if !existing.contains(&candidate) {
            return candidate;
        }
    }

    format!("{base}-{}", existing.len() + 1)
}

pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|word| !word.is_empty())
        .collect();
    if words.len() >= 2 {
        let a = words[0].chars().find(|ch| ch.is_ascii_alphanumeric());
        let b = words[1].chars().find(|ch| ch.is_ascii_alphanumeric());
        return match (a, b) {
            (Some(a), Some(b)) => format!("{}{}", a.to_ascii_uppercase(), b.to_ascii_uppercase()),
            _ => "?".to_string(),
        };
    }

    let caps: Vec<char> = name
        .chars()
        .filter(|ch| ch.is_ascii_uppercase())
        .take(2)
        .collect();
    if caps.len() >= 2 {
        return caps.into_iter().collect();
    }

    match name.chars().find(|ch| ch.is_ascii_alphanumeric()) {
        Some(ch) => ch.to_ascii_uppercase().to_string(),
        None => "?".to_string(),
    }
}

pub fn truncate_chars(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }

    let mut truncated: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_versions_from_real_asset_names() {
        assert_eq!(
            version_from_name("ComicCrate-1.0.1-x86_64.AppImage").as_deref(),
            Some("1.0.1")
        );
        assert_eq!(
            version_from_name("ReelRename-v1.0.0-linux.AppImage").as_deref(),
            Some("1.0.0")
        );
        assert_eq!(
            version_from_name("JXLerate-2.4.4-x86_64.AppImage").as_deref(),
            Some("2.4.4")
        );
        assert_eq!(version_from_name("haunted-files-x86_64.AppImage"), None);
        assert_eq!(version_from_name("app-x86_64.tar.gz"), None);
    }

    #[test]
    fn humanizes_local_names() {
        assert_eq!(humanize_file_stem("ComicCrate-1.0.1-x86_64"), "ComicCrate");
        assert_eq!(humanize_file_stem("ReelRename-1.0.0-x86_64"), "ReelRename");
        assert_eq!(humanize_file_stem("haunted-files-x86_64"), "Haunted Files");
        assert_eq!(humanize_file_stem("JXLerate-2.4.4-x86_64"), "JXLerate");
        assert_eq!(humanize_file_stem("my_cool_tool"), "My Cool Tool");
    }

    #[test]
    fn builds_safe_unique_ids() {
        assert_eq!(slugify("Haunted Files"), "haunted-files");
        assert_eq!(slugify("  Matrix Track!! "), "matrix-track");

        let mut existing = BTreeSet::new();
        existing.insert("comiccrate".to_string());
        assert_eq!(unique_id("ComicCrate", &existing), "comiccrate-2");
    }

    #[test]
    fn makes_initials() {
        assert_eq!(initials("ComicCrate"), "CC");
        assert_eq!(initials("Haunted Files"), "HF");
        assert_eq!(initials("jxlerate"), "J");
    }

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("abcdef", 4), "abc…");
        assert_eq!(truncate_chars("aéîöü", 3), "aé…");
    }
}
