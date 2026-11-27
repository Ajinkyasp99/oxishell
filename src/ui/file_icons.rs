//! File-type icons for the explorer: an Adwaita symbolic icon plus a colour
//! class per kind of file. The icons come with GTK's icon theme (bundled in
//! the macOS app too), so this adds nothing to the binary; style.css colours
//! them through the `ft-*` classes.

use std::path::Path;

/// (icon name, CSS colour class) for an explorer entry.
pub fn for_path(path: &Path, folder: bool) -> (&'static str, &'static str) {
    if folder {
        return ("folder-symbolic", "ft-folder");
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    const DOC: &str = "x-office-document-symbolic";
    const TEXT: &str = "text-x-generic-symbolic";
    match (lower.as_str(), ext) {
        (_, "lock") | ("package-lock.json" | "pnpm-lock.yaml", _) => ("changes-prevent-symbolic", "ft-lock"),
        (".gitignore" | ".gitattributes" | ".dockerignore" | ".editorconfig" | ".env", _) => ("emblem-system-symbolic", "ft-dot"),
        ("dockerfile" | "makefile" | "justfile", _) => ("emblem-system-symbolic", "ft-config"),
        (_, "rs") => (TEXT, "ft-rust"),
        (_, "ts" | "tsx" | "mts" | "cts") => (TEXT, "ft-ts"),
        (_, "js" | "jsx" | "mjs" | "cjs") => (TEXT, "ft-js"),
        (_, "py" | "pyi") => (TEXT, "ft-py"),
        (_, "go") => (TEXT, "ft-go"),
        (_, "c" | "h" | "cc" | "cpp" | "hpp" | "swift" | "java" | "kt" | "rb" | "php" | "cs" | "zig") => (TEXT, "ft-code"),
        (_, "html" | "css" | "scss" | "vue" | "svelte") => (TEXT, "ft-web"),
        (_, "sh" | "bash" | "zsh" | "fish" | "ps1") => ("utilities-terminal-symbolic", "ft-shell"),
        (_, "json" | "toml" | "yaml" | "yml" | "ini" | "conf" | "cfg" | "xml" | "prisma" | "sql") => {
            ("emblem-system-symbolic", "ft-config")
        }
        (_, "md" | "mdx" | "rst" | "txt" | "adoc") => (DOC, "ft-doc"),
        (_, "csv" | "tsv" | "xlsx" | "ods") => ("x-office-spreadsheet-symbolic", "ft-data"),
        (_, "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp" | "avif") => ("image-x-generic-symbolic", "ft-image"),
        (_, "mp3" | "wav" | "ogg" | "flac") => ("audio-x-generic-symbolic", "ft-media"),
        (_, "mp4" | "mov" | "webm" | "mkv") => ("video-x-generic-symbolic", "ft-media"),
        (_, "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" | "7z" | "deb" | "rpm" | "dmg") => ("package-x-generic-symbolic", "ft-archive"),
        _ if name.starts_with('.') => ("emblem-system-symbolic", "ft-dot"),
        _ => (TEXT, "ft-plain"),
    }
}

/// Every colour class `for_path` can return, to clear the previous one when a
/// list row is reused for another file.
pub const CLASSES: [&str; 18] = [
    "ft-folder",
    "ft-lock",
    "ft-dot",
    "ft-config",
    "ft-rust",
    "ft-ts",
    "ft-js",
    "ft-py",
    "ft-go",
    "ft-code",
    "ft-web",
    "ft-shell",
    "ft-doc",
    "ft-data",
    "ft-image",
    "ft-media",
    "ft-archive",
    "ft-plain",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str) -> (&'static str, &'static str) {
        for_path(Path::new(name), false)
    }

    #[test]
    fn common_files_get_their_kind() {
        assert_eq!(f("src/main.rs").1, "ft-rust");
        assert_eq!(f("App.TSX").1, "ft-ts");
        assert_eq!(f("index.mjs").1, "ft-js");
        assert_eq!(f("README.md"), ("x-office-document-symbolic", "ft-doc"));
        assert_eq!(f("Cargo.toml").1, "ft-config");
        assert_eq!(f("Cargo.lock").1, "ft-lock");
        assert_eq!(f("pnpm-lock.yaml").1, "ft-lock");
        assert_eq!(f("schema.prisma").1, "ft-config");
        assert_eq!(f("build.sh").0, "utilities-terminal-symbolic");
        assert_eq!(f("logo.svg").1, "ft-image");
        assert_eq!(f(".gitignore").1, "ft-dot");
        assert_eq!(f(".envrc").1, "ft-dot");
        assert_eq!(f("Dockerfile").1, "ft-config");
        assert_eq!(f("LICENSE"), ("text-x-generic-symbolic", "ft-plain"));
    }

    #[test]
    fn folders_use_the_folder_icon_whatever_their_name() {
        assert_eq!(for_path(Path::new("src.rs"), true), ("folder-symbolic", "ft-folder"));
    }

    #[test]
    fn every_returned_class_is_listed_for_clearing() {
        let names = [
            "a.rs", "a.ts", "a.js", "a.py", "a.go", "a.c", "a.css", "a.sh", "a.json", "a.md", "a.csv", "a.png", "a.mp3", "a.mp4", "a.zip",
            "a.lock", ".x", "Makefile", "plain",
        ];
        for n in names {
            assert!(CLASSES.contains(&f(n).1), "{n}");
        }
        assert!(CLASSES.contains(&for_path(Path::new("d"), true).1));
    }
}
