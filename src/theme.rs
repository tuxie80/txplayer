use eframe::egui::{self, Color32, CursorIcon, FontFamily, FontId, Margin, Stroke, TextStyle, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeId {
    #[default]
    ObsidianStudio,
    TokyoNight,
    TokyoNightStorm,
    SolarizedDark,
    SolarizedLight,
    Nord,
    CatppuccinMocha,
    GruvboxDark,
}

impl ThemeId {
    pub const ALL: &'static [ThemeId] = &[
        ThemeId::ObsidianStudio,
        ThemeId::TokyoNight,
        ThemeId::TokyoNightStorm,
        ThemeId::SolarizedDark,
        ThemeId::SolarizedLight,
        ThemeId::Nord,
        ThemeId::CatppuccinMocha,
        ThemeId::GruvboxDark,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ThemeId::ObsidianStudio => "Obsidian Studio",
            ThemeId::TokyoNight => "Tokyo Night",
            ThemeId::TokyoNightStorm => "Tokyo Night Storm",
            ThemeId::SolarizedDark => "Solarized Dark",
            ThemeId::SolarizedLight => "Solarized Light",
            ThemeId::Nord => "Nord",
            ThemeId::CatppuccinMocha => "Catppuccin Mocha",
            ThemeId::GruvboxDark => "Gruvbox Dark",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            ThemeId::ObsidianStudio => "Velvet charcoal · Emerald & gold",
            ThemeId::TokyoNight => "Deep midnight indigo · Sapphire & violet",
            ThemeId::TokyoNightStorm => "Twilight slate · Electric cyan",
            ThemeId::SolarizedDark => "Precision cyan · Warm amber",
            ThemeId::SolarizedLight => "Warm parchment · Crisp teal & blue",
            ThemeId::Nord => "Arctic polar night · Frost & aurora",
            ThemeId::CatppuccinMocha => "Warm espresso · Mauve & mint",
            ThemeId::GruvboxDark => "Retro studio · Warm gold & olive",
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            ThemeId::ObsidianStudio => Palette {
                is_light: false,
                background: Color32::from_rgb(14, 16, 17),
                sidebar: Color32::from_rgb(18, 21, 23),
                panel: Color32::from_rgb(21, 25, 27),
                surface: Color32::from_rgb(27, 32, 35),
                surface_elevated: Color32::from_rgb(34, 40, 44),
                hover: Color32::from_rgb(33, 39, 43),
                clicked_row: Color32::from_rgb(44, 52, 57),
                playing_row: Color32::from_rgb(20, 44, 37),
                selected: Color32::from_rgb(26, 48, 42),
                selected_border: Color32::from_rgb(72, 164, 128),
                border: Color32::from_rgb(39, 46, 50),
                border_strong: Color32::from_rgb(58, 68, 74),
                text: Color32::from_rgb(238, 242, 240),
                text_secondary: Color32::from_rgb(192, 202, 198),
                muted: Color32::from_rgb(142, 154, 151),
                faint: Color32::from_rgb(92, 104, 103),
                accent: Color32::from_rgb(86, 214, 162),
                accent_bright: Color32::from_rgb(138, 240, 198),
                accent_secondary: Color32::from_rgb(112, 178, 236),
                accent_dark: Color32::from_rgb(26, 70, 54),
                accent_subtle: Color32::from_rgb(22, 46, 39),
                hires_gold: Color32::from_rgb(238, 196, 104),
                hires_bg: Color32::from_rgb(52, 42, 22),
                hires_border: Color32::from_rgb(118, 94, 46),
                danger: Color32::from_rgb(235, 108, 108),
                danger_bg: Color32::from_rgb(58, 28, 30),
                danger_border: Color32::from_rgb(122, 54, 58),
            },
            ThemeId::TokyoNight => Palette {
                is_light: false,
                background: Color32::from_rgb(26, 27, 38),
                sidebar: Color32::from_rgb(22, 22, 30),
                panel: Color32::from_rgb(20, 21, 30),
                surface: Color32::from_rgb(36, 40, 59),
                surface_elevated: Color32::from_rgb(45, 51, 74),
                hover: Color32::from_rgb(38, 43, 62),
                clicked_row: Color32::from_rgb(50, 56, 80),
                playing_row: Color32::from_rgb(34, 48, 82),
                selected: Color32::from_rgb(40, 52, 87),
                selected_border: Color32::from_rgb(122, 162, 247),
                border: Color32::from_rgb(47, 53, 75),
                border_strong: Color32::from_rgb(65, 72, 104),
                text: Color32::from_rgb(192, 202, 245),
                text_secondary: Color32::from_rgb(169, 177, 214),
                muted: Color32::from_rgb(120, 127, 166),
                faint: Color32::from_rgb(86, 95, 137),
                accent: Color32::from_rgb(122, 162, 247),
                accent_bright: Color32::from_rgb(125, 207, 255),
                accent_secondary: Color32::from_rgb(187, 154, 247),
                accent_dark: Color32::from_rgb(40, 58, 102),
                accent_subtle: Color32::from_rgb(32, 42, 72),
                hires_gold: Color32::from_rgb(224, 175, 104),
                hires_bg: Color32::from_rgb(56, 46, 30),
                hires_border: Color32::from_rgb(138, 108, 62),
                danger: Color32::from_rgb(247, 118, 142),
                danger_bg: Color32::from_rgb(66, 32, 44),
                danger_border: Color32::from_rgb(144, 62, 82),
            },
            ThemeId::TokyoNightStorm => Palette {
                is_light: false,
                background: Color32::from_rgb(36, 40, 59),
                sidebar: Color32::from_rgb(31, 35, 53),
                panel: Color32::from_rgb(28, 32, 49),
                surface: Color32::from_rgb(46, 52, 76),
                surface_elevated: Color32::from_rgb(56, 63, 92),
                hover: Color32::from_rgb(49, 55, 80),
                clicked_row: Color32::from_rgb(62, 70, 100),
                playing_row: Color32::from_rgb(38, 60, 92),
                selected: Color32::from_rgb(45, 64, 96),
                selected_border: Color32::from_rgb(125, 207, 255),
                border: Color32::from_rgb(58, 66, 94),
                border_strong: Color32::from_rgb(80, 90, 126),
                text: Color32::from_rgb(212, 220, 248),
                text_secondary: Color32::from_rgb(176, 186, 224),
                muted: Color32::from_rgb(130, 142, 182),
                faint: Color32::from_rgb(94, 106, 148),
                accent: Color32::from_rgb(125, 207, 255),
                accent_bright: Color32::from_rgb(170, 228, 255),
                accent_secondary: Color32::from_rgb(187, 154, 247),
                accent_dark: Color32::from_rgb(36, 72, 98),
                accent_subtle: Color32::from_rgb(36, 56, 82),
                hires_gold: Color32::from_rgb(224, 175, 104),
                hires_bg: Color32::from_rgb(64, 52, 34),
                hires_border: Color32::from_rgb(144, 114, 68),
                danger: Color32::from_rgb(247, 118, 142),
                danger_bg: Color32::from_rgb(72, 38, 52),
                danger_border: Color32::from_rgb(154, 68, 88),
            },
            ThemeId::SolarizedDark => Palette {
                is_light: false,
                background: Color32::from_rgb(0, 43, 54),
                sidebar: Color32::from_rgb(0, 35, 44),
                panel: Color32::from_rgb(7, 54, 66),
                surface: Color32::from_rgb(10, 62, 75),
                surface_elevated: Color32::from_rgb(16, 74, 88),
                hover: Color32::from_rgb(11, 63, 76),
                clicked_row: Color32::from_rgb(20, 80, 95),
                playing_row: Color32::from_rgb(8, 74, 80),
                selected: Color32::from_rgb(12, 78, 86),
                selected_border: Color32::from_rgb(42, 161, 152),
                border: Color32::from_rgb(18, 76, 90),
                border_strong: Color32::from_rgb(36, 98, 112),
                text: Color32::from_rgb(238, 232, 213),
                text_secondary: Color32::from_rgb(186, 198, 198),
                muted: Color32::from_rgb(131, 148, 150),
                faint: Color32::from_rgb(88, 110, 117),
                accent: Color32::from_rgb(42, 161, 152),
                accent_bright: Color32::from_rgb(88, 206, 196),
                accent_secondary: Color32::from_rgb(38, 139, 210),
                accent_dark: Color32::from_rgb(14, 82, 82),
                accent_subtle: Color32::from_rgb(10, 66, 72),
                hires_gold: Color32::from_rgb(181, 137, 0),
                hires_bg: Color32::from_rgb(54, 52, 24),
                hires_border: Color32::from_rgb(142, 112, 20),
                danger: Color32::from_rgb(220, 50, 47),
                danger_bg: Color32::from_rgb(66, 30, 36),
                danger_border: Color32::from_rgb(148, 48, 46),
            },
            ThemeId::SolarizedLight => Palette {
                is_light: true,
                background: Color32::from_rgb(253, 246, 227),
                sidebar: Color32::from_rgb(238, 232, 213),
                panel: Color32::from_rgb(245, 239, 220),
                surface: Color32::from_rgb(233, 226, 206),
                surface_elevated: Color32::from_rgb(225, 218, 197),
                hover: Color32::from_rgb(232, 225, 204),
                clicked_row: Color32::from_rgb(220, 214, 194),
                playing_row: Color32::from_rgb(206, 228, 236),
                selected: Color32::from_rgb(212, 232, 228),
                selected_border: Color32::from_rgb(42, 161, 152),
                border: Color32::from_rgb(214, 206, 186),
                border_strong: Color32::from_rgb(184, 176, 156),
                text: Color32::from_rgb(30, 48, 54),
                text_secondary: Color32::from_rgb(72, 92, 98),
                muted: Color32::from_rgb(101, 123, 131),
                faint: Color32::from_rgb(147, 161, 161),
                accent: Color32::from_rgb(38, 139, 210),
                accent_bright: Color32::from_rgb(24, 114, 182),
                accent_secondary: Color32::from_rgb(42, 161, 152),
                accent_dark: Color32::from_rgb(206, 228, 244),
                accent_subtle: Color32::from_rgb(220, 236, 242),
                hires_gold: Color32::from_rgb(165, 118, 0),
                hires_bg: Color32::from_rgb(244, 232, 194),
                hires_border: Color32::from_rgb(198, 156, 52),
                danger: Color32::from_rgb(204, 46, 44),
                danger_bg: Color32::from_rgb(248, 220, 216),
                danger_border: Color32::from_rgb(214, 118, 114),
            },
            ThemeId::Nord => Palette {
                is_light: false,
                background: Color32::from_rgb(46, 52, 64),
                sidebar: Color32::from_rgb(39, 44, 54),
                panel: Color32::from_rgb(36, 41, 51),
                surface: Color32::from_rgb(59, 66, 82),
                surface_elevated: Color32::from_rgb(67, 76, 94),
                hover: Color32::from_rgb(60, 68, 84),
                clicked_row: Color32::from_rgb(74, 84, 102),
                playing_row: Color32::from_rgb(52, 72, 92),
                selected: Color32::from_rgb(58, 76, 94),
                selected_border: Color32::from_rgb(136, 192, 208),
                border: Color32::from_rgb(67, 76, 94),
                border_strong: Color32::from_rgb(88, 100, 122),
                text: Color32::from_rgb(236, 239, 244),
                text_secondary: Color32::from_rgb(216, 222, 233),
                muted: Color32::from_rgb(154, 166, 186),
                faint: Color32::from_rgb(112, 124, 146),
                accent: Color32::from_rgb(136, 192, 208),
                accent_bright: Color32::from_rgb(168, 216, 228),
                accent_secondary: Color32::from_rgb(129, 161, 193),
                accent_dark: Color32::from_rgb(54, 86, 102),
                accent_subtle: Color32::from_rgb(48, 68, 82),
                hires_gold: Color32::from_rgb(235, 203, 139),
                hires_bg: Color32::from_rgb(76, 68, 52),
                hires_border: Color32::from_rgb(154, 132, 88),
                danger: Color32::from_rgb(191, 97, 106),
                danger_bg: Color32::from_rgb(76, 48, 56),
                danger_border: Color32::from_rgb(142, 74, 82),
            },
            ThemeId::CatppuccinMocha => Palette {
                is_light: false,
                background: Color32::from_rgb(30, 30, 46),
                sidebar: Color32::from_rgb(24, 24, 37),
                panel: Color32::from_rgb(21, 21, 33),
                surface: Color32::from_rgb(42, 43, 62),
                surface_elevated: Color32::from_rgb(54, 56, 78),
                hover: Color32::from_rgb(47, 48, 66),
                clicked_row: Color32::from_rgb(60, 62, 84),
                playing_row: Color32::from_rgb(50, 42, 78),
                selected: Color32::from_rgb(54, 46, 82),
                selected_border: Color32::from_rgb(203, 166, 247),
                border: Color32::from_rgb(56, 58, 80),
                border_strong: Color32::from_rgb(78, 82, 110),
                text: Color32::from_rgb(205, 214, 244),
                text_secondary: Color32::from_rgb(186, 194, 222),
                muted: Color32::from_rgb(147, 153, 178),
                faint: Color32::from_rgb(108, 112, 134),
                accent: Color32::from_rgb(203, 166, 247),
                accent_bright: Color32::from_rgb(228, 202, 255),
                accent_secondary: Color32::from_rgb(166, 227, 161),
                accent_dark: Color32::from_rgb(74, 54, 106),
                accent_subtle: Color32::from_rgb(50, 42, 74),
                hires_gold: Color32::from_rgb(249, 226, 175),
                hires_bg: Color32::from_rgb(66, 58, 42),
                hires_border: Color32::from_rgb(154, 136, 94),
                danger: Color32::from_rgb(243, 139, 168),
                danger_bg: Color32::from_rgb(72, 38, 52),
                danger_border: Color32::from_rgb(156, 78, 102),
            },
            ThemeId::GruvboxDark => Palette {
                is_light: false,
                background: Color32::from_rgb(40, 40, 40),
                sidebar: Color32::from_rgb(29, 32, 33),
                panel: Color32::from_rgb(33, 35, 35),
                surface: Color32::from_rgb(52, 49, 46),
                surface_elevated: Color32::from_rgb(66, 61, 56),
                hover: Color32::from_rgb(58, 54, 52),
                clicked_row: Color32::from_rgb(74, 68, 63),
                playing_row: Color32::from_rgb(66, 56, 36),
                selected: Color32::from_rgb(68, 58, 38),
                selected_border: Color32::from_rgb(250, 189, 47),
                border: Color32::from_rgb(68, 63, 58),
                border_strong: Color32::from_rgb(96, 88, 80),
                text: Color32::from_rgb(235, 219, 178),
                text_secondary: Color32::from_rgb(213, 196, 161),
                muted: Color32::from_rgb(168, 153, 132),
                faint: Color32::from_rgb(124, 111, 100),
                accent: Color32::from_rgb(250, 189, 47),
                accent_bright: Color32::from_rgb(254, 214, 104),
                accent_secondary: Color32::from_rgb(184, 187, 38),
                accent_dark: Color32::from_rgb(92, 68, 22),
                accent_subtle: Color32::from_rgb(64, 52, 28),
                hires_gold: Color32::from_rgb(254, 128, 25),
                hires_bg: Color32::from_rgb(74, 44, 22),
                hires_border: Color32::from_rgb(166, 92, 34),
                danger: Color32::from_rgb(251, 73, 52),
                danger_bg: Color32::from_rgb(76, 32, 28),
                danger_border: Color32::from_rgb(162, 56, 46),
            },
        }
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub is_light: bool,
    pub background: Color32,
    pub sidebar: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub surface_elevated: Color32,
    pub hover: Color32,
    pub clicked_row: Color32,
    pub playing_row: Color32,
    pub selected: Color32,
    pub selected_border: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_bright: Color32,
    pub accent_secondary: Color32,
    pub accent_dark: Color32,
    pub accent_subtle: Color32,
    pub hires_gold: Color32,
    pub hires_bg: Color32,
    pub hires_border: Color32,
    pub danger: Color32,
    pub danger_bg: Color32,
    pub danger_border: Color32,
}

