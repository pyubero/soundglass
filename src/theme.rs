//! UI themes, from lightview: XML files that recolour and reshape the fixed UI (see README.md, Themes).

use eframe::egui::{self, Color32, CornerRadius, Vec2};
use std::path::{Path, PathBuf};
use crate::i18n::{tr, trf};

/// The fallback for a missing key or a broken file: the Light template as shipped.
const BASE: &str = include_str!("../themes/light.xml");
/// Written into the themes folder on first launch, as files users edit.
pub const TEMPLATES: [(&str, &str); 3] = [
    ("dark", include_str!("../themes/dark.xml")),
    ("light", BASE),
    ("high-contrast", include_str!("../themes/high-contrast.xml")),
];
/// Follows the system's dark/light setting with the folder's dark.xml and light.xml.
pub const SYSTEM: &str = "system";
const MAX_BYTES: u64 = 64 * 1024;

pub struct Theme {
    /// Which of egui's dark/light slots it fills, and where the values no theme sets come from.
    pub mode: egui::Theme,
    pub visuals: egui::Visuals,
    pub spacing: egui::Spacing,
    /// The played part of the seek bar and waveforms, which egui's `Visuals` has no slot for.
    pub played: Color32,
}

/// The name shown in Settings: the shipped themes by name, others by file name.
pub fn name(id: &str) -> String {
    match id {
        SYSTEM => tr("Follow system").into(),
        "dark" => tr("Dark").into(),
        "light" => tr("Light").into(),
        "high-contrast" => tr("High contrast").into(),
        _ => id.into(),
    }
}

/// Where the theme files live.
pub fn dir() -> Option<PathBuf> {
    eframe::storage_dir("soundglass").map(|d| d.join("themes"))
}

/// Opens the themes folder in the file manager.
pub fn open_folder() {
    if let Some(dir) = dir() {
        seed(&dir);
        crate::ai::open(dir);
    }
}

/// Creates the themes folder with the templates on first launch.
pub fn seed_folder() {
    if let Some(dir) = dir() {
        seed(&dir);
    }
}

/// An existing folder is left alone, so edits and deletions stay and upgrades never overwrite.
fn seed(dir: &Path) {
    if dir.exists() || std::fs::create_dir_all(dir).is_err() {
        return;
    }
    for (id, xml) in TEMPLATES {
        let _ = std::fs::write(dir.join(format!("{id}.xml")), xml);
    }
}

/// The ids of the themes in the folder, sorted.
pub fn user_themes() -> Vec<String> {
    dir().map(|d| themes_in(&d)).unwrap_or_default()
}

fn themes_in(dir: &Path) -> Vec<String> {
    let entries = std::fs::read_dir(dir).ok();
    let mut ids: Vec<String> = entries.into_iter().flatten().flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")))
        .filter_map(|p| p.file_stem()?.to_str().map(String::from))
        .filter(|id| id != SYSTEM)
        .collect();
    ids.sort();
    ids
}

/// Reads a theme from the folder by id.
pub fn load(id: &str) -> Result<Theme, String> {
    read(&dir().ok_or(tr("no data folder"))?, id)
}

fn read(dir: &Path, id: &str) -> Result<Theme, String> {
    let path = dir.join(format!("{id}.xml"));
    let error = |e: std::io::Error| format!("{}: {e}", path.display());
    // A theme is any file a user downloaded: keep it small.
    if std::fs::metadata(&path).map_err(error)?.len() > MAX_BYTES {
        return Err(trf("{path}: bigger than 64 KiB", &[("path", &path.display())]));
    }
    parse(&std::fs::read_to_string(&path).map_err(error)?).map_err(|e| format!("{}: {e}", path.display()))
}

/// The built-in base theme.
fn base() -> Theme {
    parse(BASE).expect("the base theme parses")
}

/// Applies the theme `id` to `ctx`, keeping the text sizes. A theme that fails to load is
/// replaced by the base theme and the error is returned.
pub fn apply(ctx: &egui::Context, id: &str) -> Option<String> {
    let set = |slot, t: &Theme| {
        ctx.style_mut_of(slot, |s| {
            s.visuals = t.visuals.clone();
            s.spacing = t.spacing.clone();
        });
        ctx.data_mut(|d| d.insert_temp(played_id(slot), t.played));
    };
    let or_base = |id| match load(id) {
        Ok(t) => (t, None),
        Err(e) => (base(), Some(e)),
    };
    if id == SYSTEM {
        let ((dark, dark_error), (light, light_error)) = (or_base("dark"), or_base("light"));
        // Each file fills its slot whatever its mode says, so a broken dark.xml shows the base theme.
        set(egui::Theme::Dark, &dark);
        set(egui::Theme::Light, &light);
        ctx.set_theme(egui::ThemePreference::System);
        // On Linux, `main` sets egui's fallback theme from the desktop portal.
        return dark_error.or(light_error);
    }
    let (theme, error) = or_base(id);
    set(theme.mode, &theme);
    ctx.set_theme(theme.mode);
    error
}

