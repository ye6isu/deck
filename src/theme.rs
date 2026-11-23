use egui::{Color32, Stroke, Rounding, Visuals, TextStyle, FontId, FontFamily};

#[derive(Clone, Copy)]
pub struct Palette {
    pub background: Color32, pub card: Color32, pub text: Color32,
    pub muted: Color32, pub border: Color32, pub accent: Color32,
    pub hover: Color32, pub track_bg: Color32,
}

pub struct WarmIndustrialTheme;

impl WarmIndustrialTheme {
    // Warm retro industrial / beige palette
    pub const BG_PRIMARY: Color32 = Color32::from_rgb(0xF4, 0xEF, 0xEA);      // Warm paper / beige
    pub const BG_CARD: Color32 = Color32::from_rgb(0xE8, 0xE2, 0xD9);         // Inset card panel
    pub const BORDER_COLOR: Color32 = Color32::from_rgb(0x3A, 0x38, 0x36);    // Muted dark iron
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0x23, 0x22, 0x20);    // Deep charcoal
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x8C, 0x86, 0x7E);      // Industrial ink
    pub const ACCENT_ORANGE: Color32 = Color32::from_rgb(0xD9, 0x6B, 0x43);   // Retro amber
    pub const HOVER_ROW: Color32 = Color32::from_rgb(0xEB, 0xE5, 0xDC);       // Warm hover
    pub const TRACK_BG: Color32 = Color32::from_rgb(0xD8, 0xD0, 0xC4);        // Warm slider/timeline track

    pub fn palette(style: u8, accent: u8) -> Palette {
        let mut p = Palette {
            background: Self::BG_PRIMARY,
            card: Self::BG_CARD,
            text: Self::TEXT_PRIMARY,
            muted: Self::TEXT_MUTED,
            border: Self::BORDER_COLOR,
            accent: Self::ACCENT_ORANGE,
            hover: Self::HOVER_ROW,
            track_bg: Self::TRACK_BG,
        };
        match style {
            1 => {
                p.background = Color32::from_rgb(246, 247, 249);
                p.card = Color32::from_rgb(226, 230, 236);
                p.hover = Color32::from_rgb(235, 238, 243);
                p.track_bg = Color32::from_rgb(214, 219, 227);
            }
            2 => {
                p.background = Color32::from_rgb(29, 31, 35);
                p.card = Color32::from_rgb(43, 46, 52);
                p.text = Color32::from_rgb(237, 239, 242);
                p.muted = Color32::from_rgb(158, 164, 174);
                p.border = Color32::from_rgb(102, 109, 120);
                p.hover = Color32::from_rgb(52, 56, 64);
                p.track_bg = Color32::from_rgb(58, 62, 70);
            }
            _ => {}
        }
        p.accent = match accent { 1 => Color32::from_rgb(76, 145, 220), 2 => Color32::from_rgb(69, 163, 118),
            3 => Color32::from_rgb(169, 116, 216), _ => Self::ACCENT_ORANGE };
        p
    }

    pub fn apply(ctx: &egui::Context, colors: Palette) {
        let mut visuals = if colors.text.r() > colors.background.r() {
            Visuals::dark()
        } else {
            Visuals::light()
        };

        visuals.panel_fill = colors.background;
        visuals.window_fill = colors.background;
        visuals.extreme_bg_color = colors.card;
        visuals.override_text_color = Some(colors.text);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, colors.text);
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, colors.border);

        // Inactive widgets
        visuals.widgets.inactive.bg_fill = colors.card;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, colors.border);
        visuals.widgets.inactive.rounding = Rounding::same(2.0);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, colors.text);

        // Hovered widgets
        visuals.widgets.hovered.bg_fill = colors.card;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.2, colors.border);
        visuals.widgets.hovered.rounding = Rounding::same(2.0);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.2, colors.text);

        // Active / Pressed widgets
        visuals.widgets.active.bg_fill = colors.border;
        visuals.widgets.active.bg_stroke = Stroke::new(1.2, colors.border);
        visuals.widgets.active.rounding = Rounding::same(2.0);
        visuals.widgets.active.fg_stroke = Stroke::new(1.2, colors.background);

        visuals.selection.bg_fill = colors.accent;
        visuals.selection.stroke = Stroke::new(1.0, colors.border);

        // Dropdown menu / ComboBox / Popup visuals matching theme
        visuals.menu_rounding = Rounding::same(4.0);
        visuals.window_rounding = Rounding::same(4.0);
        visuals.window_stroke = Stroke::new(1.0, colors.border);

        ctx.set_visuals(visuals);

        let mut style = (*ctx.style()).clone();
        style.text_styles = [
            (TextStyle::Heading, FontId::new(14.0, FontFamily::Monospace)),
            (TextStyle::Body, FontId::new(12.0, FontFamily::Monospace)),
            (TextStyle::Monospace, FontId::new(11.0, FontFamily::Monospace)),
            (TextStyle::Button, FontId::new(11.0, FontFamily::Monospace)),
            (TextStyle::Small, FontId::new(9.0, FontFamily::Monospace)),
        ].into();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        ctx.set_style(style);
    }
}
