//! Themes in the Warp theme format (https://github.com/warpdotdev/themes).
//!
//! A theme colours the whole window, not only the terminal: background (solid
//! or a top→bottom gradient, optionally an image), accent (solid or a
//! left→right gradient), foreground, surface shading (`details: darker|lighter`)
//! and the 16 ANSI colours. Built-in themes are bundled Warp theme files; users
//! can drop any Warp `.yaml` theme into `~/.config/cmux-linux/themes/`.
//!
//! The special theme `default` keeps the native GNOME/libadwaita look and
//! follows the system light/dark setting.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use yaml_rust2::{Yaml, YamlLoader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn parse(s: &str) -> Option<Rgb> {
        let h = s.trim().trim_start_matches('#');
        let v = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
        match h.len() {
            6 | 8 => Some(Rgb(v(0, 2)?, v(2, 2)?, v(4, 2)?)),
            3 => Some(Rgb(v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17)),
            _ => None,
        }
    }
    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
    /// Relative luminance (sRGB approximation), 0..1.
    pub fn luminance(&self) -> f64 {
        let c = |v: u8| {
            let x = v as f64 / 255.0;
            if x <= 0.03928 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * c(self.0) + 0.7152 * c(self.1) + 0.0722 * c(self.2)
    }
    /// Linear blend: t=0 → self, t=1 → other.
    pub fn mix(&self, o: Rgb, t: f64) -> Rgb {
        let m = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round().clamp(0.0, 255.0) as u8;
        Rgb(m(self.0, o.0), m(self.1, o.1), m(self.2, o.2))
    }
    pub fn rgba(&self, a: f64) -> String {
        format!("rgba({},{},{},{a:.3})", self.0, self.1, self.2)
    }
}

const BLACK: Rgb = Rgb(0, 0, 0);
const WHITE: Rgb = Rgb(255, 255, 255);

/// A colour that is either solid or a two-stop gradient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    Solid(Rgb),
    /// Background: top → bottom. Accent: left → right.
    Gradient(Rgb, Rgb),
}

