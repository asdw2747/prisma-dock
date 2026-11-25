use std::fs;
use std::io::Cursor;
#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::path::Component;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use image::{DynamicImage, GenericImageView, ImageFormat};

use crate::model::{AppEntry, Platform};
use crate::paths::AppPaths;

const MAX_ICON_BYTES: u64 = 10 * 1024 * 1024;
const MAX_ICON_DIMENSION: u32 = 2048;
const AUTO_ICON_DIR: &str = "icons/auto";
const CUSTOM_ICON_DIR: &str = "icons";

/// Resolve an icon for an app without changing its manifest entry.
///
/// Explicit `icon_path` values are handled by the caller. This function only
/// looks at the current launch target and returns either an existing image
/// beside it, an XDG theme icon, or a PNG extracted from an AppImage/.exe and
/// cached under `data/icons/auto`.
pub fn auto_icon_path(paths: &AppPaths, app: &AppEntry, platform: Platform) -> Option<PathBuf> {
    let target = app.target_for(platform)?;
    let target_path = paths.resolve_launch_path(&app.id, target.path());
    if !target_path.is_file() {
        return None;
    }

    if is_appimage(&target_path) {
        #[cfg(target_os = "linux")]
        if let Some(path) = extracted_icon(paths, app, &target_path, || appimage_icon(&target_path))
        {
            return Some(path);
        }
    }

    #[cfg(target_os = "windows")]
    if is_windows_exe(&target_path)
        && let Some(path) = extracted_icon(paths, app, &target_path, || exe_icon(&target_path))
    {
        return Some(path);
    }

    if let Some(path) = find_sidecar_icon(&target_path) {
        return Some(path);
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(path) = find_xdg_icon(app, &target_path) {
            return Some(path);
        }
        if !is_appimage(&target_path)
            && let Some(path) =
                extracted_icon(paths, app, &target_path, || embedded_png_icon(&target_path))
        {
            return Some(path);
        }
        None
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
        None
    }
}

/// Copy a chosen image into the portable data directory and return the
/// manifest-relative path.
pub fn install_custom_icon(
    paths: &AppPaths,
    app_id: &str,
    source: &Path,
) -> Result<String, String> {
    let metadata = fs::metadata(source)
        .map_err(|error| format!("Could not read {}: {error}", source.display()))?;
    if !metadata.is_file() {
        return Err("The selected icon is not a file.".to_string());
    }
    if metadata.len() > MAX_ICON_BYTES {
        return Err("The selected icon is larger than 10 MB.".to_string());
    }

    let (width, height) = image::image_dimensions(source)
        .map_err(|error| format!("Could not read the selected image: {error}"))?;
    if width == 0 || height == 0 || width > MAX_ICON_DIMENSION || height > MAX_ICON_DIMENSION {
        return Err(format!(
            "Choose an image between 1x1 and {MAX_ICON_DIMENSION}x{MAX_ICON_DIMENSION}."
        ));
    }

    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .filter(|extension| matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "ico"))
        .ok_or_else(|| "Choose a PNG, JPEG, or ICO image.".to_string())?;

    let directory = paths.data_dir.join(CUSTOM_ICON_DIR);
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
    let file_name = format!("{}.{}", safe_stem(app_id), extension);
    let destination = directory.join(&file_name);

    if same_file(source, &destination) {
        return relative_to_base(paths, &destination);
    }

    let temporary = directory.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        time_nonce()
    ));
    let result = fs::copy(source, &temporary)
        .and_then(|_| replace_file(&temporary, &destination))
        .map_err(|error| format!("Could not store the icon: {error}"));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;

    relative_to_base(paths, &destination)
}

