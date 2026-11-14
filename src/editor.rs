//! Opening `path:line[:col]` references from terminal output in an editor.
//!
//! Pure helpers (parsing, resolving, building the editor command) live here so
//! they can be unit-tested; the GTK side is in `ui::links`.

use std::path::{Path, PathBuf};

/// A file position printed by a compiler, test runner or agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub path: String,
    pub line: u32,
    pub col: Option<u32>,
}

/// Parse `path:line`, `path:line:col` or Python's `File "path", line N`.
pub fn parse_file_ref(text: &str) -> Option<FileRef> {
    let t = text.trim();
    if t.contains("://") {
        return None;
    }
    if let Some(rest) = t.strip_prefix("File \"") {
        let (path, rest) = rest.split_once('"')?;
        let n = rest.trim_start_matches(',').trim_start().strip_prefix("line")?.trim_start();
        let digits: String = n.chars().take_while(char::is_ascii_digit).collect();
        let line = digits.parse().ok().filter(|l| *l > 0)?;
        return (!path.is_empty()).then(|| FileRef {
            path: path.to_string(),
            line,
            col: None,
        });
    }
    let mut parts = t.split(':');
    let path = parts.next().filter(|p| !p.is_empty())?;
    let line = parts.next()?.parse().ok().filter(|l| *l > 0)?;
    let col = parts.next().and_then(|c| c.parse().ok()).filter(|c| *c > 0);
    Some(FileRef {
        path: path.to_string(),
        line,
        col,
    })
}

/// The existing regular file `path` refers to: relative paths are resolved
/// against `cwd`, `~/` against `home`. None when there is no such file.
pub fn resolve(path: &str, cwd: Option<&Path>, home: &Path) -> Option<PathBuf> {
    let p = if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd?.join(path)
    };
    p.is_file().then_some(p)
}

/// File name of the editor program (`/usr/bin/nvim -u NONE` → `nvim`).
fn program_name(editor: &str) -> &str {
    let prog = editor.split_whitespace().next().unwrap_or("");
    Path::new(prog).file_name().and_then(|n| n.to_str()).unwrap_or(prog)
}

/// Command line that opens `file` at `line`/`col` with `editor` (a program
/// name, optionally followed by arguments, e.g. `code --new-window`).
pub fn editor_argv(editor: &str, file: &Path, line: u32, col: Option<u32>) -> Vec<String> {
    let mut argv: Vec<String> = editor.split_whitespace().map(String::from).collect();
    let f = file.display().to_string();
    let pos = match col {
        Some(c) => format!("{f}:{line}:{c}"),
        None => format!("{f}:{line}"),
    };
    match program_name(editor) {
        "code" | "code-insiders" | "codium" | "vscodium" | "cursor" | "windsurf" => argv.extend(["-g".into(), pos]),
        "zed" | "zeditor" | "subl" | "sublime_text" | "hx" | "helix" | "micro" => argv.push(pos),
        "vim" | "nvim" | "vi" | "gvim" | "kak" | "gedit" => argv.extend([format!("+{line}"), f]),
        "nano" => argv.extend([col.map_or_else(|| format!("+{line}"), |c| format!("+{line},{c}")), f]),
        "emacs" | "emacsclient" => argv.extend([format!("+{line}:{}", col.unwrap_or(1)), f]),
        "kate" => {
            argv.extend(["-l".into(), line.to_string()]);
            if let Some(c) = col {
                argv.extend(["-c".into(), c.to_string()]);
            }
            argv.push(f);
        }
        _ => argv.push(f),
    }
    argv
}

/// True for editors that run inside a terminal (they get their own pane).
pub fn is_terminal_editor(editor: &str) -> bool {
    matches!(
        program_name(editor),
        "vim" | "nvim" | "vi" | "nano" | "micro" | "hx" | "helix" | "kak" | "ne" | "joe" | "mcedit"
    )
}

/// Graphical editors tried, in order, when nothing is configured.
const KNOWN_EDITORS: &[&str] = &["code", "zed", "codium", "subl", "kate", "gnome-text-editor", "gedit"];

