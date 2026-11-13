use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, RichText, Sense,
    TextureHandle, Vec2,
};

use crate::app::{AppRuntime, InstallState};
use crate::model::{AppEntry, Platform};
use crate::naming::{initials, truncate_chars};
use crate::theme;

pub struct CardView {
    pub app: AppEntry,
    pub runtime: AppRuntime,
    pub icon: Option<TextureHandle>,
    pub target_exists: bool,
    pub busy: bool,
    pub platform: Platform,
}

#[derive(Debug, Clone)]
pub enum CardAction {
    Launch(String),
    Install(String),
    OpenFolder(String),
    Configure,
    Check,
    CancelInstall,
}

pub fn show(ui: &mut egui::Ui, view: &CardView, width: f32, height: f32) -> Vec<CardAction> {
    let mut actions = Vec::new();
    let highlight = view
        .runtime
        .update_available(view.app.installed_version.as_deref().unwrap_or(""))
        && view.runtime.has_asset();

    ui.allocate_ui_with_layout(
        Vec2::new(width, height),
        Layout::top_down(Align::Min),
        |ui| {
            theme::card_frame(highlight).show(ui, |ui| {
                ui.set_min_size(Vec2::new(width - 34.0, height - 34.0));
                card_content(ui, view, &mut actions);
            });
        },
    );

    actions
}

fn card_content(ui: &mut egui::Ui, view: &CardView, actions: &mut Vec<CardAction>) {
    let app = &view.app;
    let release = view.runtime.release.as_ref();
    let update_available = view
        .runtime
        .update_available(app.installed_version.as_deref().unwrap_or(""));
    let has_asset = view.runtime.has_asset();
    let in_progress = matches!(
        view.runtime.install,
        InstallState::Downloading { .. } | InstallState::Installing
    );

    ui.horizontal(|ui| {
        app_icon(ui, view);
        ui.add_space(2.0);
        ui.vertical(|ui| {
            ui.add(egui::Label::new(RichText::new(&app.name).size(16.0).strong()).truncate());
            ui.add_space(2.0);
            badge(
                ui,
                &app.category,
                theme::category_color(&app.category),
                Color32::from_rgb(24, 29, 42),
            );
        });
    });

    ui.add_space(9.0);
    ui.add(
        egui::Label::new(
            RichText::new(truncate_chars(&app.description, 140))
                .size(12.5)
                .color(theme::MUTED),
        )
        .wrap(),
    );

    ui.add_space(8.0);
    ui.spacing_mut().item_spacing.x = 6.0;
    ui.horizontal_wrapped(|ui| {
        badge(
            ui,
            &app.version_label(view.target_exists),
            theme::MUTED,
            Color32::from_rgb(12, 15, 23),
        );

        if let Some(release) = release {
            let (fg, bg) = if update_available {
                (theme::AMBER, theme::AMBER_DARK)
            } else {
                (theme::GREEN, theme::GREEN_DARK)
            };
            let label = format!("Latest v{}", release.version);
            let response = badge(ui, &truncate_chars(&label, 26), fg, bg);
            let hover = if let Some(name) = &release.name {
                format!("{}\n{}", name, release.html_url)
            } else {
                release.html_url.clone()
            };
            response.on_hover_text(hover);
        } else {
            match &view.runtime.check {
                crate::app::CheckState::Checking => {
                    badge(ui, "Checking…", theme::MUTED, Color32::from_rgb(24, 29, 42));
                }
                crate::app::CheckState::Failed(_) => {
                    badge(ui, "Check failed", theme::RED, theme::RED_DARK);
                }
                crate::app::CheckState::Unchecked if app.repo.is_some() => {
                    badge(
                        ui,
                        "Not checked",
                        theme::MUTED,
                        Color32::from_rgb(24, 29, 42),
                    );
                }
                _ => {}
            }
        }
    });

    let note = card_note(view, release.is_some(), has_asset);
    if let Some((text, color, hover)) = note {
        ui.add_space(5.0);
        ui.add(
            egui::Label::new(RichText::new(&text).size(11.0).color(color))
                .wrap()
                .sense(Sense::hover()),
        )
        .on_hover_text(hover);
    }

    let bottom_space = if in_progress { 58.0 } else { 34.0 };
    let remaining = ui.available_height() - bottom_space;
    if remaining > 0.0 {
        ui.add_space(remaining);
    }

    if let InstallState::Downloading { downloaded, total } = view.runtime.install {
        let fraction = total
            .filter(|total| *total > 0)
            .map(|total| (downloaded as f32 / total as f32).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        let label = match total {
            Some(total) => format!(
                "Downloading {} of {}",
                format_bytes(downloaded),
                format_bytes(total)
            ),
            None => format!("Downloading {}", format_bytes(downloaded)),
        };
        ui.add(
            egui::ProgressBar::new(fraction)
                .text(RichText::new(label).size(11.0))
                .animate(total.is_none())
                .corner_radius(CornerRadius::same(5)),
        );
    } else if matches!(view.runtime.install, InstallState::Installing) {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(
                RichText::new("Installing update…")
                    .size(11.5)
                    .color(theme::MUTED),
            );
        });
    }

    ui.add_space(5.0);
    ui.horizontal(|ui| {
        if in_progress && ui.small_button("Cancel").clicked() {
            actions.push(CardAction::CancelInstall);
        }

        if ui.small_button("Open folder").clicked() {
            actions.push(CardAction::OpenFolder(app.id.clone()));
        }

        if update_available
            && view.target_exists
            && !view.busy
            && ui.small_button("Launch").clicked()
        {
            actions.push(CardAction::Launch(app.id.clone()));
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            primary_button(ui, view, actions, update_available);
        });
    });
}

