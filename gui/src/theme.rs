//! Look and feel: color palettes, fonts, sizes and a few styled building blocks

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use eframe::egui::{
    self, epaint::text::VariationCoords, text::LayoutJob, vec2, Align, Color32, CornerRadius, FontData,
    FontDefinitions, FontFamily, FontId, FontSelection, Margin, RichText, Stroke, TextStyle, WidgetText,
};

/// The colors of a theme
pub struct Palette {
    pub name: &'static str,
    pub dark: bool,
    pub bg: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub card_active: Color32,
    /// A widget being pressed
    pub card_pressed: Color32,
    /// Background of text fields, darker than the cards
    pub field: Color32,
    /// Stripes of tables
    pub faint: Color32,
    pub divider: Color32,
    pub text: Color32,
    /// Text of non-interactive widgets
    pub text_quiet: Color32,
    /// Names and descriptions, quieter than the values
    pub label: Color32,
    pub accent: Color32,
    /// Text on [Palette::accent]
    pub on_accent: Color32,
    /// A toggle that is on: quieter than [Palette::accent], which is kept for the main actions
    pub selected: Color32,
    pub warn: Color32,
    pub caution: Color32,
    /// The selected time range in the plots
    pub selection: Color32,
    /// The min/max band of the current plot
    pub band: Color32,
    pub digital: [Color32; 8],
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const DIGITAL_DARK: [Color32; 8] = [
    rgb(0x94d252),
    rgb(0xffc85c),
    rgb(0xff7963),
    rgb(0x5fc8e8),
    rgb(0xb49cf0),
    rgb(0x6ee0b4),
    rgb(0xf096c8),
    rgb(0xc8cc8c),
];

pub static PALETTES: [Palette; 5] = [
    Palette {
        name: "Teal",
        dark: true,
        bg: rgb(0x082831),
        card: rgb(0x103542),
        card_hover: rgb(0x174453),
        card_active: rgb(0x1e5263),
        card_pressed: rgb(0x266275),
        field: rgb(0x062028),
        faint: rgb(0x0c2f3a),
        divider: rgb(0x295563),
        text: rgb(0xeef4f4),
        text_quiet: rgb(0xc8dadb),
        label: rgb(0x8fb1b4),
        accent: rgb(0x94d252),
        on_accent: rgb(0x103108),
        selected: rgb(0x2c5c34),
        warn: rgb(0xff7963),
        caution: rgb(0xffc85c),
        selection: rgb(0x5fc8e8),
        band: rgb(0xc8cc8c),
        digital: DIGITAL_DARK,
    },
    Palette {
        name: "Graphite",
        dark: true,
        bg: rgb(0x1b1d21),
        card: rgb(0x25282d),
        card_hover: rgb(0x30343a),
        card_active: rgb(0x3b4048),
        card_pressed: rgb(0x464c55),
        field: rgb(0x141619),
        faint: rgb(0x202327),
        divider: rgb(0x3d424a),
        text: rgb(0xeceff3),
        text_quiet: rgb(0xc9ced6),
        label: rgb(0x9aa3ae),
        accent: rgb(0x4ea1ff),
        on_accent: rgb(0x06213f),
        selected: rgb(0x23466e),
        warn: rgb(0xff6b5e),
        caution: rgb(0xffc85c),
        selection: rgb(0xffb347),
        band: rgb(0xc0c4cc),
        digital: DIGITAL_DARK,
    },
    Palette {
        name: "Amber",
        dark: true,
        bg: rgb(0x1e1a14),
        card: rgb(0x2a241b),
        card_hover: rgb(0x362e22),
        card_active: rgb(0x43392a),
        card_pressed: rgb(0x504432),
        field: rgb(0x16130e),
        faint: rgb(0x241f17),
        divider: rgb(0x4a3f2e),
        text: rgb(0xf4eee4),
        text_quiet: rgb(0xd9cfbf),
        label: rgb(0xb3a58f),
        accent: rgb(0xffb000),
        on_accent: rgb(0x2b1d00),
        selected: rgb(0x5a4310),
        warn: rgb(0xff6b5e),
        caution: rgb(0xffd27a),
        selection: rgb(0x6ec8ff),
        band: rgb(0xd8c9a8),
        digital: DIGITAL_DARK,
    },
    Palette {
        name: "Light",
        dark: false,
        bg: rgb(0xe9eef0),
        card: rgb(0xffffff),
        card_hover: rgb(0xe1e8eb),
        card_active: rgb(0xd3dde1),
        card_pressed: rgb(0xc4d1d6),
        field: rgb(0xf5f8f9),
        faint: rgb(0xf1f5f6),
        divider: rgb(0xc5d0d4),
        text: rgb(0x13262c),
        text_quiet: rgb(0x2c4148),
        label: rgb(0x587077),
        accent: rgb(0x3a8a2a),
        on_accent: rgb(0xffffff),
        selected: rgb(0xc9e6bd),
        warn: rgb(0xd13f2b),
        caution: rgb(0xb27400),
        selection: rgb(0x1f8fb8),
        band: rgb(0x8a8f55),
        digital: [
            rgb(0x3a8a2a),
            rgb(0xc08a00),
            rgb(0xd13f2b),
            rgb(0x1f8fb8),
            rgb(0x7a5cc8),
            rgb(0x1d9e74),
            rgb(0xc2408a),
            rgb(0x7d8040),
        ],
    },
    Palette {
        name: "High contrast",
        dark: true,
        bg: rgb(0x000000),
        card: rgb(0x121212),
        card_hover: rgb(0x262626),
        card_active: rgb(0x363636),
        card_pressed: rgb(0x474747),
        field: rgb(0x000000),
        faint: rgb(0x0a0a0a),
        divider: rgb(0x707070),
        text: rgb(0xffffff),
        text_quiet: rgb(0xffffff),
        label: rgb(0xd4d4d4),
        accent: rgb(0xffe600),
        on_accent: rgb(0x000000),
        selected: rgb(0x5c5300),
        warn: rgb(0xff4d4d),
        caution: rgb(0xffb000),
        selection: rgb(0x00d0ff),
        band: rgb(0xffffff),
        digital: [
            rgb(0xffe600),
            rgb(0xff9f1a),
            rgb(0xff4d4d),
            rgb(0x00d0ff),
            rgb(0xc59bff),
            rgb(0x3dffb0),
            rgb(0xff6ad5),
            rgb(0xffffff),
        ],
    },
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// The current palette
pub fn p() -> &'static Palette {
    &PALETTES[CURRENT.load(Ordering::Relaxed)]
}

/// Index of the current palette in [PALETTES]
pub fn current() -> usize {
    CURRENT.load(Ordering::Relaxed)
}

const BOLD: &str = "bold";
const RADIUS: u8 = 6;

/// The semibold font, for values and headings
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

/// Set the fonts and the palette with this name (else the first)
pub fn init(ctx: &egui::Context, palette: Option<&str>) {
    ctx.set_fonts(fonts());
    let index = palette.and_then(|name| PALETTES.iter().position(|p| p.name == name)).unwrap_or(0);
    select(ctx, index);
}

/// Switch to palette `index` of [PALETTES]
pub fn select(ctx: &egui::Context, index: usize) {
    CURRENT.store(index % PALETTES.len(), Ordering::Relaxed);
    let p = p();
    ctx.set_theme(if p.dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(12.5)),
            (TextStyle::Body, FontId::proportional(15.)),
            (TextStyle::Button, FontId::proportional(15.)),
            (TextStyle::Heading, bold(20.)),
            (TextStyle::Monospace, FontId::monospace(14.)),
        ]
        .into();

