use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.png");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rustc-check-cfg=cfg(has_embedded_icon)");

    if Path::new("assets/icon.png").is_file() {
        println!("cargo:rustc-cfg=has_embedded_icon");
    }

    // Use the target OS, not the build-script host OS, so this runs when
    // cross-compiling the Windows executable from Linux.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_resources();
    }
}

fn embed_windows_resources() {
    let icon = Path::new("assets/icon.ico");
    if !icon.is_file() {
        println!(
            "cargo:warning=assets/icon.ico not found; building without a Windows executable icon"
        );
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(icon.to_string_lossy().as_ref());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "1.0.0".to_string());
    resource.set("ProductName", "Prisma Dock");
    resource.set(
        "FileDescription",
        "A lightweight cross-platform app hub and GitHub Releases updater.",
    );
    resource.set("ProductVersion", &version);
    resource.set("FileVersion", &version);
    resource.set("LegalCopyright", "Copyright (c) 2026 Prisma Chill");
    resource.set("OriginalFilename", "prisma-dock.exe");

    if let Err(error) = resource.compile() {
        println!("cargo:warning=failed to embed Windows icon/resources: {error}");
    }
}
