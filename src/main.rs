#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod discover;
mod github;
mod icons;
mod installer;
mod launch;
mod model;
mod naming;
mod paths;
mod scan;
mod theme;
mod ui;
mod worker;

use eframe::egui;

use crate::app::PrismaDockApp;

fn main() -> eframe::Result<()> {
    let title = "Prisma Dock";
    let viewport = egui::ViewportBuilder::default()
        .with_title(format!("Prisma Dock v{}", env!("CARGO_PKG_VERSION")))
        .with_inner_size([1220.0, 820.0])
        .with_min_inner_size([880.0, 580.0])
        .with_icon(PrismaDockApp::app_icon());

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        title,
        options,
        Box::new(|cc| Ok(Box::new(PrismaDockApp::new(cc)))),
    )
}
