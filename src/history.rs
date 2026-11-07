//! Command history for suggestions: reads bash, zsh and fish history files,
//! newest first, de-duplicated; plus commands typed in cmux-linux this session.

use std::path::{Path, PathBuf};

pub const MAX_ENTRIES: usize = 20_000;

#[derive(Debug, Default, Clone)]
pub struct History {
    /// Newest first, unique.
    entries: Vec<String>,
}

fn clean(cmd: &str) -> Option<String> {
    let c = cmd.trim_end_matches(['\r', '\n']).trim();
    if c.is_empty() || c.len() > 2000 || c.chars().any(|ch| ch.is_control() && ch != '\t') {
        return None;
    }
    Some(c.to_string())
}

/// bash: one command per line; `#<timestamp>` lines (HISTTIMEFORMAT) are skipped.
pub fn parse_bash(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !(l.starts_with('#') && l[1..].chars().all(|c| c.is_ascii_digit()) && l.len() > 1))
        .filter_map(clean)
        .collect()
}

/// zsh: plain lines or extended `: <time>:<dur>;<cmd>`; trailing `\` joins lines.
pub fn parse_zsh(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        let body = if cur.is_empty() && line.starts_with(": ") {
            line.split_once(';').map(|(_, c)| c).unwrap_or("")
        } else {
            line
        };
        if let Some(stripped) = body.strip_suffix('\\') {
            cur.push_str(stripped);
            cur.push('\n');
            continue;
        }
        cur.push_str(body);
        // Multi-line commands are not useful as single-line suggestions.
        if !cur.contains('\n') {
            if let Some(c) = clean(&cur) {
                out.push(c);
            }
        }
        cur.clear();
    }
    out
}

/// fish: YAML-like `- cmd: <command>` entries with `\\` and `\n` escapes.
pub fn parse_fish(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("- cmd: "))
        .filter(|c| !c.contains("\\n"))
        .filter_map(|c| clean(&c.replace("\\\\", "\\")))
        .collect()
}

impl History {
    /// Build from oldest→newest command lists (later lists are newer).
    pub fn from_oldest(lists: Vec<Vec<String>>) -> History {
        let mut h = History::default();
        for list in lists {
            for c in list {
                h.push(&c);
            }
        }
        h
    }

    /// Load the user's shell history files.
    pub fn load(home: &Path) -> History {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        History::load_from(home, var("HISTFILE"), var("XDG_DATA_HOME"))
    }

    pub fn load_from(home: &Path, histfile: Option<PathBuf>, data_home: Option<PathBuf>) -> History {
        let read = |p: PathBuf| {
            std::fs::read(&p)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default()
        };
        let bash = histfile.unwrap_or_else(|| home.join(".bash_history"));
        let zsh = home.join(".zsh_history");
        let fish = data_home.unwrap_or_else(|| home.join(".local/share")).join("fish/fish_history");
        History::from_oldest(vec![parse_fish(&read(fish)), parse_zsh(&read(zsh)), parse_bash(&read(bash))])
    }

    /// Record a command as the newest entry.
    pub fn push(&mut self, cmd: &str) {
        let Some(c) = clean(cmd) else { return };
        self.entries.retain(|e| *e != c);
        self.entries.insert(0, c);
        self.entries.truncate(MAX_ENTRIES);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Newest command that starts with `prefix` and is longer than it.
    pub fn suggest(&self, prefix: &str) -> Option<&str> {
        if prefix.trim().is_empty() {
            return None;
        }
        self.entries
            .iter()
            .find(|e| e.len() > prefix.len() && e.starts_with(prefix))
            .map(String::as_str)
    }

    /// Search for the history popup: prefix matches, then substring, then
    /// fuzzy (all words present), each group newest first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&str> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self.entries.iter().take(limit).map(String::as_str).collect();
        }
        let words: Vec<&str> = q.split_whitespace().collect();
        let (mut pre, mut sub, mut fuzzy) = (Vec::new(), Vec::new(), Vec::new());
        for e in &self.entries {
            let l = e.to_lowercase();
            if l.starts_with(&q) {
                pre.push(e.as_str());
            } else if l.contains(&q) {
                sub.push(e.as_str());
            } else if words.iter().all(|w| l.contains(w)) {
                fuzzy.push(e.as_str());
            }
            if pre.len() >= limit {
                break;
            }
        }
        pre.into_iter().chain(sub).chain(fuzzy).take(limit).collect()
    }
}

/// Follows what the user types at the prompt (from the terminal's input
/// stream). Anything it cannot follow precisely (Tab, arrows, pastes with
/// control characters, ...) invalidates it until the next line.
#[derive(Debug, Clone)]
pub struct LineTracker {
    pub line: String,
    pub valid: bool,
}