impl Paint {
    pub fn average(&self) -> Rgb {
        match self {
            Paint::Solid(c) => *c,
            Paint::Gradient(a, b) => a.mix(*b, 0.5),
        }
    }
    pub fn first(&self) -> Rgb {
        match self {
            Paint::Solid(c) | Paint::Gradient(c, _) => *c,
        }
    }
    fn css(&self, direction: &str) -> String {
        match self {
            Paint::Solid(c) => c.hex(),
            Paint::Gradient(a, b) => format!("linear-gradient(to {direction}, {}, {})", a.hex(), b.hex()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundImage {
    pub path: PathBuf,
    /// 0–100: how strongly the image shows through the background colour.
    pub opacity: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub background: Paint,
    pub accent: Paint,
    pub foreground: Rgb,
    pub cursor: Option<Rgb>,
    /// `details: lighter` → UI surfaces lighter than the background; otherwise darker.
    pub details_lighter: bool,
    pub normal: [Rgb; 8],
    pub bright: [Rgb; 8],
    pub background_image: Option<BackgroundImage>,
    /// The native libadwaita look (the `default` theme).
    pub native: bool,
    pub user: bool,
}

const COLOR_NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
const TANGO: [&str; 16] = [
    "#2e3436", "#cc0000", "#4e9a06", "#c4a000", "#3465a4", "#75507b", "#06989a", "#d3d7cf", "#555753", "#ef2929", "#8ae234", "#fce94f",
    "#729fcf", "#ad7fa8", "#34e2e2", "#eeeeec",
];

fn title_case(id: &str) -> String {
    id.split(['_', '-', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn paint(y: &Yaml, a: &str, b: &str) -> Option<Paint> {
    if let Some(s) = y.as_str() {
        return Rgb::parse(s).map(Paint::Solid);
    }
    let first = Rgb::parse(y[a].as_str()?)?;
    let second = Rgb::parse(y[b].as_str()?)?;
    Some(Paint::Gradient(first, second))
}

/// Parse a Warp theme file. `base_dir` resolves a relative `background_image.path`.
pub fn parse_warp(id: &str, text: &str, base_dir: Option<&Path>) -> Result<Theme, String> {
    let docs = YamlLoader::load_from_str(text).map_err(|e| format!("{id}: invalid YAML: {e}"))?;
    let y = docs.first().ok_or_else(|| format!("{id}: empty theme file"))?;
    let background = paint(&y["background"], "top", "bottom").ok_or_else(|| format!("{id}: missing or invalid `background`"))?;
    let foreground = y["foreground"]
        .as_str()
        .and_then(Rgb::parse)
        .ok_or_else(|| format!("{id}: missing or invalid `foreground`"))?;
    let accent = paint(&y["accent"], "left", "right").unwrap_or(Paint::Solid(foreground));
    let tc = &y["terminal_colors"];
    let mut normal = [BLACK; 8];
    let mut bright = [BLACK; 8];
    for (i, n) in COLOR_NAMES.iter().enumerate() {
        let fallback = Rgb::parse(TANGO[i]).unwrap();
        normal[i] = tc["normal"][*n].as_str().and_then(Rgb::parse).unwrap_or(fallback);
        bright[i] = tc["bright"][*n].as_str().and_then(Rgb::parse).unwrap_or(normal[i]);
    }
    let background_image = match (&y["background_image"]["path"].as_str(), base_dir) {
        (Some(p), Some(dir)) => {
            let path = Path::new(p);
            let path = if path.is_absolute() { path.to_path_buf() } else { dir.join(path) };
            let opacity = y["background_image"]["opacity"].as_i64().unwrap_or(30).clamp(0, 100) as u8;
            path.is_file().then_some(BackgroundImage { path, opacity })
        }
        _ => None,
    };
    Ok(Theme {
        id: id.to_string(),
        name: y["name"].as_str().map(str::to_string).unwrap_or_else(|| title_case(id)),
        background,
        accent,
        foreground,
        cursor: y["cursor"].as_str().and_then(Rgb::parse),
        details_lighter: y["details"].as_str() == Some("lighter"),
        normal,
        bright,
        background_image,
        native: false,
        user: false,
    })
}

/// The native look; colours depend on the system light/dark preference.
pub fn default_theme(dark: bool) -> Theme {
    let pal: Vec<Rgb> = TANGO.iter().map(|c| Rgb::parse(c).unwrap()).collect();
    let (fg, bg) = if dark {
        (Rgb(0xe6, 0xe6, 0xe6), Rgb(0x1e, 0x1e, 0x22))
    } else {
        (Rgb(0x20, 0x21, 0x24), Rgb(0xfb, 0xfb, 0xfb))
    };
    Theme {
        id: "default".into(),
        name: "Default (system)".into(),
        background: Paint::Solid(bg),
        accent: Paint::Solid(Rgb(0x35, 0x84, 0xe4)),
        foreground: fg,
        cursor: None,
        details_lighter: !dark,
        normal: pal[..8].try_into().unwrap(),
        bright: pal[8..].try_into().unwrap(),
        background_image: None,
        native: true,
        user: false,
    }
}

fn builtins() -> &'static [Theme] {
    static B: OnceLock<Vec<Theme>> = OnceLock::new();
    B.get_or_init(|| {
        crate::theme_builtin::BUILTIN
            .iter()
            .filter_map(|(id, text)| {
                parse_warp(id, text, None)
                    .map_err(|e| eprintln!("cmux-linux: bundled theme {e}"))
                    .ok()
            })
            .collect()
    })
}

pub fn user_dir() -> PathBuf {
    crate::paths::config_dir().join("themes")
}

/// Themes from `dir` (*.yaml, *.yml). Invalid files are reported, not fatal.
pub fn load_dir(dir: &Path) -> (Vec<Theme>, Vec<String>) {
    let (mut out, mut errors) = (Vec::new(), Vec::new());
    let Ok(rd) = std::fs::read_dir(dir) else { return (out, errors) };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    files.sort();
    for p in files {
        let id = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        match std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|t| parse_warp(&id, &t, Some(dir)))
        {
            Ok(mut t) => {
                t.user = true;
                out.push(t);
            }
            Err(e) => errors.push(format!("{}: {e}", p.display())),
        }
    }
    (out, errors)
}

/// Older config names → current ids.
fn alias(name: &str) -> &str {
    match name {
        "solarized" => "solarized_dark",
        "gruvbox" => "gruvbox_dark",
        other => other,
    }
}

/// Every available theme: default, then built-ins and user themes sorted by
/// name. A user theme with the same id replaces the built-in one.
pub fn all(dark: bool) -> Vec<Theme> {
    let mut list: Vec<Theme> = builtins().to_vec();
    for t in load_dir(&user_dir()).0 {
        list.retain(|b| b.id != t.id);
        list.push(t);
    }
    list.sort_by_key(|t| t.name.to_lowercase());
    list.insert(0, default_theme(dark));
    list
}

/// Resolve a configured theme name; unknown names fall back to `default`.
pub fn resolve(name: &str, dark: bool) -> (Theme, bool) {
    let id = alias(name);
    if id == "default" {
        return (default_theme(dark), true);
    }
    match all(dark).into_iter().find(|t| t.id == id) {
        Some(t) => (t, true),
        None => (default_theme(dark), false),
    }
}

impl Theme {
    /// Whether the rest of the UI should use the dark libadwaita style.
    pub fn is_dark(&self) -> bool {
        self.background.average().luminance() < 0.4
    }

    /// True when the terminal must be transparent so a gradient/image behind it shows.
    pub fn layered_background(&self) -> bool {
        matches!(self.background, Paint::Gradient(..)) || self.background_image.is_some()
    }

    /// Panels (header, sidebar, status bar) shade.
    pub fn surface(&self) -> Rgb {
        let bg = self.background.average();
        if self.details_lighter {
            bg.mix(WHITE, 0.07)
        } else {
            bg.mix(BLACK, 0.22)
        }
    }

    /// GTK CSS that themes the whole window. Empty for the native theme.
    pub fn css(&self) -> String {
        if self.native {
            return String::new();
        }
        let fg = self.foreground;
        let accent = self.accent.first();
        let accent_fg = if accent.luminance() > 0.45 { BLACK } else { WHITE };
        let surface = self.surface();
        let raised = if self.is_dark() {
            surface.mix(WHITE, 0.06)
        } else {
            surface.mix(BLACK, 0.04)
        };
        let bg_avg = self.background.average();
        // Chip colours from the palette: bright variants read better on dark themes.
        let chip = if self.is_dark() { self.bright } else { self.normal };
        let mut bg_css = self.background.css("bottom");
        if let Some(img) = &self.background_image {
            let veil = bg_avg.rgba(1.0 - img.opacity as f64 / 100.0);
            let url = format!("file://{}", img.path.display()).replace('"', "%22");
            bg_css = format!(
                "linear-gradient({veil}, {veil}), url(\"{url}\") center / cover no-repeat, {}",
                bg_avg.hex()
            );
        }
        let see_through = if self.layered_background() {
            ".cmux-root vte-terminal, .cmux-root scrolledwindow, .cmux-root overlay, .cmux-root stack, .cmux-root paned { background: transparent; }\n"
        } else {
            ""
        };
        see_through.to_string()
            + &format!(
                r#"/* generated from theme "{name}" */
@define-color accent_bg_color {a};
@define-color accent_color {a};
@define-color accent_fg_color {afg};
@define-color window_bg_color {bg};
@define-color window_fg_color {fg};
@define-color view_bg_color {bg};
@define-color view_fg_color {fg};
@define-color headerbar_bg_color {surf};
@define-color headerbar_fg_color {fg};
@define-color headerbar_backdrop_color {surf};
@define-color sidebar_bg_color {surf};
@define-color sidebar_fg_color {fg};
@define-color sidebar_backdrop_color {surf};
@define-color popover_bg_color {raised};
@define-color popover_fg_color {fg};
@define-color dialog_bg_color {raised};
@define-color dialog_fg_color {fg};
@define-color card_bg_color {card};
@define-color card_fg_color {fg};
.cmux-root {{ background: {bgcss}; color: {fg}; }}
.cmux-root headerbar {{ background: {surf_t}; box-shadow: none; }}
.cmux-root .sidebar-pane {{ background: {surf_t}; }}
.cmux-root .statusbar {{ background: {surf_t}; color: {fg_dim}; }}
.cmux-root headerbar, .cmux-root .statusbar {{ border-color: {line}; }}
.cmux-root .sidebar-pane {{ border-right: 1px solid {line}; }}
.ws-list > row:selected {{ background: {sel}; }}
.badge {{ background: {accent_css}; color: {afg}; }}
.pane.focused {{ border-color: {focus}; }}
.pane.attention {{ border-color: {a}; box-shadow: 0 0 0 1px {a}, 0 0 10px {glow}; }}
.ws-agent, .ws-attention, .notif-unread .ws-name {{ color: {a}; }}
.chip-env {{ color: {chip_env}; }}
.chip-dir {{ color: {chip_dir}; }}
.chip-git {{ color: {chip_git}; }}
.ghost {{ color: {ghost}; }}
"#,
                name = self.name,
                a = accent.hex(),
                afg = accent_fg.hex(),
                bg = bg_avg.hex(),
                fg = fg.hex(),
                surf = surface.hex(),
                raised = raised.hex(),
                card = fg.rgba(0.06),
                bgcss = bg_css,
                surf_t = surface.rgba(if self.layered_background() { 0.0 } else { 1.0 }),
                fg_dim = fg.rgba(0.7),
                sel = accent.rgba(0.22),
                line = fg.rgba(0.08),
                accent_css = self.accent.css("right"),
                focus = accent.rgba(0.55),
                glow = accent.rgba(0.6),
                chip_env = chip[3].hex(),
                chip_dir = chip[4].hex(),
                chip_git = chip[2].hex(),
                ghost = fg.rgba(0.4),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_theme_parses() {
        assert_eq!(builtins().len(), crate::theme_builtin::BUILTIN.len());
        for t in builtins() {
            assert!(!t.name.is_empty());
            assert!(!t.css().is_empty());
        }
    }

    #[test]
    fn parses_gradients_and_colors() {
        let y = "background:\n  top: \"#002633\"\n  bottom: \"#000000\"\naccent:\n  left: '#007972'\n  right: '#7b008f'\nforeground: \"#fff\"\ndetails: lighter\nterminal_colors:\n  normal:\n    red: \"#ff8272\"\n";
        let t = parse_warp("cyber_wave", y, None).unwrap();
        assert_eq!(t.name, "Cyber Wave");
        assert_eq!(t.background, Paint::Gradient(Rgb(0, 0x26, 0x33), Rgb(0, 0, 0)));
        assert_eq!(t.accent, Paint::Gradient(Rgb(0, 0x79, 0x72), Rgb(0x7b, 0, 0x8f)));
        assert_eq!(t.foreground, Rgb(255, 255, 255));
        assert_eq!(t.normal[1], Rgb(0xff, 0x82, 0x72));
        assert_eq!(t.bright[1], t.normal[1], "missing bright falls back to normal");
        assert!(t.details_lighter && t.layered_background() && t.is_dark());
        assert!(t.css().contains("linear-gradient(to bottom, #002633, #000000)"));
    }

    #[test]
    fn rejects_broken_theme_files() {
        assert!(parse_warp("x", "foreground: '#fff'", None).is_err());
        assert!(parse_warp("x", "background: 'nothex'\nforeground: '#fff'", None).is_err());
        assert!(parse_warp("x", "[[[", None).is_err());
    }

    #[test]
    fn user_dir_themes_and_background_image() {
        let dir = std::env::temp_dir().join(format!("cmux-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bg.jpg"), b"x").unwrap();
        std::fs::write(
            dir.join("mine.yaml"),
            "name: My Theme\nbackground: '#101010'\nforeground: '#eeeeee'\naccent: '#ff0000'\nbackground_image:\n  path: bg.jpg\n  opacity: 40\n",
        )
        .unwrap();
        std::fs::write(dir.join("broken.yml"), "nope: [").unwrap();
        let (themes, errors) = load_dir(&dir);
        assert_eq!(themes.len(), 1);
        assert_eq!(errors.len(), 1);
        let t = &themes[0];
        assert_eq!(t.name, "My Theme");
        assert_eq!(t.background_image.as_ref().unwrap().opacity, 40);
        assert!(t.css().contains("bg.jpg"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolve_aliases_and_unknown() {
        assert_eq!(resolve("solarized", true).0.id, "solarized_dark");
        assert_eq!(resolve("dracula", true).0.id, "dracula");
        let (t, found) = resolve("does_not_exist", true);
        assert!(t.native && !found);
        assert!(default_theme(true).css().is_empty());
    }

    #[test]
    fn color_math() {
        assert_eq!(Rgb::parse("#abc"), Some(Rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(Rgb(0, 0, 0).mix(Rgb(255, 255, 255), 0.5), Rgb(128, 128, 128));
        assert!(Rgb(255, 255, 255).luminance() > 0.99);
    }
}
