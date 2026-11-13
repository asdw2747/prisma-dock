use std::path::{Path, PathBuf};
use std::process::Command;

use crate::model::{AppEntry, LaunchTarget, Platform, valid_id};
use crate::paths::AppPaths;

pub fn current_target_path(paths: &AppPaths, app: &AppEntry) -> Option<PathBuf> {
    if !valid_id(&app.id) {
        return None;
    }
    let platform = Platform::current();
    let target = app.target_for(platform)?;
    let path = paths.resolve_launch_path(&app.id, target.path());
    Some(path)
}

pub fn target_exists(paths: &AppPaths, app: &AppEntry) -> bool {
    current_target_path(paths, app).is_some_and(|path| path.is_file())
}

pub fn launch(paths: &AppPaths, app: &AppEntry) -> Result<(), String> {
    if !valid_id(&app.id) {
        return Err(format!(
            "{} has an invalid id; use letters, numbers, '-' or '_'.",
            app.name
        ));
    }

    let platform = Platform::current();
    let target = app.target_for(platform).ok_or_else(|| {
        format!(
            "{} has no launch target for {}. Use Add App or edit data/apps.json.",
            app.name,
            platform.label()
        )
    })?;

    match target {
        LaunchTarget::Binary { path } => {
            let binary = paths.resolve_launch_path(&app.id, path);
            if !binary.is_file() {
                return Err(format!(
                    "{} could not be launched because {} does not exist.",
                    app.name,
                    binary.display()
                ));
            }

            let mut command = Command::new(&binary);
            if let Some(parent) = binary.parent() {
                command.current_dir(parent);
            }
            sanitize_child_environment(&mut command);
            command
                .spawn()
                .map_err(|error| spawn_error(&binary, error))?;
            Ok(())
        }
        LaunchTarget::Python {
            script_path,
            venv_path,
            windowless,
        } => {
            let script = paths.resolve_launch_path(&app.id, script_path);
            if !script.is_file() {
                return Err(format!(
                    "{} could not be launched because the Python script {} does not exist.",
                    app.name,
                    script.display()
                ));
            }

            let python = python_interpreter(paths, &app.id, venv_path.as_deref(), *windowless)?;
            let mut command = Command::new(&python);
            command.arg(&script);
            if let Some(parent) = script.parent() {
                command.current_dir(parent);
            }
            sanitize_child_environment(&mut command);
            command
                .spawn()
                .map_err(|error| spawn_error(&python, error))?;
            Ok(())
        }
    }
}

fn python_interpreter(
    paths: &AppPaths,
    app_id: &str,
    venv_path: Option<&str>,
    windowless: bool,
) -> Result<PathBuf, String> {
    if let Some(venv_path) = venv_path.filter(|value| !value.trim().is_empty()) {
        let venv = paths.resolve_launch_path(app_id, venv_path);
        let python = python_in_venv(&venv, windowless);
        if python.is_file() {
            return Ok(python);
        }
        return Err(format!(
            "The configured Python venv does not contain the expected interpreter: {}",
            python.display()
        ));
    }

    #[cfg(target_os = "windows")]
    let fallback = if windowless {
        PathBuf::from("pythonw")
    } else {
        PathBuf::from("python")
    };
    #[cfg(not(target_os = "windows"))]
    let fallback = {
        let _ = windowless;
        PathBuf::from("python3")
    };

    Ok(fallback)
}

fn python_in_venv(venv: &Path, windowless: bool) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let executable = if windowless {
            "pythonw.exe"
        } else {
            "python.exe"
        };
        venv.join("Scripts").join(executable)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = windowless;
        venv.join("bin").join("python")
    }
}

fn spawn_error(program: &Path, error: std::io::Error) -> String {
    #[cfg(unix)]
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        return format!(
            "Could not start {}: the file is not executable. Run chmod +x on it and try again.",
            program.display()
        );
    }

    format!("Could not start {}: {error}", program.display())
}

#[cfg(target_os = "linux")]
fn sanitize_child_environment(command: &mut Command) {
    command
        .env_remove("APPIMAGE")
        .env_remove("APPDIR")
        .env_remove("LD_LIBRARY_PATH");
}

#[cfg(not(target_os = "linux"))]
fn sanitize_child_environment(_command: &mut Command) {}

pub fn open_folder(paths: &AppPaths, app: &AppEntry) -> Result<(), String> {
    if !valid_id(&app.id) {
        return Err(format!(
            "{} has an invalid id; use letters, numbers, '-' or '_'.",
            app.name
        ));
    }

    let folder = current_target_path(paths, app)
        .and_then(|path| {
            if path.is_file() {
                path.parent().map(Path::to_path_buf)
            } else {
                None
            }
        })
        .unwrap_or_else(|| paths.install_dir(&app.id));

    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("Could not create {}: {error}", folder.display()))?;
    open_path(&folder)
}

pub fn open_path(path: &Path) -> Result<(), String> {
    open::that_detached(path).map_err(|error| format!("Could not open {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppEntry, LaunchTarget};

    fn app_with_target(path: String) -> AppEntry {
        let mut app = AppEntry {
            id: "tool".to_string(),
            name: "Tool".to_string(),
            description: String::new(),
            category: "Other".to_string(),
            repo: None,
            installed_version: None,
            icon_path: None,
            windows: None,
            linux: None,
        };
        app.set_target_for(Platform::current(), LaunchTarget::Binary { path });
        app
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn resolves_current_target_path() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_base(temp.path());
        let app = app_with_target("Tool.AppImage".to_string());
        let expected = temp.path().join("apps").join("tool").join("Tool.AppImage");
        assert_eq!(current_target_path(&paths, &app), Some(expected));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn strips_appimage_variables_from_linux_children() {
        let mut command = Command::new("env");
        command
            .env("APPIMAGE", "/parent/Parent.AppImage")
            .env("APPDIR", "/tmp/parent.AppDir")
            .env("LD_LIBRARY_PATH", "/tmp/parent.AppDir/usr/lib");
        sanitize_child_environment(&mut command);

        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("APPIMAGE="));
        assert!(!stdout.contains("APPDIR="));
        assert!(!stdout.contains("LD_LIBRARY_PATH="));
        assert!(stdout.contains("PATH="));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn launches_python_script_in_its_own_directory() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("main.py");
        std::fs::write(
            &script,
            "open('marker.txt', 'w', encoding='utf-8').write('launched')\n",
        )
        .unwrap();

        let paths = AppPaths::from_base(temp.path());
        let app = AppEntry {
            id: "python-tool".to_string(),
            name: "Python Tool".to_string(),
            description: String::new(),
            category: "Tools".to_string(),
            repo: None,
            installed_version: None,
            icon_path: None,
            windows: None,
            linux: Some(LaunchTarget::Python {
                script_path: script.to_string_lossy().into_owned(),
                venv_path: None,
                windowless: false,
            }),
        };

        launch(&paths, &app).expect("python3 should launch");
        let marker = temp.path().join("marker.txt");
        for _ in 0..40 {
            if marker.is_file() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "launched");
    }
}
