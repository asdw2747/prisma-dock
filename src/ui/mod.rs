pub mod add_app;
pub mod card;
pub mod settings;

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Margin, RichText, Stroke, Vec2};

use crate::app::{PrismaDockApp, StatusKind};
use crate::launch;
use crate::theme;

use self::add_app::AddModalOutcome;
use self::card::{CardAction, CardView};
use self::settings::SettingsAction;

impl PrismaDockApp {
    pub(crate) fn draw(&mut self, ctx: &egui::Context, root: &mut egui::Ui) {
        self.top_bar(root);
        self.footer(root);

        let frame = egui::Frame::new()
            .fill(theme::BG)
            .inner_margin(Margin::symmetric(20, 16));
        egui::CentralPanel::default().frame(frame).show(root, |ui| {
            if self.startup_error.is_some() {
                self.banner(ui);
            }
            self.content(ui, ctx);
        });

        self.modals(ctx);
    }

    fn top_bar(&mut self, root: &mut egui::Ui) {
        let mut check = false;
        let mut add = false;
        let mut settings = false;
        let mut reload = false;
        let mut edit = false;
        let mut open_data = false;

        let frame = egui::Frame::new()
            .fill(theme::BG_PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER_SOFT))
            .inner_margin(Margin::symmetric(18, 9));

        egui::Panel::top("prisma-topbar")
            .frame(frame)
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    if let Some(icon) = &self.icon {
                        ui.add(
                            egui::Image::new(egui::load::SizedTexture::new(
                                icon.id(),
                                Vec2::splat(30.0),
                            ))
                            .corner_radius(CornerRadius::same(8)),
                        );
                    }
                    ui.add_space(2.0);
                    ui.label(RichText::new("Prisma Dock").size(19.0).strong());
                    pill(
                        ui,
                        concat!("v", env!("CARGO_PKG_VERSION")),
                        theme::MUTED,
                        Color32::from_rgb(24, 29, 42),
                    );

                    let updates = self.update_count();
                    if updates > 0 {
                        pill(
                            ui,
                            &format!("{updates} update{}", if updates == 1 { "" } else { "s" }),
                            theme::AMBER,
                            theme::AMBER_DARK,
                        );
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Settings").clicked() {
                            settings = true;
                        }

                        let add_button = egui::Button::new(
                            RichText::new("+ Add App").strong().color(theme::ON_ACCENT),
                        )
                        .fill(theme::ACCENT)
                        .corner_radius(CornerRadius::same(8));
                        if ui.add(add_button).clicked() {
                            add = true;
                        }

                        let checking = self.checking_total > 0;
                        let label = if checking {
                            format!(
                                "Checking {}/{}…",
                                self.check_index.max(1),
                                self.checking_total
                            )
                        } else if self.worker_busy {
                            "Working…".to_string()
                        } else {
                            "Check for Updates".to_string()
                        };
                        let can_check = !self.worker_busy && !self.worker_dead;
                        let response = ui.add_enabled(
                            can_check,
                            egui::Button::new(label).min_size(Vec2::new(140.0, 28.0)),
                        );
                        if response.clicked() {
                            check = true;
                        }

