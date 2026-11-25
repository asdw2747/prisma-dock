# Prisma Dock

**A lightweight cross-platform app hub and GitHub Releases updater.**

Prisma Dock is a native Rust dashboard for launching and updating a personal suite of Windows and Linux utilities. It keeps its configuration in a portable `data/` folder next to the executable and installs updated releases into `apps/<app_id>/`.

Built with `eframe`/`egui`. There is no web view, and HTTP uses `rustls`, so no OpenSSL is required.

## Features

- Dark, responsive card dashboard with installed/latest version badges.
- Per-app icons extracted automatically from AppImages, Windows executables, and embedded native icons, with a right-click override.
- One-click **Launch**, **Install**, and **Update** actions.
- GitHub Releases checks in a background worker, with an optional startup check.
- Automatic discovery of new public `asdw2747` repositories with published releases.
- Platform-aware release asset matching:
  - Linux: `.AppImage`, then `.tar.gz` / `.tgz`
  - Windows: `.exe`, then `.zip`
- Streaming download progress with size limits, pre-commit validation, and cancellable installs.
- Staged/versioned installs, safe archive extraction, and automatic `chmod 0o755` on Unix.
- Separate `windows` and `linux` launch targets in one shared manifest.
- Native binary launching and Python launching with virtualenv and windowless (`pythonw.exe`) support.
- Portable `apps.json` with no hard-coded database.
- Add apps from a local folder or files via native file dialogs.
- Optional GitHub personal access token for private repos and rate limits.
- Optional remote `apps.json` URL for syncing the app list across machines.
- Linux child-process environment sanitizing for AppImage launchers.

## Quick start

### Linux

Install the GTK3 and xkbcommon development packages used by the native file dialog and window backend. On Arch-based systems:

```sh
sudo pacman -S --needed base-devel gtk3 libxkbcommon
```

On Debian/Ubuntu:

```sh
sudo apt install build-essential pkg-config libgtk-3-dev libxkbcommon-dev
```

Build and run:

```sh
cargo run --release
```

### Windows

Use the MSVC Rust toolchain:

```powershell
cargo build --release
.\target\release\prisma-dock.exe
```

The release binary uses the Windows GUI subsystem, so it does not open a console window.

## Portable layout

Prisma Dock resolves its portable base directory in this order:

1. `PRISMA_DOCK_HOME`, if set and non-empty.
2. On Linux, the directory containing the running `.AppImage` (from the `APPIMAGE` environment variable).
3. The directory containing the executable.
4. The current working directory if the executable path cannot be determined.

If the selected directory is not writable — for example, an AppImage’s read-only SquashFS mount or `/usr/bin` — Prisma Dock falls back to the platform user-data directory:

- Linux: `$XDG_DATA_HOME/prisma-dock` or `~/.local/share/prisma-dock`
- Windows: `%LOCALAPPDATA%\PrismaDock`

The footer always shows the active data directory.

```text
PrismaDock/
  prisma-dock(.exe)
  data/
    apps.json          # app registry (created on first launch)
    settings.json      # token, remote URL, auto-check toggle
    icons/             # custom icons and the automatic icon cache
  apps/
    comiccrate/        # downloaded releases and local app folders
    reelrename/
    ...
```

For development, you can point it somewhere else:

```sh
PRISMA_DOCK_HOME=/tmp/prisma-dock-dev cargo run
```

When running through `cargo run`, the portable base is `target/debug/` or `target/release/`.

On first launch, Prisma Dock writes a starter manifest containing:

| App | Category | Initial GitHub repo | Initial version |
| --- | --- | --- | --- |
| ComicCrate | Comics | `asdw2747/comic-crate` | `1.0.1` |
| ReelRename | Media | `asdw2747/reel-rename` | `1.0.0` |
| Haunted Files | Files | `asdw2747/haunted-files-py` | `1.0.0` |
| JXLerate | Media | `asdw2747/JXLerate` | `2.4.4` |
| MatrixTrack | Audio | `asdw2747/MatrixTrack` | `1.1.0` |
| SurroundTag | Audio | `asdw2747/surround-tag` | `1.0.0` |