        let s = &mut style.spacing;
        s.item_spacing = vec2(8., 8.);
        s.button_padding = vec2(12., 5.);
        s.interact_size.y = 28.;
        s.window_margin = Margin::same(12);
        s.menu_margin = Margin::same(6);
        s.combo_width = 80.;

        let v = &mut style.visuals;
        *v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.panel_fill = p.bg;
        v.window_fill = p.card;
        v.window_stroke = Stroke::new(1., p.divider);
        v.window_corner_radius = CornerRadius::same(8);
        v.menu_corner_radius = CornerRadius::same(RADIUS);
        v.extreme_bg_color = p.field;
        v.faint_bg_color = p.faint;
        v.weak_text_color = Some(p.label);
        v.hyperlink_color = p.accent;
        v.warn_fg_color = p.caution;
        v.error_fg_color = p.warn;
        v.selection.bg_fill = p.selected;
        v.selection.stroke = Stroke::new(1., p.accent);
        v.striped = true;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = p.bg;
        w.noninteractive.weak_bg_fill = p.bg;
        w.noninteractive.bg_stroke = Stroke::new(1., p.divider);
        w.noninteractive.fg_stroke = Stroke::new(1., p.text_quiet);
        for (state, fill, stroke) in [
            // One step lighter than the cards, so they stand out on them too
            (&mut w.inactive, p.card_hover, Stroke::NONE),
            (&mut w.hovered, p.card_active, Stroke::new(1., p.divider)),
            (&mut w.active, p.card_pressed, Stroke::new(1., p.divider)),
            (&mut w.open, p.card_active, Stroke::new(1., p.divider)),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = stroke;
            state.fg_stroke = Stroke::new(1.5, p.text);
            state.corner_radius = CornerRadius::same(RADIUS);
            state.expansion = 0.;
        }
        for state in [&mut w.noninteractive, &mut w.inactive] {
            state.corner_radius = CornerRadius::same(RADIUS);
        }
    });
}

