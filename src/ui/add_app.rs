use std::path::PathBuf;

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Margin, RichText};

use crate::scan::{self, CandidateKind, ScannedCandidate};
use crate::theme;

pub struct CandidateRow {
    pub candidate: ScannedCandidate,
    pub selected: bool,
    pub name: String,
    pub category: String,
    pub venv_path: String,
}

pub struct NewAppDraft {
    pub candidate: ScannedCandidate,
    pub name: String,
    pub category: String,
    pub venv_path: Option<PathBuf>,
}

pub enum AddModalOutcome {
    Add(Vec<NewAppDraft>),
    Close,
}

pub struct AddAppModal {
    rows: Vec<CandidateRow>,
    status: Option<(bool, String)>,
}

impl AddAppModal {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            status: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<AddModalOutcome> {
        let mut outcome = None;

        let response = egui::Modal::new(egui::Id::unique("prisma-add-app-modal"))
            .frame(theme::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(760.0);

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Add apps").size(22.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Close").clicked() {
                            outcome = Some(AddModalOutcome::Close);
                        }
                    });
                });
                ui.label(
                    RichText::new(
                        "Register existing .exe, .AppImage, .py, or .pyw tools. Windows tools are added to the windows section and Linux tools to the linux section.",
                    )
                    .size(11.5)
                    .color(theme::MUTED),
                );

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Scan folder…").clicked() {
                        self.scan_folder();
                    }
                    if ui.button("Add files…").clicked() {
                        self.pick_files();
                    }
                    if !self.rows.is_empty() {
                        ui.separator();
                        if ui.button("Select all").clicked() {
                            for row in &mut self.rows {
                                row.selected = true;
                            }
                        }
                        if ui.button("Clear selection").clicked() {
                            for row in &mut self.rows {
                                row.selected = false;
                            }
                        }
                    }
                });

                if let Some((ok, message)) = &self.status {
                    ui.add_space(5.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(message)
                                .size(11.5)
                                .color(if *ok { theme::GREEN } else { theme::RED }),
                        )
                        .wrap(),
                    );
                }

                ui.add_space(10.0);
                if self.rows.is_empty() {
                    egui::Frame::new()
                        .fill(theme::BG_CARD)
                        .corner_radius(CornerRadius::same(10))
                        .stroke(egui::Stroke::new(1.0, theme::BORDER_SOFT))
                        .inner_margin(Margin::symmetric(18, 28))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new("Scan a folder or add files to get started")
                                        .size(14.0)
                                        .strong(),
                                );
                                ui.label(
                                    RichText::new(
                                        "You can edit each detected name and category before adding it.",
                                    )
                                    .size(11.5)
                                    .color(theme::MUTED),
                                );
                            });
                        });
                } else {
                    let mut remove_index = None;
                    egui::ScrollArea::vertical()
                        .max_height(390.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (index, row) in self.rows.iter_mut().enumerate() {
                                ui.push_id(index, |ui| {
                                    egui::Frame::new()
                                        .fill(theme::BG_CARD)
                                        .corner_radius(CornerRadius::same(10))
                                        .stroke(egui::Stroke::new(1.0, theme::BORDER_SOFT))
                                        .inner_margin(Margin::same(10))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.checkbox(&mut row.selected, "");
                                                kind_badge(ui, row.candidate.kind);

                                                ui.vertical(|ui| {
                                                    ui.add(
                                                        egui::TextEdit::singleline(&mut row.name)
                                                            .desired_width(235.0)
                                                            .hint_text("App name"),
                                                    );
                                                    let path = row.candidate.path.display().to_string();
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(&path)
                                                                .size(10.5)
                                                                .monospace()
                                                                .color(theme::MUTED),
                                                        )
                                                        .truncate(),
                                                    )
                                                    .on_hover_text(path);
                                                });

                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        if ui.small_button("Remove").clicked() {
                                                            remove_index = Some(index);
                                                        }

                                                        let mut choices: Vec<String> =
                                                            scan::CATEGORIES
                                                                .iter()
                                                                .map(|value| (*value).to_string())
                                                                .collect();
                                                        if !choices.contains(&row.category) {
                                                            choices.push(row.category.clone());
                                                        }

                                                        egui::ComboBox::from_id_salt((
                                                            "add-app-category",
                                                            index,
                                                        ))
                                                        .selected_text(&row.category)
                                                        .width(118.0)
                                                        .show_ui(ui, |ui| {
                                                            for category in choices {
                                                                ui.selectable_value(
                                                                    &mut row.category,
                                                                    category.clone(),
                                                                    category,
                                                                );
                                                            }
                                                        });
                                                    },
                                                );
                                            });

                                            if row.candidate.kind == CandidateKind::Python {
                                                ui.add_space(4.0);
                                                ui.horizontal(|ui| {
                                                    ui.add_space(22.0);
                                                    ui.label(
                                                        RichText::new("Venv")
                                                            .size(11.0)
                                                            .color(theme::MUTED),
                                                    );
                                                    ui.add(
                                                        egui::TextEdit::singleline(
                                                            &mut row.venv_path,
                                                        )
                                                        .hint_text("Optional path to .venv")
                                                        .desired_width(f32::INFINITY),
                                                    );
                                                });
                                            }
                                        });
                                });
                                ui.add_space(5.0);
                            }
                        });

                    if let Some(index) = remove_index {
                        self.rows.remove(index);
                    }
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);
                let selected = self.rows.iter().filter(|row| row.selected).count();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} candidate(s) • {} selected",
                            self.rows.len(),
                            selected
                        ))
                        .size(11.5)
                        .color(theme::MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = format!("Add {selected} app(s)");
                        if ui
                            .add_enabled(
                                selected > 0,
                                egui::Button::new(
                                    RichText::new(label).strong().color(theme::ON_ACCENT),
                                )
                                .fill(theme::ACCENT),
                            )
                            .clicked()
                        {
                            outcome = Some(AddModalOutcome::Add(self.drafts()));
                        }
                        if ui.button("Cancel").clicked() {
                            outcome = Some(AddModalOutcome::Close);
                        }
                    });
                });
            });

        if outcome.is_none() && response.should_close() {
            outcome = Some(AddModalOutcome::Close);
        }
        outcome
    }

    fn scan_folder(&mut self) {
        let Some(folder) = rfd::FileDialog::new()
            .set_title("Scan for launchable apps")
            .pick_folder()
        else {
            return;
        };

        match scan::scan_folder(&folder) {
            Ok(result) => {
                let mut added = 0usize;
                let mut skipped = 0usize;
                for candidate in result.candidates {
                    if self.push_candidate(candidate) {
                        added += 1;
                    } else {
                        skipped += 1;
                    }
                }

                let mut message = if added == 0 {
                    format!(
                        "No launchable .exe, .AppImage, .py, or .pyw files found in {}.",
                        folder.display()
                    )
                } else {
                    format!("Found {added} launchable tool(s) in {}.", folder.display())
                };
                if skipped > 0 {
                    message.push_str(&format!(" {skipped} already listed."));
                }
                if result.truncated {
                    message.push_str(" Scan limit reached; add another folder to continue.");
                }
                self.status = Some((added > 0, message));
            }
            Err(error) => self.status = Some((false, error)),
        }
    }

    fn pick_files(&mut self) {
        let Some(files) = rfd::FileDialog::new()
            .set_title("Add launchable apps")
            .add_filter(
                "Launchable tools",
                &[
                    "exe", "EXE", "appimage", "AppImage", "APPIMAGE", "py", "PY", "pyw", "PYW",
                ],
            )
            .pick_files()
        else {
            return;
        };

        let mut added = 0usize;
        let mut failures = Vec::new();
        for file in files {
            match scan::candidate_from_path(&file) {
                Ok(candidate) => {
                    if self.push_candidate(candidate) {
                        added += 1;
                    }
                }
                Err(error) => failures.push(error),
            }
        }

        self.status = if failures.is_empty() {
            Some((added > 0, format!("Added {added} candidate(s).")))
        } else {
            Some((
                false,
                format!(
                    "Added {added} candidate(s). {} file(s) were skipped: {}",
                    failures.len(),
                    failures.join(" ")
                ),
            ))
        };
    }

    fn push_candidate(&mut self, candidate: ScannedCandidate) -> bool {
        if self
            .rows
            .iter()
            .any(|row| row.candidate.path == candidate.path)
        {
            return false;
        }

        let name = candidate.name.clone();
        let category = scan::infer_category(&name).to_string();
        let venv_path = candidate
            .venv_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();

        self.rows.push(CandidateRow {
            candidate,
            selected: true,
            name,
            category,
            venv_path,
        });
        true
    }

    fn drafts(&self) -> Vec<NewAppDraft> {
        self.rows
            .iter()
            .filter(|row| row.selected)
            .map(|row| NewAppDraft {
                candidate: row.candidate.clone(),
                name: row.name.trim().to_string(),
                category: row.category.trim().to_string(),
                venv_path: if row.candidate.kind == CandidateKind::Python {
                    let value = row.venv_path.trim();
                    (!value.is_empty()).then(|| PathBuf::from(value))
                } else {
                    None
                },
            })
            .collect()
    }
}

fn kind_badge(ui: &mut egui::Ui, kind: CandidateKind) {
    let color = match kind {
        CandidateKind::WindowsExe => Color32::from_rgb(120, 180, 255),
        CandidateKind::LinuxAppImage => theme::CYAN,
        CandidateKind::Python => theme::AMBER,
    };

    egui::Frame::new()
        .fill(Color32::from_rgb(24, 29, 42))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(kind.label()).size(10.0).color(color));
        });
}
