use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle};

pub const BG: Color32 = Color32::from_rgb(8, 10, 16);
pub const BG_PANEL: Color32 = Color32::from_rgb(14, 17, 26);
pub const BG_CARD: Color32 = Color32::from_rgb(19, 23, 34);
pub const BG_INPUT: Color32 = Color32::from_rgb(10, 13, 20);
pub const BORDER: Color32 = Color32::from_rgb(37, 44, 60);
pub const BORDER_SOFT: Color32 = Color32::from_rgb(29, 35, 49);
pub const TEXT: Color32 = Color32::from_rgb(230, 234, 243);
pub const MUTED: Color32 = Color32::from_rgb(139, 150, 170);
pub const ACCENT: Color32 = Color32::from_rgb(139, 124, 246);
pub const ACCENT_DARK: Color32 = Color32::from_rgb(45, 37, 79);
pub const ON_ACCENT: Color32 = Color32::from_rgb(13, 11, 22);
pub const CYAN: Color32 = Color32::from_rgb(79, 209, 197);
pub const GREEN: Color32 = Color32::from_rgb(74, 222, 128);
pub const GREEN_DARK: Color32 = Color32::from_rgb(18, 48, 34);
pub const AMBER: Color32 = Color32::from_rgb(245, 184, 66);
pub const AMBER_DARK: Color32 = Color32::from_rgb(56, 43, 17);
pub const RED: Color32 = Color32::from_rgb(248, 113, 113);
pub const RED_DARK: Color32 = Color32::from_rgb(60, 24, 29);

pub fn configure(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.visuals = egui::Visuals::dark();
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.panel_fill = BG;
        style.visuals.window_fill = BG_PANEL;
        style.visuals.window_stroke = Stroke::new(1.0, BORDER);
        style.visuals.window_corner_radius = CornerRadius::same(16);
        style.visuals.menu_corner_radius = CornerRadius::same(10);
        style.visuals.extreme_bg_color = BG_INPUT;
        style.visuals.faint_bg_color = BG_CARD;
        style.visuals.code_bg_color = BG_INPUT;
        style.visuals.selection.bg_fill = ACCENT_DARK;
        style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
        style.visuals.hyperlink_color = CYAN;
        style.visuals.widgets.noninteractive.bg_fill = BG_PANEL;
        style.visuals.widgets.noninteractive.weak_bg_fill = BG_PANEL;
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        style.visuals.widgets.noninteractive.corner_radius = CornerRadius::same(8);

        style.visuals.widgets.inactive.bg_fill = BG_INPUT;
        style.visuals.widgets.inactive.weak_bg_fill = BG_INPUT;
        style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        style.visuals.widgets.inactive.corner_radius = CornerRadius::same(8);

        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(31, 37, 53);
        style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(31, 37, 53);
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        style.visuals.widgets.hovered.corner_radius = CornerRadius::same(8);

        style.visuals.widgets.active.bg_fill = ACCENT_DARK;
        style.visuals.widgets.active.weak_bg_fill = ACCENT_DARK;
        style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
        style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
        style.visuals.widgets.active.corner_radius = CornerRadius::same(8);

        style.visuals.widgets.open.bg_fill = BG_CARD;
        style.visuals.widgets.open.weak_bg_fill = BG_CARD;
        style.visuals.widgets.open.bg_stroke = Stroke::new(1.0, BORDER);
        style.visuals.widgets.open.corner_radius = CornerRadius::same(8);

        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.interact_size = egui::vec2(40.0, 28.0);
        style.spacing.combo_width = 130.0;

        style
            .text_styles
            .insert(TextStyle::Heading, FontId::proportional(22.0));
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(13.5));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::proportional(11.5));
    });
}

pub fn category_color(category: &str) -> Color32 {
    match category.to_ascii_lowercase().as_str() {
        "comics" => Color32::from_rgb(196, 148, 255),
        "media" => CYAN,
        "files" => AMBER,
        "images" => Color32::from_rgb(244, 132, 189),
        "audio" => GREEN,
        "tools" => Color32::from_rgb(120, 180, 255),
        _ => MUTED,
    }
}

pub fn app_accent(name: &str) -> Color32 {
    const PALETTE: [Color32; 8] = [
        Color32::from_rgb(139, 124, 246),
        Color32::from_rgb(79, 209, 197),
        Color32::from_rgb(244, 132, 189),
        Color32::from_rgb(120, 180, 255),
        Color32::from_rgb(91, 217, 155),
        Color32::from_rgb(245, 158, 114),
        Color32::from_rgb(183, 156, 255),
        Color32::from_rgb(78, 205, 196),
    ];

    let hash = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3)
    });
    PALETTE[(hash as usize) % PALETTE.len()]
}

pub fn card_frame(highlight: bool) -> egui::Frame {
    egui::Frame::new()
        .fill(BG_CARD)
        .stroke(Stroke::new(
            1.0,
            if highlight { ACCENT } else { BORDER_SOFT },
        ))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::same(15))
}

pub fn modal_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BG_PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(egui::Margin::same(20))
}
