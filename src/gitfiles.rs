//! Per-file git status and diffs for the file explorer.
//!
//! `git` runs only when asked (explorer opened or refreshed, a file saved, an
//! agent finished its turn, the window regained focus): there is no polling.
//! Both commands use `GIT_OPTIONAL_LOCKS=0` so they never contend with the
//! user's own git commands. The parsers are pure and tested on every OS.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What changed in a file, as one letter in the explorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
}

impl Mark {
    pub fn letter(self) -> &'static str {
        match self {
            Mark::Modified => "M",
            Mark::Added => "A",
            Mark::Deleted => "D",
            Mark::Renamed => "R",
            Mark::Untracked => "U",
            Mark::Conflict => "!",
        }
    }

    /// CSS class for the letter's colour (see style.css).
    pub fn css_class(self) -> &'static str {
        match self {
            Mark::Modified | Mark::Renamed => "git-modified",
            Mark::Added | Mark::Untracked => "git-added",
            Mark::Deleted | Mark::Conflict => "git-deleted",
        }
    }

    fn from_xy(x: u8, y: u8) -> Option<Self> {
        Some(match (x, y) {
            (b'!', b'!') => return None,
            (b'?', b'?') => Mark::Untracked,
            (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => Mark::Conflict,
            (b'R' | b'C', _) | (_, b'R' | b'C') => Mark::Renamed,
            (b'A', _) => Mark::Added,
            (b'D', _) | (_, b'D') => Mark::Deleted,
            _ => Mark::Modified,
        })
    }
}

/// Git status of every changed file under one repository.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Status {
    /// Absolute paths. An untracked folder is listed once, as the folder.
    files: HashMap<PathBuf, Mark>,
}

impl Status {
    /// The mark of `path`: its own, or that of an untracked folder holding it.
    pub fn file(&self, path: &Path) -> Option<Mark> {
        if let Some(m) = self.files.get(path) {
            return Some(*m);
        }
        path.ancestors()
            .skip(1)
            .find_map(|a| self.files.get(a).filter(|m| **m == Mark::Untracked).copied())
    }

    /// A folder contains a change somewhere below it.
    pub fn folder_changed(&self, dir: &Path) -> bool {
        self.files.keys().any(|p| p != dir && p.starts_with(dir))
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The same status for paths seen through `to` instead of `from` (the
    /// explorer root as the user opened it, and its canonical path, which is
    /// what git reports). Files outside `from` are dropped.
    pub fn rebase(self, from: &Path, to: &Path) -> Status {
        let files = self
            .files
            .into_iter()
            .filter_map(|(p, m)| Some((to.join(p.strip_prefix(from).ok()?), m)))
            .collect();
        Status { files }
    }
}

/// Every class `Mark::css_class` returns, to clear a reused label.
pub const CSS_CLASSES: [&str; 3] = ["git-modified", "git-added", "git-deleted"];

/// Parse `git status --porcelain=v1 -z` (paths relative to `top`, the
/// repository root). Renames and copies carry the old path as an extra entry.
pub fn parse_porcelain_z(top: &Path, out: &[u8]) -> Status {
    let mut files = HashMap::new();
    let mut fields = out.split(|b| *b == 0);
    while let Some(entry) = fields.next() {
        if entry.len() < 4 || entry[2] != b' ' {
            continue;
        }
        let (x, y) = (entry[0], entry[1]);
        if matches!(x, b'R' | b'C') {
            fields.next(); // the old path
        }
        let rel = String::from_utf8_lossy(&entry[3..]);
        let rel = rel.trim_end_matches('/');
        if rel.is_empty() {
            continue;
        }
        if let Some(mark) = Mark::from_xy(x, y) {
            files.insert(top.join(rel), mark);
        }
    }
    Status { files }
}

fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    c
}

/// The repository root holding `dir`, if any. Blocking.
pub fn toplevel(dir: &Path) -> Option<PathBuf> {
    let out = git(dir).args(["rev-parse", "--show-toplevel"]).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim_end_matches(['\n', '\r']);
    (out.status.success() && !s.is_empty()).then(|| PathBuf::from(s))
}

/// Status of the repository holding `dir`; empty outside a repository. Blocking.
pub fn status(dir: &Path) -> Status {
    let Some(top) = toplevel(dir) else {
        return Status::default();
    };
    match git(&top)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=normal"])
        .output()
    {
        Ok(out) if out.status.success() => parse_porcelain_z(&top, &out.stdout),
        _ => Status::default(),
    }
}

/// The diff of `path` against the last commit, staged and unstaged together.
/// A new repository without commits diffs against the index. Untracked files
/// show as entirely added. Output is capped at `max_bytes`. Blocking.
pub fn diff(path: &Path, untracked: bool, max_bytes: usize) -> Result<String, String> {
    let dir = path.parent().ok_or("This file has no folder.")?;
    let text = if untracked {
        let body = std::fs::read_to_string(path).map_err(|e| format!("Couldn't read file: {e}"))?;
        let mut s = String::from("@@ new file @@\n");
        for line in body.lines() {
            s.push('+');
            s.push_str(line);
            s.push('\n');
        }
        s
    } else {
        let run = |base: &[&str]| {
            git(dir)
                .args(base)
                .args(["--no-color", "--no-ext-diff", "-U3", "--"])
                .arg(path)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        };
        run(&["diff", "HEAD"])
            .or_else(|| run(&["diff"]))
            .ok_or("git could not diff this file.")?
    };
    if text.len() > max_bytes {
        let mut end = max_bytes;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        return Ok(format!("{}\n@@ diff truncated at {} KiB @@\n", &text[..end], max_bytes / 1024));
    }
    Ok(text)
}

/// One line of a unified diff, as the diff view draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLine {
    /// `diff --git`, `index`, `---`, `+++` and mode lines: not shown.
    Meta,
    /// `@@ -1,4 +1,5 @@`: a dim separator.
    Hunk,
    Added,
    Removed,
    Context,
}