impl Default for LineTracker {
    fn default() -> Self {
        LineTracker {
            line: String::new(),
            valid: true,
        }
    }
}

pub enum Event {
    /// A line was submitted with Enter (if it was tracked reliably).
    Submitted(Option<String>),
    Changed,
}

impl LineTracker {
    pub fn feed(&mut self, input: &str) -> Event {
        let mut submitted = None;
        let mut did_submit = false;
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' | '\n' => {
                    submitted = self.valid.then(|| self.line.clone());
                    did_submit = true;
                    self.reset();
                }
                '\u{3}' | '\u{15}' => self.reset(), // Ctrl+C, Ctrl+U: line cleared
                '\u{7f}' | '\u{8}' => {
                    self.line.pop();
                }
                '\u{1b}' => {
                    // Escape sequence (arrows, Home, Alt+...): cursor may have moved.
                    self.valid = false;
                    while chars.peek().is_some_and(|n| !n.is_ascii_alphabetic() && *n != '~') {
                        chars.next();
                    }
                    chars.next();
                }
                c if c.is_control() => self.valid = false, // Tab, Ctrl+W, Ctrl+A, ...
                c => self.line.push(c),
            }
        }
        if did_submit {
            Event::Submitted(submitted)
        } else {
            Event::Changed
        }
    }

    pub fn reset(&mut self) {
        self.line.clear();
        self.valid = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_history_formats() {
        assert_eq!(parse_bash("ls\n#1700000000\ngit status\n\n"), vec!["ls", "git status"]);
        assert_eq!(
            parse_zsh(": 1700000000:0;git push\nls -la\n: 1:0;echo a\\\necho b\n"),
            vec!["git push", "ls -la"]
        );
        assert_eq!(
            parse_fish("- cmd: cargo build\n  when: 1\n- cmd: echo a\\nb\n"),
            vec!["cargo build"]
        );
    }

    #[test]
    fn newest_first_unique_and_suggest() {
        let mut h = History::from_oldest(vec![vec!["git status".into(), "git commit -m x".into(), "git status".into()]]);
        assert_eq!(h.search("", 10), vec!["git status", "git commit -m x"]);
        assert_eq!(h.suggest("git c"), Some("git commit -m x"));
        assert_eq!(h.suggest("git status"), None, "no suggestion when the line already equals it");
        assert_eq!(h.suggest(""), None);
        h.push("git checkout main");
        assert_eq!(h.suggest("git c"), Some("git checkout main"));
    }

    #[test]
    fn search_ranks_prefix_then_substring_then_words() {
        let h = History::from_oldest(vec![vec![
            "docker compose up".into(),
            "sudo docker ps".into(),
            "docker ps".into(),
            "ps aux | grep docker".into(),
        ]]);
        assert_eq!(
            h.search("docker", 10),
            vec!["docker ps", "docker compose up", "ps aux | grep docker", "sudo docker ps"]
        );
        assert_eq!(h.search("compose docker", 10), vec!["docker compose up"]);
    }

    #[test]
    fn line_tracker() {
        let mut t = LineTracker::default();
        t.feed("git chk");
        t.feed("\u{7f}\u{7f}");
        assert_eq!(t.line, "git c");
        match t.feed("heckout\r") {
            Event::Submitted(Some(c)) => assert_eq!(c, "git checkout"),
            _ => panic!("expected submit"),
        }
        t.feed("ls\u{1b}[D");
        assert!(!t.valid, "arrow keys invalidate");
        match t.feed("x\r") {
            Event::Submitted(None) => {}
            _ => panic!("untracked line is not recorded"),
        }
        assert!(t.valid && t.line.is_empty());
        t.feed("cd\t");
        assert!(!t.valid, "tab completion invalidates");
        t.feed("\u{15}");
        assert!(t.valid && t.line.is_empty(), "Ctrl+U clears");
    }

    #[test]
    fn loads_real_files() {
        let home = std::env::temp_dir().join(format!("cmux-hist-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".local/share/fish")).unwrap();
        std::fs::write(home.join(".bash_history"), "make test\n").unwrap();
        std::fs::write(home.join(".zsh_history"), ": 1:0;npm run dev\n").unwrap();
        std::fs::write(home.join(".local/share/fish/fish_history"), "- cmd: cargo run\n").unwrap();
        let h = History::load_from(&home, None, None);
        assert_eq!(h.len(), 3);
        assert_eq!(h.suggest("ma"), Some("make test"));
        std::fs::remove_dir_all(&home).unwrap();
    }
}