fn played_id(slot: egui::Theme) -> egui::Id {
    egui::Id::unique(("theme-played", slot))
}

/// The theme's colour for the played part, for the dark or light slot `ctx` shows now.
pub fn played(ctx: &egui::Context) -> Color32 {
    ctx.data(|d| d.get_temp(played_id(ctx.theme()))).unwrap_or_else(|| ctx.global_style().visuals.selection.bg_fill)
}

/// A theme file: egui's defaults for its mode, then the base theme's values, then the file's.
pub fn parse(xml: &str) -> Result<Theme, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if root.tag_name().name() != "theme" {
        return Err(trf("<{tag}>: invalid value", &[("tag", &root.tag_name().name())]));
    }
    let mode = match root.attribute("mode") {
        None | Some("dark") => egui::Theme::Dark,
        Some("light") => egui::Theme::Light,
        Some(other) => return Err(trf("mode=\"{other}\": invalid value", &[("other", &other)])),
    };
    let mut t = Theme { mode, visuals: mode.default_visuals(), spacing: egui::Spacing::default(), played: Color32::TRANSPARENT };
    apply_elements(&roxmltree::Document::parse(BASE).expect("the base theme is XML"), &mut t)?;
    // Not the base's: a file without it (lightview's) takes its own accent.
    t.played = Color32::TRANSPARENT;
    apply_elements(&doc, &mut t)?;
    if t.played == Color32::TRANSPARENT {
        t.played = t.visuals.selection.bg_fill;
    }
    Ok(t)
}

fn apply_elements(doc: &roxmltree::Document, t: &mut Theme) -> Result<(), String> {
    for node in doc.root_element().children().filter(|n| n.is_element()) {
        let line = doc.text_pos_at(node.range().start).row;
        let bad = |key: &str, v: &str| trf("line {line}: {key}=\"{v}\": invalid value", &[("line", &line), ("key", &key), ("v", &v)]);
        let color = |key: &str| node.attribute(key).map(|v| parse_color(v).ok_or_else(|| bad(key, v))).transpose();
        let number = |key: &str, max: f32| node.attribute(key).map(|v| v.trim().parse::<f32>().ok().filter(|n| n.is_finite()).map(|n| n.clamp(0.0, max)).ok_or_else(|| bad(key, v))).transpose();
        let pair = |key: &str| node.attribute(key).map(|v| parse_pair(v).ok_or_else(|| bad(key, v))).transpose();
        let v = &mut t.visuals;
        match node.tag_name().name() {
            "colors" => {
                if let Some(c) = color("panel")? { v.panel_fill = c; }
                if let Some(c) = color("window")? { v.window_fill = c; }
                if let Some(c) = color("border")? { v.window_stroke.color = c; }
                if let Some(c) = color("field")? { v.extreme_bg_color = c; }
                if let Some(c) = color("text")? { v.widgets.noninteractive.fg_stroke.color = c; }
                if let Some(c) = color("accent")? { v.selection.bg_fill = c; }
                if let Some(c) = color("selected-text")? { v.selection.stroke.color = c; }
                if let Some(c) = color("hyperlink")? { v.hyperlink_color = c; }
                if let Some(c) = color("warn")? { v.warn_fg_color = c; }
                if let Some(c) = color("error")? { v.error_fg_color = c; }
                if let Some(c) = color("played")? { t.played = c; }
            }
            "widget" => {
                let w = match node.attribute("state") {
                    Some("noninteractive") => &mut v.widgets.noninteractive,
                    Some("inactive") => &mut v.widgets.inactive,
                    Some("hovered") => &mut v.widgets.hovered,
                    Some("active") => &mut v.widgets.active,
                    Some("open") => &mut v.widgets.open,
                    other => return Err(bad("state", other.unwrap_or(""))),
                };
                // Buttons fill with weak_bg_fill, other widgets with bg_fill.
                if let Some(c) = color("fill")? { (w.bg_fill, w.weak_bg_fill) = (c, c); }
                if let Some(c) = color("text")? { w.fg_stroke.color = c; }
                if let Some(c) = color("border")? { w.bg_stroke.color = c; }
            }
            "shape" => {
                if let Some(r) = number("rounding", 16.0)? {
                    let r = CornerRadius::same(r as u8);
                    (v.window_corner_radius, v.menu_corner_radius) = (r, r);
                    for w in widgets(v) { w.corner_radius = r; }
                }
                if let Some(s) = number("stroke", 4.0)? {
                    let s = s.max(0.5);
                    v.window_stroke.width = s;
                    for w in widgets(v) { w.bg_stroke.width = s; }
                }
                if let Some(p) = pair("spacing")? { t.spacing.item_spacing = p; }
                if let Some(p) = pair("padding")? { t.spacing.button_padding = p; }
            }
            _ => {} // Unknown elements (such as lightview's canvas) are ignored, so other themes still load.
        }
    }
    Ok(())
}