/// The default fonts, preceded by Bahnschrift (a condensed sans that comes with Windows) if available
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let defaults = fonts.families[&FontFamily::Proportional].clone();
    let mut bold_family = defaults.clone();

    let candidates = [r"C:\Windows\Fonts\bahnschrift.ttf"];
    if let Some(bytes) = candidates.iter().find_map(|path| std::fs::read(path).ok()) {
        let regular = FontData::from_owned(bytes);
        let mut semibold = regular.clone();
        semibold.tweak.coords = VariationCoords::new([("wght", 600.)]);
        fonts.font_data.insert("ui".into(), Arc::new(regular));
        fonts.font_data.insert("ui-bold".into(), Arc::new(semibold));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "ui".into());
        bold_family.insert(0, "ui-bold".into());
    }
    fonts.families.insert(FontFamily::Name(BOLD.into()), bold_family);
    fonts
}

/// A card: a rounded, slightly lighter area, full width
pub fn card<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(p().card)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// The title of a card or section
pub fn title(text: &str) -> RichText {
    RichText::new(text).color(p().label).size(14.)
}

/// A value, large and semibold
pub fn value(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(bold(size)).color(p().text)
}

/// `text` with its first `key` character underlined, marking the keyboard shortcut (case insensitive)
pub fn mnemonic(ui: &egui::Ui, text: &str, key: char) -> WidgetText {
    let Some((i, c)) = text.char_indices().find(|(_, c)| c.eq_ignore_ascii_case(&key)) else {
        return text.into();
    };
    let style = ui.style();
    let mut job = LayoutJob::default();
    let font = FontSelection::Style(TextStyle::Button);
    let end = i + c.len_utf8();
    for (part, underline) in [(&text[..i], false), (&text[i..end], true), (&text[end..], false)] {
        if !part.is_empty() {
            let rich = RichText::new(part);
            let rich = if underline { rich.underline() } else { rich };
            rich.append_to(&mut job, style, font.clone(), Align::Center);
        }
    }
    job.into()
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Normal,
    /// The main action: accent colored
    Primary,
    /// Stops or ends something
    Danger,
}

/// A framed toggle button, green when on
pub fn toggle(ui: &mut egui::Ui, on: &mut bool, text: impl Into<WidgetText>) -> egui::Response {
    let mut response = ui.add(egui::Button::new(text).selected(*on));
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response
}

/// A framed button for one of several choices, green when selected
pub fn choice(ui: &mut egui::Ui, selected: bool, text: impl Into<WidgetText>) -> egui::Response {
    ui.add(egui::Button::new(text).selected(selected))
}

/// A large button, `width` wide (0 for fitting the text), with its keyboard `shortcut` on the right
pub fn big_button<'a>(text: &str, shortcut: &str, kind: Kind, width: f32) -> egui::Button<'a> {
    let p = p();
    let (fill, fg) = match kind {
        Kind::Normal => (p.card_hover, p.text),
        Kind::Primary => (p.accent, p.on_accent),
        Kind::Danger => (Color32::TRANSPARENT, p.warn),
    };
    let mut button = egui::Button::new(RichText::new(text).size(16.).color(fg))
        .fill(fill)
        .corner_radius(CornerRadius::same(RADIUS))
        .min_size(vec2(width, 38.));
    if !shortcut.is_empty() {
        button = button.shortcut_text(RichText::new(shortcut).size(12.5).color(fg.gamma_multiply(0.65)));
    }
    if kind == Kind::Danger {
        button = button.stroke(Stroke::new(1.5, p.warn));
    }
    button
}