The first run also performs a best-effort local discovery in the portable `apps/` folder, the Desktop, Downloads, `~/Applications`, `~/.local/bin`, `~/bin`, and (on Windows) `%LOCALAPPDATA%\Programs` plus the Program Files directories. It searches depth-limited for matching `.AppImage`, `.exe`, and extensionless executables, then saves any paths it finds. Discovery only runs when `apps.json` is created; later changes are made through **Add App** or by editing the manifest.

> The starter repo names are editable. Private repositories require a token in Settings. A public repo with no published releases will show “No releases found”, not an app error.

## `apps.json` schema

`apps.json` is a JSON object with one required array, `apps`. Save the file and use **More → Reload manifest** to apply external edits.

```json
{
  "apps": [
    {
      "id": "comiccrate",
      "name": "ComicCrate",
      "description": "Convert and read comic archives with a fast native reader.",
      "category": "Comics",
      "repo": "asdw2747/comic-crate",
      "installed_version": "1.0.1",
      "icon_path": null,
      "windows": {
        "kind": "binary",
        "path": "C:\\Users\\you\\Desktop\\ComicCrate.exe"
      },
      "linux": {
        "kind": "binary",
        "path": "/home/you/Desktop/ComicCrate-1.0.1-x86_64.AppImage"
      }
    },
    {
      "id": "my-python-tool",
      "name": "My Python Tool",
      "description": "A local Python utility.",
      "category": "Tools",
      "repo": null,
      "installed_version": null,
      "icon_path": "data/icons/my-python-tool.png",
      "windows": {
        "kind": "python",
        "script_path": "C:\\Tools\\my-python-tool\\main.py",
        "venv_path": "C:\\Tools\\my-python-tool\\.venv",
        "windowless": true
      },
      "linux": {
        "kind": "python",
        "script_path": "tools/my-python-tool/main.py",
        "venv_path": "tools/my-python-tool/.venv"
      }
    }
  ]
}
```

### Fields

| Field | Required | Description |
| --- | --- | --- |
| `id` | yes | Unique filesystem-safe slug, unique case-insensitively for Windows compatibility. Letters, numbers, `-`, and `_` only; maximum 64 characters. Used as the install folder name. |
| `name` | yes | Display name. |
| `description` | no | Card subtitle. |
| `category` | no | Free-form label; defaults to `Other`. `Media`, `Files`, `Comics`, `Images`, `Audio`, and `Tools` get distinct chip colors. |
| `repo` | no | `"owner/repo"` used for GitHub Releases. `null` disables update checks for the app. |
| `installed_version` | no | Local version string or `null`. Tags such as `v1.2.3` are normalized when comparing. |
| `icon_path` | no | Absolute path or path relative to the portable base directory. PNG, JPEG, and ICO are supported. When omitted, Prisma Dock tries the app's embedded icon, an icon beside the launch target, or an XDG theme icon, then falls back to an initials tile. |
| `windows` | no | Windows launch target. |
| `linux` | no | Linux launch target. |

### Launch targets

Binaries:

```json
{ "kind": "binary", "path": "ComicCrate.AppImage" }
```

Python:

```json
{
  "kind": "python",
  "script_path": "main.py",
  "venv_path": ".venv",
  "windowless": false
}
```

Path rules:

- Absolute paths are used as-is. `~` and `~/...` are expanded.
- Relative binary paths, script paths, and venv paths resolve under `apps/<id>/`.
- Auto-discovered and scanned files under your home directory are stored as `~/...` when possible, so a manifest copied to another machine has a better chance of resolving.
- `icon_path` relative paths resolve under the portable base directory, so `data/icons/tool.png` works.
- When `icon_path` is omitted, Prisma Dock reads the AppImage's `.DirIcon`, a Windows `.exe`'s embedded icon resource, an `icon.png`/`<name>.png` beside the launch target, a matching XDG icon, or a square PNG embedded in a native ELF binary. Extracted icons are cached as PNGs under `data/icons/auto/`; the cache follows the installed target and is rebuilt after updates.
- To override an automatic icon, right-click the card icon and choose **Choose icon…**. The selected image is copied to `data/icons/<app-id>.<ext>` and written to `icon_path`. Choose **Use automatic icon** to remove the override.
- With a venv, the interpreter is `Scripts/python.exe` on Windows or `bin/python` on Unix. Set `"windowless": true` to use `Scripts/pythonw.exe` instead on Windows. A configured but broken venv is an error rather than a silent fallback to global Python.
- Without a venv, launching uses `python` (or `pythonw` when windowless) on Windows and `python3` on Linux.
- `.pyw` files discovered through **Add App** are automatically marked windowless.
- Python scripts run with their own directory as the working directory.
- On Linux, **every** child launch removes `APPIMAGE`, `APPDIR`, and `LD_LIBRARY_PATH` from the inherited environment. This lets an AppImage-hosted Prisma Dock spawn other AppImages cleanly.

`settings.json` is managed by the Settings dialog:

```json
{
  "github_token": "",
  "remote_manifest_url": "",
  "auto_check_updates": true
}
```

On Unix, `settings.json` is written with `0600` permissions from the moment the temporary file is created. The token is only sent to GitHub hosts. It is not written into `apps.json`, and it is never included in logs or error messages.

## Updates and installation

**Check for Updates** queries the latest release for every configured app and also scans the public repository list:

```text
GET https://api.github.com/repos/{owner}/{repo}/releases/latest
GET https://api.github.com/users/asdw2747/repos?type=owner&sort=updated
```

Update checks run on a background thread, use the token if configured, and show progress in the top bar. Startup checks are controlled by the Settings toggle and are enabled by default. Checks work even when the manifest has no repo-backed apps yet.

### Automatic repository discovery

Every check also looks for new public repositories under `asdw2747`:

- Forks, archived repos, private repos, `prisma-dock` itself, and repos already present in `apps.json` are ignored. Repo matching is case-insensitive.
- Each new repo is checked for a published latest release. Repos with no release are skipped.
- A repo with a release is appended to `apps.json` with the repo name, GitHub description, `category: "Other"`, `installed_version: null`, and no launch targets.
- Its release metadata is cached immediately, so compatible new repos show up as **Install** cards without another check.
- Existing app entries, custom paths, and installed versions are never rewritten. The new app is simply appended and the manifest is saved.
- Discovery runs on both manual checks and startup auto-checks. It is non-fatal: a discovery failure is reported in the status bar while normal app checks continue.

The repository list scan is paginated and capped for safety. A token is recommended if you publish many repos or check frequently, because the unauthenticated API limit is shared with release checks.

For each release, the platform asset is selected automatically:

| Platform | Preferred | Fallback | Rejected |
| --- | --- | --- | --- |
| Linux | `.AppImage` | `.tar.gz`, `.tgz` | Windows/macOS assets, checksums, source archives |
| Windows | `.exe` | `.zip` | Linux/macOS assets, checksums, source archives |

Names containing the current OS and architecture are preferred. Wrong-architecture assets (for example an `aarch64` build on x86_64) are filtered out, so a release without a matching build reports “No asset” instead of installing the wrong binary.

Recommended release asset names:

```text
ComicCrate-1.0.1-x86_64.AppImage
ReelRename-1.0.0-windows-x64.zip
haunted-files-1.0.0-linux-x86_64.tar.gz
JXLerate-2.4.4-x86_64.AppImage
```

The install/download flow:

1. The asset downloads into a hidden `.staging-*` directory under `apps/<id>/`.
2. Downloads are capped at 2 GiB and must match the declared HTTP/API size.
3. The card shows a progress bar and downloaded/total size, plus a **Cancel** action.
4. Archives are extracted into the staging directory with path-traversal protection, a 50,000-entry limit, and a 4 GiB total / 2 GiB per-file cap. Symlinks are skipped.
5. The best matching `.exe`, `.AppImage`, or executable is selected only from the freshly extracted staging contents.
6. On Unix, the selected launcher is set to mode `0755`.
7. The staging directory is atomically renamed to a unique versioned folder under `apps/<id>/`, and `installed_version` plus the new target are saved to `apps.json`.