fn widgets(v: &mut egui::Visuals) -> [&mut egui::style::WidgetVisuals; 5] {
    let w = &mut v.widgets;
    [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open]
}

/// `#rrggbb` or `#rrggbbaa`.
fn parse_color(s: &str) -> Option<Color32> {
    let hex = s.trim().strip_prefix('#').filter(|h| (h.len() == 6 || h.len() == 8) && h.is_ascii());
    let bytes = hex.and_then(|h| (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok()).collect::<Option<Vec<u8>>>());
    match bytes.as_deref() {
        Some([r, g, b]) => Some(Color32::from_rgb(*r, *g, *b)),
        Some([r, g, b, a]) => Some(Color32::from_rgba_unmultiplied(*r, *g, *b, *a)),
        _ => None,
    }
}

/// `x,y`, each clamped to 0–24.
fn parse_pair(s: &str) -> Option<Vec2> {
    let (x, y) = s.split_once(',')?;
    let n = |v: &str| v.trim().parse::<f32>().ok().filter(|n| n.is_finite()).map(|n| n.clamp(0.0, 24.0));
    Some(Vec2::new(n(x)?, n(y)?))
}

/// The desktop's dark/light preference from the XDG desktop portal, which winit doesn't read on
/// Linux. Takes ~35 ms, so call it off the UI thread. From lightview.
#[cfg(target_os = "linux")]
pub fn system_theme() -> Option<egui::Theme> {
    let out = std::process::Command::new("gdbus").args(["call", "--session", "--dest", "org.freedesktop.portal.Desktop", "--object-path", "/org/freedesktop/portal/desktop",
        "--method", "org.freedesktop.portal.Settings.ReadOne", "org.freedesktop.appearance", "color-scheme"]).output().ok()?;
    parse_color_scheme(&String::from_utf8_lossy(&out.stdout))
}