fn relative_to_base(paths: &AppPaths, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(&paths.base)
        .map_err(|_| format!("The icon must live under {}.", paths.base.display()))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn extracted_icon(
    paths: &AppPaths,
    app: &AppEntry,
    target: &Path,
    extract: impl FnOnce() -> Option<DynamicImage>,
) -> Option<PathBuf> {
    let directory = paths.data_dir.join(AUTO_ICON_DIR);
    fs::create_dir_all(&directory).ok()?;

    let file_name = cache_file_name(&app.id, target);
    let destination = directory.join(&file_name);
    if destination.is_file() {
        return Some(destination);
    }

    let image = extract()?;
    save_png(&destination, &image).ok()?;
    prune_cached_icons(&directory, &app.id, &file_name);
    Some(destination)
}

fn cache_file_name(app_id: &str, target: &Path) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in target.to_string_lossy().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    if let Ok(metadata) = fs::metadata(target) {
        hash ^= metadata.len();
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        if let Ok(modified) = metadata.modified()
            && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
        {
            hash ^= duration.as_nanos() as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{}-{hash:016x}.png", safe_stem(app_id))
}

fn prune_cached_icons(directory: &Path, app_id: &str, keep: &str) {
    let prefix = format!("{}-", safe_stem(app_id));
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && name != keep {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn save_png(destination: &Path, image: &DynamicImage) -> Result<(), String> {
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|error| format!("Could not encode the icon: {error}"))?;
    let bytes = bytes.into_inner();

    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("icon.png");
    let temporary = destination.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        time_nonce()
    ));
    let result = fs::write(&temporary, bytes)
        .and_then(|_| replace_file(&temporary, destination))
        .map_err(|error| format!("Could not cache the icon: {error}"));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    match fs::rename(temporary, destination) {
        Ok(()) => Ok(()),
        Err(_error) if destination.exists() => {
            fs::remove_file(destination)?;
            fs::rename(temporary, destination)
        }
        Err(error) => Err(error),
    }
}

fn decode_icon(bytes: &[u8]) -> Option<DynamicImage> {
    if bytes.len() as u64 > MAX_ICON_BYTES {
        return None;
    }
    let image = image::load_from_memory(bytes).ok()?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 || width > MAX_ICON_DIMENSION || height > MAX_ICON_DIMENSION {
        return None;
    }
    Some(image)
}

fn read_icon(path: &Path) -> Option<DynamicImage> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_ICON_BYTES {
        return None;
    }
    decode_icon(&fs::read(path).ok()?)
}

#[cfg(target_os = "linux")]
fn appimage_icon(path: &Path) -> Option<DynamicImage> {
    let image = appimage::AppImage::open(path).ok()?;

    let dir_icon = image.unpack().matching(".DirIcon").to_temp().ok()?;
    let dir_icon_path = dir_icon.join(".DirIcon");
    if dir_icon_path.is_file()
        && let Some(icon) = read_icon(&dir_icon_path)
    {
        return Some(icon);
    }
    let linked_icon = fs::read_link(&dir_icon_path).ok();

    // `.DirIcon` is usually a symlink to a root-level PNG, which is itself a
    // symlink into usr/share/icons. Extract the PNGs so either link resolves.
    let pngs = image.unpack().matching("*.png").to_temp().ok()?;
    if let Some(link) = linked_icon
        && let Some(relative) = archive_relative_path(&link)
        && let Some(icon) = read_icon(&pngs.join(relative))
    {
        return Some(icon);
    }

    best_png(&pngs)
}

#[cfg(target_os = "linux")]
fn archive_relative_path(path: &Path) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !relative.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!relative.as_os_str().is_empty()).then_some(relative)
}

#[cfg(target_os = "linux")]
fn best_png(root: &Path) -> Option<DynamicImage> {
    let mut candidates = Vec::new();
    collect_pngs(root, 0, &mut candidates);

    let mut best: Option<DynamicImage> = None;
    let mut best_area = 0u64;
    for path in candidates {
        let Some(icon) = read_icon(&path) else {
            continue;
        };
        let (width, height) = icon.dimensions();
        let area = u64::from(width) * u64::from(height);
        if area > best_area {
            best = Some(icon);
            best_area = area;
        }
    }
    best
}

#[cfg(target_os = "linux")]
fn collect_pngs(directory: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_pngs(&path, depth + 1, out);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            && fs::metadata(&path)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

#[cfg(target_os = "linux")]
fn embedded_png_icon(path: &Path) -> Option<DynamicImage> {
    const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
    const MAX_PNG_BYTES: usize = 2 * 1024 * 1024;

    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SCAN_BYTES {
        return None;
    }
    let mut header = [0u8; 4];
    fs::File::open(path).ok()?.read_exact(&mut header).ok()?;
    if &header != b"\x7fELF" {
        return None;
    }
    let bytes = fs::read(path).ok()?;

    let mut best = None;
    let mut best_area = 0u64;
    let mut search_from = 0;
    let mut seen = 0usize;
    while seen < 32 {
        let Some(relative) = find_bytes(&bytes[search_from..], PNG_SIGNATURE) else {
            break;
        };
        let start = search_from + relative;
        let Some(end) = png_end(&bytes, start) else {
            search_from = start + PNG_SIGNATURE.len();
            continue;
        };
        let png = &bytes[start..end];
        if png.len() <= MAX_PNG_BYTES
            && let Some(icon) = decode_icon(png)
        {
            let (width, height) = icon.dimensions();
            let area = u64::from(width) * u64::from(height);
            if width == height && width >= 16 && area > best_area {
                best = Some(icon);
                best_area = area;
            }
        }
        seen += 1;
        search_from = end;
    }
    best
}

#[cfg(target_os = "linux")]
fn png_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start..start + 8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut position = start + 8;
    while position + 12 <= bytes.len() {
        let length =
            u32::from_be_bytes(bytes.get(position..position + 4)?.try_into().ok()?) as usize;
        let kind = bytes.get(position + 4..position + 8)?;
        let next = position.checked_add(12)?.checked_add(length)?;
        if next > bytes.len() {
            return None;
        }
        if kind == b"IEND" {
            return Some(next);
        }
        position = next;
    }
    None
}