The previous installation is never overwritten. If an update fails or is cancelled, the live install stays intact and the staging directory is removed. Install folders accumulate across versions; use **Open folder** and go up one level to clean old `apps/<id>/v*` directories manually. Interrupted staging directories older than one hour are removed on the next launch.

There is no code-signature or checksum verification. Assets are downloaded over HTTPS from GitHub. Treat release assets with the same trust you would give the upstream project.

### Rate limits and private repos

The unauthenticated GitHub API is limited (typically 60 requests/hour). A fine-grained token with read access to the relevant repositories is enough; a classic token needs `repo` scope for private repositories.

If GitHub returns 404, the UI reports “No releases found” because GitHub uses the same response for missing repos and missing releases. Add a token for private repositories.

## Remote app-list sync

In **Settings**, enter a raw HTTPS `apps.json` URL and press **Sync now**. GitHub `blob` URLs are converted to raw URLs automatically. When a token is configured, blob URLs are fetched through the GitHub contents API so private repositories work. You can also paste an API contents URL such as `https://api.github.com/repos/owner/repo/contents/data/apps.json?ref=main`.

Sync behavior:

- Remote app definitions replace local names, descriptions, categories, repos, and platform sections.
- Local `installed_version` values are preserved by matching `id`.
- If a platform section is missing remotely, the local platform section is kept.
- Local-only apps are kept, so a sync never silently deletes a local entry.
- Invalid or duplicate remote IDs are skipped.
- The previous manifest is copied to `data/apps.json.bak`.
- A failed download or invalid JSON changes nothing.

The remote file is only app definitions. Settings and tokens stay local to each machine.

## Adding local apps

**Add App** opens a native dialog with two options:

- **Scan folder…** recursively searches (depth-limited) for `.exe`, `.AppImage`, `.py`, and `.pyw` tools and lets you edit each name, category, and Python venv before adding.
- **Add files…** registers one or more specific launchable files.

Detected `.exe` files go into the `windows` section and `.AppImage` files into `linux`, so a manifest edited on one machine remains useful on the other. Python entries are added for the current platform. Existing paths are deduplicated.

The starter manifest includes MatrixTrack and SurroundTag as the configured audio apps. Use **Add App** for any additional utilities.

## Reloading the manifest

Because `apps.json` is a normal editable file, the app does not watch it. After external edits, use **More → Reload manifest**. A malformed manifest is never overwritten; the app keeps running and shows an error banner with actions to reload or open the data folder.

## Development

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

Unit tests cover manifest parsing, path resolution, version comparison, release asset selection, filename inference, ZIP traversal rejection, extraction limits, staging cleanup, atomic config writes, and atomic settings permissions.

Windows cross-check from Linux (optional):

```sh
rustup target add x86_64-pc-windows-msvc
cargo xwin check --target x86_64-pc-windows-msvc
```

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| “No releases found” | Repo name, whether the repo has a published release, or whether a token is needed. |
| “GitHub rate limit reached” | Add a token in Settings or wait for the reset window. |
| Update has no compatible asset | Publish a Linux `.AppImage`/`.tar.gz` or Windows `.exe`/`.zip` asset with a recognizable name. |
| Launch fails with permission denied on Linux | Make the target executable (`chmod +x`). Downloads are made executable automatically. |
| Python venv error | Point `venv_path` at the folder containing `bin/python` or `Scripts/python.exe`. |
| Window will not start on Linux | Install GTK3/X11/Wayland runtime libraries; `rfd` uses the native GTK dialog backend. |
| App data cannot be written | Prisma Dock falls back to the OS user-data directory when the portable/AppImage folder is read-only. Check the data path shown in the footer. |

## License

MIT © 2026 Prisma Chill. See [LICENSE](LICENSE).