                        ui.menu_button("More", |ui| {
                            if ui.button("Reload manifest").clicked() {
                                reload = true;
                                ui.close();
                            }
                            if ui.button("Edit apps.json").clicked() {
                                edit = true;
                                ui.close();
                            }
                            if ui.button("Open data folder").clicked() {
                                open_data = true;
                                ui.close();
                            }
                        });
                    });
                });
            });

        if check {
            self.start_check_all();
        }
        if add {
            self.open_add_modal();
        }
        if settings {
            self.open_settings_modal();
        }
        if reload {
            self.reload_manifest();
        }
        if edit {
            self.edit_manifest();
        }
        if open_data {
            self.open_data_folder();
        }
    }

    fn footer(&mut self, root: &mut egui::Ui) {
        let frame = egui::Frame::new()
            .fill(theme::BG_PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER_SOFT))
            .inner_margin(Margin::symmetric(18, 6));

        egui::Panel::bottom("prisma-footer")
            .frame(frame)
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    if self.worker_busy {
                        ui.add(egui::Spinner::new().size(12.0));
                    }

                    let color = match self.status.kind {
                        StatusKind::Info => theme::MUTED,
                        StatusKind::Success => theme::GREEN,
                        StatusKind::Error => theme::RED,
                    };
                    let width = (ui.available_width() - 390.0).max(150.0);
                    ui.add_sized(
                        [width, 18.0],
                        egui::Label::new(RichText::new(&self.status.text).size(11.5).color(color))
                            .truncate(),
                    )
                    .on_hover_text(&self.status.text);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} • v{}",
                                self.platform.label(),
                                env!("CARGO_PKG_VERSION")
                            ))
                            .size(10.5)
                            .color(theme::MUTED),
                        );
                        let path = self.paths.data_dir.display().to_string();
                        ui.add(
                            egui::Label::new(
                                RichText::new(path.clone())
                                    .size(10.5)
                                    .monospace()
                                    .color(theme::MUTED),
                            )
                            .truncate(),
                        )
                        .on_hover_text(path);
                    });
                });
            });
    }

    fn banner(&mut self, ui: &mut egui::Ui) {
        let Some(error) = self.startup_error.clone() else {
            return;
        };

        let mut reload = false;
        let mut open = false;
        let mut dismiss = false;

        egui::Frame::new()
            .fill(theme::RED_DARK)
            .stroke(Stroke::new(1.0, theme::RED))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("!").strong().size(16.0).color(theme::RED));
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new(&error).size(11.5).color(theme::RED))
                                .wrap(),
                        );
                        ui.horizontal(|ui| {
                            if ui.small_button("Reload").clicked() {
                                reload = true;
                            }
                            if ui.small_button("Open data folder").clicked() {
                                open = true;
                            }
                            if ui.small_button("Dismiss").clicked() {
                                dismiss = true;
                            }
                        });
                    });
                });
            });

        ui.add_space(10.0);

        if reload {
            self.reload_manifest();
            if self.status.kind == StatusKind::Success {
                self.startup_error = None;
                return;
            }
        }
        if open {
            self.open_data_folder();
        }
        if dismiss {
            self.startup_error = None;
        }
    }

    fn content(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Your apps").size(20.0).strong());
            pill(
                ui,
                &format!("{}", self.manifest.apps.len()),
                theme::MUTED,
                Color32::from_rgb(24, 29, 42),
            );

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.checking_total > 0 {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(
                        RichText::new(format!(
                            "Checking {}/{}",
                            self.check_index, self.checking_total
                        ))
                        .size(11.0)
                        .color(theme::MUTED),
                    );
                } else if self.worker_busy {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new("Working…").size(11.0).color(theme::MUTED));
                }
            });
        });
        ui.label(
            RichText::new("Launch and update your suite from one portable dashboard.")
                .size(11.5)
                .color(theme::MUTED),
        );
        ui.add_space(14.0);

        if self.manifest.apps.is_empty() {
            self.empty_state(ui);
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.card_grid(ui, ctx);
            });
    }

    fn card_grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let apps = self.manifest.apps.clone();
        let mut views = Vec::with_capacity(apps.len());

        for app in &apps {
            let runtime = self.runtime_for(&app.id);
            let icon = self.icon_for(ctx, app);
            let target_exists = launch::target_exists(&self.paths, app);
            views.push(CardView {
                app: app.clone(),
                runtime,
                icon,
                target_exists,
                busy: self.worker_busy,
                platform: self.platform,
            });
        }

        let available = ui.available_width().max(1.0);
        let gap = 14.0;
        let min_width = 310.0;
        let columns = (((available + gap) / (min_width + gap)).floor() as usize).clamp(1, 3);
        let card_width =
            ((available - gap * (columns.saturating_sub(1)) as f32) / columns as f32).max(260.0);
        let card_height = 292.0;

        let mut actions = Vec::new();
        for chunk in views.chunks(columns) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for view in chunk {
                    actions.extend(card::show(ui, view, card_width, card_height));
                }
            });
            ui.add_space(gap);
        }

        for action in actions {
            match action {
                CardAction::Launch(app_id) => self.launch_app(&app_id),
                CardAction::Install(app_id) => self.start_install(&app_id),
                CardAction::OpenFolder(app_id) => self.open_app_folder(&app_id),
                CardAction::Configure => self.edit_manifest(),
                CardAction::Check => self.start_check_all(),
                CardAction::CancelInstall => self.cancel_install(),
                CardAction::ChooseIcon(app_id) => self.choose_icon(&app_id),
                CardAction::ClearIcon(app_id) => self.clear_icon(&app_id),
            }
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let mut add = false;
        egui::Frame::new()
            .fill(theme::BG_CARD)
            .corner_radius(CornerRadius::same(14))
            .stroke(Stroke::new(1.0, theme::BORDER_SOFT))
            .inner_margin(Margin::symmetric(24, 44))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No apps registered yet").size(18.0).strong());
                    ui.label(
                        RichText::new(
                            "Use Add App to register an existing .exe, .AppImage, or .py tool, or edit data/apps.json directly.",
                        )
                        .size(12.0)
                        .color(theme::MUTED),
                    );
                    ui.add_space(12.0);
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("+ Add App")
                                    .strong()
                                    .color(theme::ON_ACCENT),
                            )
                            .fill(theme::ACCENT),
                        )
                        .clicked()
                    {
                        add = true;
                    }
                });
            });

        if add {
            self.open_add_modal();
        }
    }

    fn modals(&mut self, ctx: &egui::Context) {
        let mut add_outcome = None;
        if let Some(modal) = self.add_modal.as_mut() {
            add_outcome = modal.show(ctx);
        }
        match add_outcome {
            Some(AddModalOutcome::Add(drafts)) => self.add_apps(drafts),
            Some(AddModalOutcome::Close) => self.add_modal = None,
            None => {}
        }

        let mut settings_action = None;
        if let Some(modal) = self.settings_modal.as_mut() {
            settings_action = modal.show(ctx, self.worker_busy, &self.paths.data_dir);
        }
        if let Some(action) = settings_action {
            self.handle_settings_action(action);
        }
    }

    fn handle_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::Save(settings) => {
                if self.apply_settings(settings) {
                    self.settings_modal = None;
                    self.notify(StatusKind::Success, "Settings saved.");
                }
            }
            SettingsAction::SyncNow(settings) => self.start_sync(settings),
            SettingsAction::Close => self.settings_modal = None,
            SettingsAction::OpenData => self.open_data_folder(),
        }
    }
}

fn pill(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.5).color(fg));
        });
}