/// Classify every line of a unified diff. Header lines (`diff --git`,
/// `index`, `---`, `+++`, modes) only count as such before a file's first
/// hunk, so a removed line that reads `-- comment` stays a removed line.
pub fn classify(diff: &str) -> Vec<(DiffLine, &str)> {
    let mut in_header = true;
    diff.lines()
        .map(|line| {
            let kind = if line.starts_with("diff --git ") {
                in_header = true;
                DiffLine::Meta
            } else if line.starts_with("@@") {
                in_header = false;
                DiffLine::Hunk
            } else if in_header || line.starts_with('\\') {
                DiffLine::Meta
            } else if line.starts_with('+') {
                DiffLine::Added
            } else if line.starts_with('-') {
                DiffLine::Removed
            } else {
                DiffLine::Context
            };
            (kind, line)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_marks_each_kind_and_skips_rename_sources() {
        let top = Path::new("/repo");
        let out = b" M src/a.rs\0A  new.rs\0 D gone.rs\0R  to.rs\0from.rs\0?? scratch/\0?? notes.md\0UU both.rs\0!! ignored\0MM twice.rs\0";
        let s = parse_porcelain_z(top, out);
        assert_eq!(s.file(Path::new("/repo/src/a.rs")), Some(Mark::Modified));
        assert_eq!(s.file(Path::new("/repo/new.rs")), Some(Mark::Added));
        assert_eq!(s.file(Path::new("/repo/gone.rs")), Some(Mark::Deleted));
        assert_eq!(s.file(Path::new("/repo/to.rs")), Some(Mark::Renamed));
        assert_eq!(s.file(Path::new("/repo/from.rs")), None, "rename source is not a file of its own");
        assert_eq!(s.file(Path::new("/repo/notes.md")), Some(Mark::Untracked));
        assert_eq!(s.file(Path::new("/repo/both.rs")), Some(Mark::Conflict));
        assert_eq!(s.file(Path::new("/repo/ignored")), None);
        assert_eq!(s.file(Path::new("/repo/twice.rs")), Some(Mark::Modified));
    }

    #[test]
    fn untracked_folder_marks_what_is_inside_and_folders_know_about_changes() {
        let s = parse_porcelain_z(Path::new("/repo"), b"?? scratch/\0 M src/ui/x.rs\0");
        assert_eq!(s.file(Path::new("/repo/scratch")), Some(Mark::Untracked));
        assert_eq!(s.file(Path::new("/repo/scratch/deep/a.txt")), Some(Mark::Untracked));
        assert!(s.folder_changed(Path::new("/repo/src")));
        assert!(s.folder_changed(Path::new("/repo/src/ui")));
        assert!(!s.folder_changed(Path::new("/repo/docs")));
        assert!(!s.folder_changed(Path::new("/repo/src/ui/x.rs")), "a file is not its own folder");
    }

    #[test]
    fn rebase_maps_canonical_paths_to_the_opened_root() {
        let s = parse_porcelain_z(Path::new("/real/repo"), b" M app/x.rs\0 M other/y.rs\0");
        let s = s.rebase(Path::new("/real/repo/app"), Path::new("/home/me/link"));
        assert_eq!(s.file(Path::new("/home/me/link/x.rs")), Some(Mark::Modified));
        assert_eq!(s.file(Path::new("/real/repo/other/y.rs")), None, "outside the opened root");
        assert!(!s.folder_changed(Path::new("/home/me/other")));
    }

    #[test]
    fn malformed_and_empty_output_is_ignored() {
        assert!(parse_porcelain_z(Path::new("/r"), b"").is_empty());
        assert!(parse_porcelain_z(Path::new("/r"), b"xx\0 M\0garbage").is_empty());
    }

    #[test]
    fn letters_and_classes_are_stable() {
        assert_eq!(Mark::Modified.letter(), "M");
        assert_eq!(Mark::Untracked.letter(), "U");
        assert_eq!(Mark::Added.css_class(), "git-added");
        assert_eq!(Mark::Conflict.css_class(), "git-deleted");
    }

    #[test]
    fn diff_lines_are_classified_with_headers_only_before_hunks() {
        let diff = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@ fn main\n unchanged\n--- old sql comment\n+++ new\n-let x = 0;\n+let x = 1;\n\\ No newline at end of file\ndiff --git a/y b/y\nnew file mode 100644\n--- /dev/null\n+++ b/y\n@@ -0,0 +1 @@\n+y\n";
        let kinds: Vec<DiffLine> = classify(diff).into_iter().map(|(k, _)| k).collect();
        use DiffLine::*;
        assert_eq!(
            kinds,
            vec![Meta, Meta, Meta, Meta, Hunk, Context, Removed, Added, Removed, Added, Meta, Meta, Meta, Meta, Meta, Hunk, Added]
        );
        assert_eq!(classify("@@ new file @@\n+a\n"), vec![(Hunk, "@@ new file @@"), (Added, "+a")]);
        assert!(classify("").is_empty());
    }

    #[test]
    fn diff_of_untracked_file_is_all_added_and_large_diffs_are_cut() {
        let dir = std::env::temp_dir().join(format!("oxishell-gitfiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("new.txt");
        std::fs::write(&f, "one\ntwo\n").unwrap();
        let d = diff(&f, true, 1 << 20).unwrap();
        assert_eq!(d, "@@ new file @@\n+one\n+two\n");
        let cut = diff(&f, true, 8).unwrap();
        assert!(cut.starts_with("@@ new f") && cut.contains("truncated"), "{cut:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
