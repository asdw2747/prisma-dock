use std::path::Path;

use eframe::egui::{self, Align, Layout, RichText};

use crate::model::Settings;
use crate::theme;

pub struct SettingsModal {
    pub draft: Settings,
    pub show_token: bool,
    pub sync_status: Option<(bool, String)>,
}

pub enum SettingsAction {
    Save(Settings),
    SyncNow(Settings),
    Close,
    OpenData,
}

impl SettingsModal {
    pub fn new(settings: &Settings) -> Self {
        Self {
            draft: settings.clone(),
            show_token: false,
            sync_status: None,
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        busy: bool,
        data_dir: &Path,
    ) -> Option<SettingsAction> {
        let mut action = None;

        let response = egui::Modal::new(egui::Id::unique("prisma-settings-modal"))
            .frame(theme::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(620.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Settings").size(22.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Close").clicked() {
                            action = Some(SettingsAction::Close);
                        }
                    });
                });
                ui.label(
                    RichText::new("Optional GitHub access for private repos, rate limits, and cross-machine app-list sync.")
                        .size(12.0)
                        .color(theme::MUTED),
                );

                ui.add_space(14.0);
                ui.label(RichText::new("GitHub access").size(13.5).strong());
                ui.add_space(3.0);
                ui.label(
                    RichText::new(
                        "A personal access token is optional. It is stored in data/settings.json; on Unix the file is written with 0600 permissions.",
                    )
                    .size(11.5)
                    .color(theme::MUTED),
                );
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.draft.github_token)
                            .password(!self.show_token)
                            .hint_text("github_pat_… (optional)")
                            .desired_width(430.0),
                    );
                    ui.checkbox(&mut self.show_token, "Show");
                });
                ui.horizontal(|ui| {
                    ui.hyperlink_to(
                        "Create a token",
                        "https://github.com/settings/tokens",
                    );
                    ui.label(
                        RichText::new("Use repo read access for private releases.")
                            .size(11.0)
                            .color(theme::MUTED),
                    );
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(10.0);
                ui.label(RichText::new("Updates").size(13.5).strong());
                ui.add_space(4.0);
                ui.checkbox(
                    &mut self.draft.auto_check_updates,
                    "Check for updates automatically on startup",
                );

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(10.0);
                ui.label(RichText::new("Remote app list").size(13.5).strong());
                ui.add_space(3.0);
                ui.label(
                    RichText::new(
                        "Optional. Point at a raw apps.json URL to sync app definitions between machines. Local installed versions and local-only apps are preserved.",
                    )
                    .size(11.5)
                    .color(theme::MUTED),
                );
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.remote_manifest_url)
                        .hint_text("https://raw.githubusercontent.com/…/apps.json")
                        .desired_width(f32::INFINITY),
                );

                ui.horizontal(|ui| {
                    let can_sync = !busy && !self.draft.remote_manifest_url.trim().is_empty();
                    if ui
                        .add_enabled(can_sync, egui::Button::new("Sync now"))
                        .clicked()
                    {
                        action = Some(SettingsAction::SyncNow(self.draft.clone()));
                    }
                    if busy {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.label(RichText::new("Working…").size(11.5).color(theme::MUTED));
                    }
                    if let Some((ok, message)) = &self.sync_status {
                        ui.add(
                            egui::Label::new(
                                RichText::new(message).size(11.0).color(if *ok {
                                    theme::GREEN
                                } else {
                                    theme::RED
                                }),
                            )
                            .truncate(),
                        )
                        .on_hover_text(message);
                    }
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(10.0);
                ui.label(RichText::new("Portable data").size(13.5).strong());
                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    let path = data_dir.display().to_string();
                    ui.add(
                        egui::Label::new(
                            RichText::new(path.clone()).monospace().size(11.0).color(theme::MUTED),
                        )
                        .truncate(),
                    )
                    .on_hover_text(path);
                    if ui.small_button("Open data folder").clicked() {
                        action = Some(SettingsAction::OpenData);
                    }
                });

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Save settings").strong().color(theme::ON_ACCENT),
                                )
                                .fill(theme::ACCENT),
                            )
                            .clicked()
                        {
                            action = Some(SettingsAction::Save(self.draft.clone()));
                        }
                        if ui.button("Cancel").clicked() {
                            action = Some(SettingsAction::Close);
                        }
                    });
                });
            });

        if action.is_none() && response.should_close() {
            action = Some(SettingsAction::Close);
        }
        action
    }
}