/// Which editor to use: the configured one, then `$VISUAL`, `$EDITOR`, then the
/// first known graphical editor found by `on_path`. None = use the desktop's
/// default app for the file (no line number).
pub fn choose_editor(configured: &str, visual: Option<&str>, editor: Option<&str>, on_path: impl Fn(&str) -> bool) -> Option<String> {
    [Some(configured), visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|e| !e.is_empty())
        .map(String::from)
        .or_else(|| KNOWN_EDITORS.iter().find(|e| on_path(e)).map(|e| e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr(path: &str, line: u32, col: Option<u32>) -> Option<FileRef> {
        Some(FileRef {
            path: path.into(),
            line,
            col,
        })
    }

    #[test]
    fn parses_path_line_and_column() {
        assert_eq!(parse_file_ref("src/model.rs:230"), fr("src/model.rs", 230, None));
        assert_eq!(parse_file_ref("src/model.rs:230:9"), fr("src/model.rs", 230, Some(9)));
        assert_eq!(parse_file_ref("./a.rs:3"), fr("./a.rs", 3, None));
        assert_eq!(parse_file_ref("/tmp/a.rs:4:"), fr("/tmp/a.rs", 4, None));
        assert_eq!(parse_file_ref("~/x/b.py:7"), fr("~/x/b.py", 7, None));
    }

    #[test]
    fn extra_numbers_after_the_column_are_ignored() {
        assert_eq!(parse_file_ref("a.rs:12:30:45"), fr("a.rs", 12, Some(30)));
    }

    #[test]
    fn parses_python_tracebacks() {
        assert_eq!(parse_file_ref(r#"File "app/main.py", line 12"#), fr("app/main.py", 12, None));
    }

    #[test]
    fn rejects_text_without_a_usable_line() {
        assert_eq!(parse_file_ref("src/model.rs"), None);
        assert_eq!(parse_file_ref("src/model.rs:0"), None);
        assert_eq!(parse_file_ref("src/model.rs:abc"), None);
        assert_eq!(parse_file_ref(":12"), None);
        assert_eq!(parse_file_ref("https://example.com/a.rs:12"), None);
    }

    #[test]
    fn resolves_only_existing_files() {
        let dir = std::env::temp_dir().join(format!("oxishell-editor-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "").unwrap();
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("h.rs"), "").unwrap();

        assert_eq!(resolve("src/a.rs", Some(&dir), &home), Some(dir.join("src/a.rs")));
        assert_eq!(resolve("./src/a.rs", Some(&dir), &home), Some(dir.join("./src/a.rs")));
        let abs = dir.join("src/a.rs").display().to_string();
        assert_eq!(resolve(&abs, None, &home), Some(dir.join("src/a.rs")));
        assert_eq!(resolve("~/h.rs", None, &home), Some(home.join("h.rs")));
        assert_eq!(resolve("src/missing.rs", Some(&dir), &home), None);
        assert_eq!(resolve("src", Some(&dir), &home), None, "directories are not files");
        assert_eq!(resolve("src/a.rs", None, &home), None, "relative path without a cwd");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn builds_editor_commands_with_line_syntax() {
        let f = Path::new("/p/a.rs");
        assert_eq!(editor_argv("code", f, 12, Some(3)), ["code", "-g", "/p/a.rs:12:3"]);
        assert_eq!(
            editor_argv("code --new-window", f, 12, None),
            ["code", "--new-window", "-g", "/p/a.rs:12"]
        );
        assert_eq!(editor_argv("/usr/bin/codium", f, 5, None), ["/usr/bin/codium", "-g", "/p/a.rs:5"]);
        assert_eq!(editor_argv("zed", f, 12, Some(3)), ["zed", "/p/a.rs:12:3"]);
        assert_eq!(editor_argv("subl", f, 12, None), ["subl", "/p/a.rs:12"]);
        assert_eq!(editor_argv("vim", f, 12, Some(3)), ["vim", "+12", "/p/a.rs"]);
        assert_eq!(editor_argv("nvim", f, 12, None), ["nvim", "+12", "/p/a.rs"]);
        assert_eq!(editor_argv("nano", f, 12, Some(3)), ["nano", "+12,3", "/p/a.rs"]);
        assert_eq!(editor_argv("hx", f, 12, Some(3)), ["hx", "/p/a.rs:12:3"]);
        assert_eq!(editor_argv("kate", f, 12, Some(3)), ["kate", "-l", "12", "-c", "3", "/p/a.rs"]);
        assert_eq!(editor_argv("someeditor", f, 12, None), ["someeditor", "/p/a.rs"]);
    }

    #[test]
    fn knows_which_editors_need_a_terminal() {
        for e in ["vim", "nvim", "vi", "nano", "micro", "hx", "/usr/bin/vim", "nvim -u NONE"] {
            assert!(is_terminal_editor(e), "{e}");
        }
        for e in ["code", "zed", "kate", "gnome-text-editor", "emacs"] {
            assert!(!is_terminal_editor(e), "{e}");
        }
    }

    #[test]
    fn chooses_configured_then_environment_then_installed_editor() {
        let none = |_: &str| false;
        assert_eq!(choose_editor("zed", Some("vim"), Some("nano"), none).as_deref(), Some("zed"));
        assert_eq!(choose_editor("", Some("vim"), Some("nano"), none).as_deref(), Some("vim"));
        assert_eq!(choose_editor("  ", None, Some("nano"), none).as_deref(), Some("nano"));
        assert_eq!(choose_editor("", Some(""), None, |p| p == "zed").as_deref(), Some("zed"));
        assert_eq!(
            choose_editor("", None, None, |p| p == "code" || p == "zed").as_deref(),
            Some("code")
        );
        assert_eq!(choose_editor("", None, None, none), None);
    }
}