/// gdbus prints `(<uint32 1>,)`: 1 prefers dark, 2 prefers light, 0 has no preference.
#[cfg(target_os = "linux")]
fn parse_color_scheme(reply: &str) -> Option<egui::Theme> {
    match reply.trim().trim_start_matches("(<").trim_start_matches('<').split(['>', ',']).next()?.trim() {
        "uint32 1" => Some(egui::Theme::Dark),
        "uint32 2" => Some(egui::Theme::Light),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `xml` applied over egui's defaults alone, with no base theme underneath.
    fn without_base(xml: &str) -> egui::Visuals {
        let doc = roxmltree::Document::parse(xml).unwrap();
        let mode = if doc.root_element().attribute("mode") == Some("light") { egui::Theme::Light } else { egui::Theme::Dark };
        let mut t = Theme { mode, visuals: mode.default_visuals(), spacing: egui::Spacing::default(), played: Color32::TRANSPARENT };
        apply_elements(&doc, &mut t).unwrap();
        assert_ne!(t.played, Color32::TRANSPARENT);
        t.visuals
    }

    #[test]
    fn templates_parse_and_set_every_key() {
        for (id, xml) in TEMPLATES {
            let t = parse(xml).unwrap_or_else(|e| panic!("{id}: {e}"));
            // Same result with or without the base: the template relies on none of its values.
            assert_eq!(t.visuals, without_base(xml), "{id} leaves a key to the base theme");
        }
        assert_eq!(parse(TEMPLATES[1].1).unwrap().mode, egui::Theme::Light);
    }

    /// WCAG contrast ratio of two opaque colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let lin = |v: u8| { let v = v as f32 / 255.0; if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) } };
            0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn templates_are_readable() {
        for (id, xml) in TEMPLATES {
            let t = parse(xml).unwrap();
            let v = &t.visuals;
            // egui fills the focused window's title bar with `open` but writes the title in the text colour
            // and the close button in `inactive`'s; 3:1 is the minimum for large text and graphics.
            let title = v.widgets.open.weak_bg_fill;
            assert!(contrast(v.widgets.noninteractive.fg_stroke.color, title) >= 3.0, "{id}: window title");
            assert!(contrast(v.widgets.inactive.fg_stroke.color, title) >= 3.0, "{id}: close button");
            assert!(contrast(t.played, v.panel_fill) >= 3.0, "{id}: played part");
        }
    }

    #[test]
    fn missing_keys_take_the_base_values() {
        let t = parse(r##"<theme><colors panel="#102030"/><widget state="hovered" fill="#ff000080"/><unknown/></theme>"##).unwrap();
        let base = base();
        assert_eq!(t.mode, egui::Theme::Dark);
        assert_eq!(t.visuals.panel_fill, Color32::from_rgb(16, 32, 48));
        assert_eq!(t.visuals.widgets.hovered.weak_bg_fill, Color32::from_rgba_unmultiplied(255, 0, 0, 128));
        assert_eq!(t.visuals.window_fill, base.visuals.window_fill);
        assert_eq!(t.visuals.widgets.inactive, base.visuals.widgets.inactive);
        // Played is the exception: left out, it's the file's own accent.
        let t = parse(r##"<theme><colors accent="#123456"/></theme>"##).unwrap();
        assert_eq!(t.played, Color32::from_rgb(0x12, 0x34, 0x56));
        assert_eq!(parse(r##"<theme><colors played="#abcdef"/></theme>"##).unwrap().played, Color32::from_rgb(0xab, 0xcd, 0xef));
    }

    #[test]
    fn bad_values_are_errors_with_their_line() {
        let e = parse("<theme>\n<colors\n panel=\"red\"/></theme>").err().unwrap();
        assert!(e.contains(" 2: panel=\"red\""), "{e}");
        assert!(parse(r#"<theme mode="sepia"/>"#).is_err());
        assert!(parse(r#"<theme><widget state="pressed"/></theme>"#).is_err());
        assert!(parse(r#"<theme><shape spacing="8"/></theme>"#).is_err());
        assert!(parse("<colours/>").is_err());
        assert!(parse("not xml").is_err());
        assert!(parse_color("#ééé").is_none());
    }

    #[test]
    fn numbers_are_clamped() {
        assert!(parse(r#"<theme><shape padding="NaN,1"/></theme>"#).is_err());
        let t = parse(r#"<theme><shape rounding="900" stroke="-3" spacing="100,-1"/></theme>"#).unwrap();
        assert_eq!(t.visuals.window_corner_radius, CornerRadius::same(16));
        assert_eq!(t.visuals.widgets.inactive.bg_stroke.width, 0.5);
        assert_eq!(t.spacing.item_spacing, Vec2::new(24.0, 0.0));
    }

    #[test]
    fn a_missing_theme_applies_the_base() {
        let ctx = egui::Context::default();
        assert!(apply(&ctx, "no such theme, surely").is_some());
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert_eq!(ctx.style_of(egui::Theme::Light).visuals, base().visuals);
        assert_eq!(played(&ctx), base().played);
    }

    #[test]
    fn folder_is_seeded_once_and_read() {
        let dir = std::env::temp_dir().join(format!("soundglass-theme-test-{}", std::process::id())).join("themes");
        seed(&dir);
        assert_eq!(themes_in(&dir), ["dark", "high-contrast", "light"]);
        assert_eq!(read(&dir, "high-contrast").unwrap().visuals.panel_fill, Color32::BLACK);
        std::fs::remove_file(dir.join("dark.xml")).unwrap();
        std::fs::write(dir.join("big.xml"), format!("<theme>{}</theme>", " ".repeat(65 * 1024))).unwrap();
        seed(&dir);
        assert_eq!(themes_in(&dir), ["big", "high-contrast", "light"]);
        assert!(read(&dir, "big").is_err());
        assert!(read(&dir, "dark").is_err());
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn color_scheme_replies() {
        assert_eq!(parse_color_scheme("(<uint32 1>,)\n"), Some(egui::Theme::Dark));
        assert_eq!(parse_color_scheme("(<<uint32 2>>,)"), Some(egui::Theme::Light));
        assert_eq!(parse_color_scheme("(<uint32 0>,)"), None);
    }
}