fn primary_button(
    ui: &mut egui::Ui,
    view: &CardView,
    actions: &mut Vec<CardAction>,
    update_available: bool,
) {
    let app = &view.app;
    let has_asset = view.runtime.has_asset();
    let enabled = !view.busy;

    enum Primary {
        Launch,
        Install,
        Update,
        Check(&'static str),
        Configure,
        Disabled(&'static str, &'static str),
    }

    let primary = if view.target_exists {
        if update_available && has_asset {
            Primary::Update
        } else {
            Primary::Launch
        }
    } else if view.runtime.release.is_some() && has_asset {
        Primary::Install
    } else if app.repo.is_some() {
        match &view.runtime.check {
            crate::app::CheckState::Failed(_) => Primary::Check("Retry check"),
            crate::app::CheckState::Unchecked => Primary::Check("Check"),
            crate::app::CheckState::Checking => Primary::Disabled("Checking…", "Checking GitHub…"),
            crate::app::CheckState::Ok => Primary::Disabled(
                "No asset",
                "The latest release has no compatible asset for this platform.",
            ),
        }
    } else {
        Primary::Configure
    };

    let add_button = |ui: &mut egui::Ui, label: &str, fill: Color32, text: Color32| {
        ui.add_enabled(
            enabled,
            egui::Button::new(RichText::new(label).strong().color(text))
                .fill(fill)
                .corner_radius(CornerRadius::same(8))
                .min_size(Vec2::new(94.0, 28.0)),
        )
    };

    match primary {
        Primary::Launch => {
            if add_button(ui, "Launch", theme::ACCENT, theme::ON_ACCENT).clicked() {
                actions.push(CardAction::Launch(app.id.clone()));
            }
        }
        Primary::Install => {
            if add_button(ui, "Install", theme::ACCENT, theme::ON_ACCENT).clicked() {
                actions.push(CardAction::Install(app.id.clone()));
            }
        }
        Primary::Update => {
            if add_button(ui, "Update", theme::AMBER, theme::ON_ACCENT).clicked() {
                actions.push(CardAction::Install(app.id.clone()));
            }
        }
        Primary::Check(label) => {
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new(label).min_size(Vec2::new(94.0, 28.0)),
                )
                .clicked()
            {
                actions.push(CardAction::Check);
            }
        }
        Primary::Configure => {
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new("Set up").min_size(Vec2::new(94.0, 28.0)),
                )
                .clicked()
            {
                actions.push(CardAction::Configure);
            }
        }
        Primary::Disabled(label, hover) => {
            ui.add_enabled(
                false,
                egui::Button::new(label).min_size(Vec2::new(94.0, 28.0)),
            )
            .on_disabled_hover_text(hover);
        }
    }
}

fn card_note(
    view: &CardView,
    has_release: bool,
    has_asset: bool,
) -> Option<(String, Color32, String)> {
    if let InstallState::Failed(error) = &view.runtime.install {
        return Some((error.clone(), theme::RED, error.clone()));
    }

    if let crate::app::CheckState::Failed(error) = &view.runtime.check {
        return Some((error.clone(), theme::RED, error.clone()));
    }

    if has_release && !has_asset {
        let text = format!(
            "No compatible {} asset in the latest release.",
            view.platform.label()
        );
        return Some((text.clone(), theme::AMBER, text));
    }

    if !view.target_exists {
        let text = if view.app.repo.is_some() {
            format!(
                "No {} launch target yet — install it, or use Set up.",
                view.platform.label()
            )
        } else {
            format!("No {} launch target configured.", view.platform.label())
        };
        return Some((text.clone(), theme::MUTED, text));
    }

    None
}

fn app_icon(ui: &mut egui::Ui, view: &CardView) {
    let size = Vec2::splat(42.0);
    if let Some(texture) = &view.icon {
        ui.add(
            egui::Image::new(egui::load::SizedTexture::new(texture.id(), size))
                .corner_radius(CornerRadius::same(10)),
        );
        return;
    }

    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let accent = theme::app_accent(&view.app.name);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(10), accent);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        initials(&view.app.name),
        FontId::proportional(15.0),
        Color32::from_rgb(12, 10, 20),
    );
}

fn badge(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).color(fg));
        })
        .response
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes / KB)
    } else {
        format!("{bytes:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_download_sizes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn detects_newer_release() {
        let runtime = AppRuntime::default();
        assert!(!runtime.update_available("1.0.0"));
    }

    #[test]
    fn card_note_keeps_the_full_no_releases_message() {
        let message =
            "No releases found. Check the repo name, or add a token for a private repository.";
        let view = CardView {
            app: AppEntry {
                id: "tool".to_string(),
                name: "Tool".to_string(),
                description: String::new(),
                category: "Other".to_string(),
                repo: Some("owner/tool".to_string()),
                installed_version: None,
                icon_path: None,
                windows: None,
                linux: None,
            },
            runtime: AppRuntime {
                check: crate::app::CheckState::Failed(message.to_string()),
                ..AppRuntime::default()
            },
            icon: None,
            target_exists: true,
            busy: false,
            platform: Platform::Linux,
        };

        let (text, _, hover) = card_note(&view, false, false).expect("note should be present");
        assert_eq!(text, message);
        assert_eq!(hover, message);
        assert!(text.chars().count() > 58);
    }
}