impl Palette {
    pub fn on_accent(&self) -> Color32 {
        // Compute luminance so text/icons on primary accent fill always have crisp contrast
        let lum = 0.299 * self.accent.r() as f32
            + 0.587 * self.accent.g() as f32
            + 0.114 * self.accent.b() as f32;
        if lum > 130.0 {
            Color32::from_rgb(14, 16, 18)
        } else {
            Color32::from_rgb(248, 250, 252)
        }
    }
}

pub fn apply(context: &egui::Context, theme_id: ThemeId) -> Palette {
    let p = theme_id.palette();
    let egui_theme = if p.is_light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    };
    let mut style = (*context.style_of(egui_theme)).clone();
    style.visuals = if p.is_light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    style.visuals.panel_fill = p.background;
    style.visuals.window_fill = p.panel;
    style.visuals.window_stroke = Stroke::new(1.2, p.border_strong);
    style.visuals.window_corner_radius = egui::CornerRadius::same(8);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(8);
    style.interaction.tooltip_delay = 0.30;
    style.visuals.extreme_bg_color = p.background;
    style.visuals.faint_bg_color = p.surface;
    style.visuals.code_bg_color = p.surface;
    style.visuals.warn_fg_color = p.hires_gold;
    style.visuals.error_fg_color = p.danger;
    style.visuals.override_text_color = Some(p.text);
    style.visuals.selection.bg_fill = p.selected;
    style.visuals.selection.stroke = Stroke::new(1.0, p.accent);
    style.visuals.hyperlink_color = p.accent;
    style.visuals.interact_cursor = Some(CursorIcon::Default);

    style.visuals.widgets.noninteractive.bg_fill = p.surface;
    style.visuals.widgets.noninteractive.weak_bg_fill = p.surface;
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);

    style.visuals.widgets.inactive.bg_fill = p.surface;
    style.visuals.widgets.inactive.weak_bg_fill = p.surface;
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);

    style.visuals.widgets.hovered.bg_fill = p.hover;
    style.visuals.widgets.hovered.weak_bg_fill = p.hover;
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.border_strong);

    style.visuals.widgets.active.bg_fill = p.selected;
    style.visuals.widgets.active.weak_bg_fill = p.selected;
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, p.accent);
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);

    style.visuals.widgets.open.bg_fill = p.selected;
    style.visuals.widgets.open.weak_bg_fill = p.selected;
    style.visuals.widgets.open.fg_stroke = Stroke::new(1.0, p.text);
    style.visuals.widgets.open.bg_stroke = Stroke::new(1.0, p.selected_border);

    style.visuals.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 1,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, if p.is_light { 50 } else { 140 }),
    };
    style.visuals.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, if p.is_light { 42 } else { 125 }),
    };

    style.spacing.item_spacing = Vec2::new(6.0, 4.0);
    style.spacing.button_padding = Vec2::new(9.0, 5.0);
    style.spacing.interact_size = Vec2::new(26.0, 24.0);
    style.spacing.menu_margin = Margin::symmetric(8, 6);
    style.spacing.window_margin = Margin::symmetric(14, 12);
    style.spacing.scroll = egui::style::ScrollStyle::thin();

    style.text_styles = [
        (
            TextStyle::Small,
            FontId::new(10.5, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(12.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(11.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Heading,
            FontId::new(14.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(11.0, FontFamily::Monospace),
        ),
    ]
    .into();
    context.set_style_of(egui_theme, style);
    context.set_theme(egui_theme);
    p
}