#[cfg(target_os = "linux")]
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(target_os = "windows")]
fn exe_icon(path: &Path) -> Option<DynamicImage> {
    use icoextract_rs::thumbnail::{ThumbnailOptions, generate_thumbnail};

    let temporary = std::env::temp_dir().join(format!(
        ".prisma-dock-icon-{}-{}.png",
        std::process::id(),
        time_nonce()
    ));
    let result = generate_thumbnail(path, &temporary, ThumbnailOptions::new(256, false))
        .ok()
        .and_then(|()| read_icon(&temporary));
    let _ = fs::remove_file(&temporary);
    result
}

fn find_sidecar_icon(target: &Path) -> Option<PathBuf> {
    let parent = target.parent()?;
    let stem = target.file_stem()?.to_string_lossy().into_owned();

    let mut directories = vec![parent.to_path_buf()];
    for subdirectory in ["assets", "resources", "docs"] {
        directories.push(parent.join(subdirectory));
    }
    if let Some(grandparent) = parent.parent() {
        for subdirectory in ["assets", "resources", "docs"] {
            directories.push(grandparent.join(subdirectory));
        }
    }

    let names = [
        format!("{stem}.png"),
        format!("{stem}.jpg"),
        format!("{stem}.jpeg"),
        format!("{stem}.ico"),
        "icon.png".to_string(),
        "icon.jpg".to_string(),
        "icon.jpeg".to_string(),
        "icon.ico".to_string(),
    ];

    for directory in directories {
        for name in &names {
            let path = directory.join(name);
            if is_valid_icon(&path) {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn find_xdg_icon(app: &AppEntry, target: &Path) -> Option<PathBuf> {
    let names = candidate_names(app, target);
    for root in xdg_icon_roots() {
        if let Some(path) = find_in_icon_root(&root, &names) {
            return Some(path);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn xdg_icon_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(data) = dirs::data_dir() {
        roots.push(data.join("icons"));
    }
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".icons"));
    }
    roots.push(PathBuf::from("/usr/local/share/icons"));
    roots.push(PathBuf::from("/usr/share/icons"));
    roots.push(PathBuf::from("/usr/local/share/pixmaps"));
    roots.push(PathBuf::from("/usr/share/pixmaps"));
    roots
}

#[cfg(target_os = "linux")]
fn find_in_icon_root(root: &Path, names: &[String]) -> Option<PathBuf> {
    let hicolor = root.join("hicolor");
    let mut sizes = Vec::new();
    if let Ok(entries) = fs::read_dir(&hicolor) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let size = name
                .split_once('x')
                .and_then(|(width, _)| width.parse::<u32>().ok())
                .unwrap_or(0);
            if entry.path().is_dir() {
                sizes.push((size, entry.path()));
            }
        }
    }
    sizes.sort_by_key(|(size, _)| *size);
    for (_, directory) in sizes.into_iter().rev() {
        let apps = directory.join("apps");
        for name in names {
            for extension in ["png", "jpg", "jpeg", "ico"] {
                let path = apps.join(format!("{name}.{extension}"));
                if is_valid_icon(&path) {
                    return Some(path);
                }
            }
        }
    }

    for pixmap in [root.join("pixmaps"), root.to_path_buf()] {
        for name in names {
            for extension in ["png", "jpg", "jpeg", "ico"] {
                let path = pixmap.join(format!("{name}.{extension}"));
                if is_valid_icon(&path) {
                    return Some(path);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn candidate_names(app: &AppEntry, target: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let mut push = |value: String| {
        let value = value.trim().to_ascii_lowercase();
        if !value.is_empty() && !names.contains(&value) {
            names.push(value);
        }
    };

    push(app.id.clone());
    push(app.name.replace([' ', '_'], "-"));
    if let Some(stem) = target.file_stem().and_then(|stem| stem.to_str()) {
        push(stem.to_string());
    }
    if let Some(repo) = app.repo.as_deref()
        && let Some(name) = repo.rsplit('/').next()
    {
        push(name.replace('_', "-"));
    }
    names
}

fn is_valid_icon(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > MAX_ICON_BYTES {
        return false;
    }
    image::image_dimensions(path)
        .map(|(width, height)| {
            width > 0 && height > 0 && width <= MAX_ICON_DIMENSION && height <= MAX_ICON_DIMENSION
        })
        .unwrap_or(false)
}

fn is_appimage(path: &Path) -> bool {
    extension_is(path, "appimage")
}

#[cfg(target_os = "windows")]
fn is_windows_exe(path: &Path) -> bool {
    extension_is(path, "exe")
}

fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn safe_stem(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            result.push(character);
        } else {
            result.push('_');
        }
    }
    if result.is_empty() {
        "app".to_string()
    } else {
        result
    }
}

fn time_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use crate::model::LaunchTarget;

    #[cfg(target_os = "linux")]
    fn app_with_target(target: &Path) -> AppEntry {
        AppEntry {
            id: "tool".to_string(),
            name: "Tool".to_string(),
            description: String::new(),
            category: "Other".to_string(),
            repo: None,
            installed_version: None,
            icon_path: None,
            windows: None,
            linux: Some(LaunchTarget::Binary {
                path: target.to_string_lossy().into_owned(),
            }),
        }
    }

    #[test]
    fn cache_file_changes_when_target_changes() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("Tool.AppImage");
        fs::write(&target, b"one").unwrap();
        let first = cache_file_name("tool", &target);
        fs::write(&target, b"two").unwrap();
        let second = cache_file_name("tool", &target);
        assert_ne!(first, second);
        assert!(first.starts_with("tool-"));
    }

    #[test]
    fn sidecar_icon_is_found_next_to_the_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("Tool");
        fs::write(&target, b"binary").unwrap();
        let icon = temp.path().join("icon.png");
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
        image.save(&icon).unwrap();
        assert_eq!(find_sidecar_icon(&target), Some(icon));
    }

    #[test]
    fn sidecar_icon_uses_assets_directory() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("tool.py");
        fs::write(&target, b"print('tool')").unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir(&assets).unwrap();
        let icon = assets.join("icon.png");
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
        image.save(&icon).unwrap();
        assert_eq!(find_sidecar_icon(&target), Some(icon));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn xdg_icon_prefers_the_largest_variant() {
        let temp = tempfile::tempdir().unwrap();
        let small = temp.path().join("hicolor/16x16/apps");
        let large = temp.path().join("hicolor/256x256/apps");
        fs::create_dir_all(&small).unwrap();
        fs::create_dir_all(&large).unwrap();
        image::RgbaImage::from_pixel(16, 16, image::Rgba([1, 2, 3, 255]))
            .save(small.join("tool.png"))
            .unwrap();
        image::RgbaImage::from_pixel(256, 256, image::Rgba([1, 2, 3, 255]))
            .save(large.join("tool.png"))
            .unwrap();

        let target = temp.path().join("tool");
        fs::write(&target, b"binary").unwrap();
        let app = app_with_target(&target);
        assert_eq!(
            find_in_icon_root(temp.path(), &candidate_names(&app, &target)),
            Some(large.join("tool.png"))
        );
    }

    #[test]
    fn custom_icon_is_copied_into_the_data_directory() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_base(temp.path());
        let source = temp.path().join("chosen.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .save(&source)
            .unwrap();

        let relative = install_custom_icon(&paths, "my-tool", &source).unwrap();
        assert_eq!(relative, "data/icons/my-tool.png");
        assert!(paths.base.join(&relative).is_file());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn appimage_dir_icon_is_extracted() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Test.AppDir");
        fs::create_dir_all(source.join("usr/bin")).unwrap();
        fs::write(source.join("usr/bin/hello"), b"hi").unwrap();
        fs::write(source.join("AppRun"), b"#!/bin/sh\n").unwrap();
        let icon = source.join("test.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([9, 8, 7, 255]))
            .save(&icon)
            .unwrap();
        symlink("test.png", source.join(".DirIcon")).unwrap();

        let app_dir = appimage::AppDir::open(&source).unwrap();
        let image = appimage::Packer::new(&app_dir, &appimage::Runtime::inert())
            .write_to(temp.path().join("Test.AppImage"))
            .unwrap();
        let extracted = appimage_icon(&image.path).expect("embedded icon");
        assert_eq!(extracted.dimensions(), (4, 4));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn embedded_png_icon_is_found_in_a_native_binary() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("tool");
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(32, 32, image::Rgba([4, 5, 6, 255]))
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let mut bytes = b"\x7fELF fake binary".to_vec();
        bytes.extend_from_slice(&png.into_inner());
        fs::write(&target, bytes).unwrap();
        let icon = embedded_png_icon(&target).expect("embedded icon");
        assert_eq!(icon.dimensions(), (32, 32));
    }

    #[test]
    fn custom_icon_rejects_non_images() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_base(temp.path());
        let source = temp.path().join("notes.txt");
        fs::write(&source, b"not an image").unwrap();
        assert!(install_custom_icon(&paths, "tool", &source).is_err());
    }
}
