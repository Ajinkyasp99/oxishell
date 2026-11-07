//! Terminal color themes. `default` follows the light/dark UI appearance.

pub struct Theme {
    pub name: &'static str,
    /// (foreground, background) for dark and light UI.
    pub dark: (&'static str, &'static str),
    pub light: (&'static str, &'static str),
    pub palette: [&'static str; 16],
}

const TANGO: [&str; 16] = [
    "#2e3436", "#cc0000", "#4e9a06", "#c4a000", "#3465a4", "#75507b", "#06989a", "#d3d7cf", "#555753", "#ef2929", "#8ae234", "#fce94f",
    "#729fcf", "#ad7fa8", "#34e2e2", "#eeeeec",
];

pub const THEMES: &[Theme] = &[
    Theme {
        name: "default",
        dark: ("#e6e6e6", "#1e1e22"),
        light: ("#202124", "#fbfbfb"),
        palette: TANGO,
    },
    Theme {
        name: "solarized",
        dark: ("#839496", "#002b36"),
        light: ("#657b83", "#fdf6e3"),
        palette: [
            "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5", "#002b36", "#cb4b16", "#586e75",
            "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
        ],
    },
    Theme {
        name: "dracula",
        dark: ("#f8f8f2", "#282a36"),
        light: ("#f8f8f2", "#282a36"),
        palette: [
            "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2", "#6272a4", "#ff6e6e", "#69ff94",
            "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
        ],
    },
    Theme {
        name: "gruvbox",
        dark: ("#ebdbb2", "#282828"),
        light: ("#3c3836", "#fbf1c7"),
        palette: [
            "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984", "#928374", "#fb4934", "#b8bb26",
            "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
        ],
    },
];

pub fn by_name(name: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|t| t.name == name)
}

pub fn names() -> Vec<&'static str> {
    THEMES.iter().map(|t| t.name).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn all_colors_are_hex() {
        for t in super::THEMES {
            for c in t.palette.iter().chain([t.dark.0, t.dark.1, t.light.0, t.light.1].iter()) {
                assert!(
                    c.len() == 7 && c.starts_with('#') && u32::from_str_radix(&c[1..], 16).is_ok(),
                    "{} {c}",
                    t.name
                );
            }
        }
    }
}
