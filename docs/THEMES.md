# Themes

oxishell uses the [Warp theme format](https://github.com/warpdotdev/themes). A theme colours the
**whole window**: background (solid, gradient or image), accent (badges, focus rings, selection), panels,
dialogs and the terminal's 16 colours.

## Choosing a theme

- **Settings → Appearance → Theme**: click a preview card. It applies instantly and is saved.
- **Command palette** (<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd>): type `theme` and pick one.
- **Config file**: `theme = "tokyo_night"` under `[appearance]` (the id is the file name without `.yaml`).

`default` keeps the native GNOME look and follows the system light/dark setting (or the *Style* option).
Every other theme sets light or dark itself, based on its background.

## Built-in themes

41 themes bundled from `warpdotdev/themes` (Apache-2.0):

| | | | |
|---|---|---|---|
| Warp Dark | Warp Light | Cyber Wave* | Willow Dream* |
| Dark City | Fancy Dracula* | Jellyfish | Koi |
| Leafy | Marble | Pink City | Red Rock |
| Snowy | Paper Botanical | Paper Botanical Dark | Dracula |
| Gruvbox Dark | Gruvbox Light | Solarized Dark | Solarized Light |
| Catppuccin | Nord | Tokyo Night | Tokyo Night Storm |
| One Dark | GitHub Dark | GitHub Light | Monokai Pro |
| Night Owl | Synthwave 84 | Ayu Dark | Ayu Light |
| Everforest Hard | Kanagawa | Material Theme | Cobalt 2 |
| Outrun | Shades of Purple | Poimandres | Vitesse Dark |
| Palenight | | | |

\* gradient themes. Warp's background images are not bundled; image themes use their colours.

## Adding your own

1. Settings → Appearance → **Open Themes Folder** (or create `~/.config/oxishell/themes/`).
2. Drop in any `.yaml` theme, for example from [warpdotdev/themes](https://github.com/warpdotdev/themes).
3. Reopen Settings or the command palette: the theme is listed. A file with the same name as a built-in
   theme replaces it.

### Format

```yaml
name: My Theme               # optional; defaults to the file name in Title Case
background: "#1e1e2e"        # or a gradient:
# background:
#   top: "#002633"
#   bottom: "#000000"
accent: "#89b4fa"            # or a gradient with left:/right:
foreground: "#cdd6f4"
cursor: "#f5e0dc"            # optional
details: darker              # panels darker (or `lighter`) than the background
background_image:            # optional
  path: wallpaper.jpg        # relative to the themes folder, or absolute
  opacity: 30                # 0–100, how strongly the image shows
terminal_colors:
  normal:
    black: "#45475a"
    red: "#f38ba8"
    green: "#a6e3a1"
    yellow: "#f9e2af"
    blue: "#89b4fa"
    magenta: "#f5c2e7"
    cyan: "#94e2d5"
    white: "#bac2de"
  bright:                    # optional; falls back to normal
    black: "#585b70"
    # ...
```

Invalid theme files are listed with the error in Settings instead of being silently ignored. A missing
or unknown theme falls back to `default` with a notice.

## Credits

Theme files © Warp and the original authors, Apache License 2.0. See
[data/themes/NOTICE.md](../data/themes/NOTICE.md) and
[data/themes/LICENSE-warp-themes](../data/themes/LICENSE-warp-themes).
