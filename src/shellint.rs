//! Shell integration for context chips.
//!
//! Activating a Python virtualenv or conda environment only changes the
//! shell's own variables, which are not visible from outside (/proc shows the
//! environment the shell *started* with). So the shell reports it: at every
//! prompt it writes the active environment name to a small per-pane file,
//! `$OXISHELL_ENV_FILE`, using only shell builtins (no extra process).
//!
//! bash is set up automatically through `PROMPT_COMMAND` in the pane
//! environment (unless your own config replaces PROMPT_COMMAND). zsh is set up
//! automatically through `ZDOTDIR` (see [`zsh_zdotdir`]). fish needs one line in
//! its config: `oxi shell-integration fish`.
//!
//! The same hook optionally replaces the prompt with a compact `❯` (the chips
//! already show folder, branch and environment), only inside oxishell panes.

use std::path::{Path, PathBuf};

pub fn env_file(pane: crate::model::PaneId) -> PathBuf {
    crate::paths::runtime_dir().join("panes").join(format!("{pane}.env"))
}

/// Where the shell appends every command it runs (`<seq>\t <command>` lines,
/// the format of `fc -l`).
/// The shell knows the command exactly, whatever the user did to build it
/// (Tab, arrows, Ctrl+R, paste); oxishell's own view of the keystrokes cannot.
pub fn hist_file(pane: crate::model::PaneId) -> PathBuf {
    env_file(pane).with_extension("hist")
}

/// Commands in the text of a history log: `<seq>\t<command>` lines. A line
/// without a sequence number continues the previous command (multi-line
/// commands, which are not kept as suggestions).
pub fn parse_hist_log(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines() {
        let head = l.trim_start();
        let digits = head.bytes().take_while(u8::is_ascii_digit).count();
        match head[digits..].strip_prefix('\t') {
            // `fc -l` puts a marker column (space, or `*` for an edited entry) first.
            Some(cmd) if digits > 0 => out.push(cmd.strip_prefix([' ', '*']).unwrap_or(cmd).to_string()),
            _ => {
                if let Some(last) = out.last_mut() {
                    last.push('\n');
                    last.push_str(l);
                }
            }
        }
    }
    out
}

/// The complete lines appended to `path` since `*offset`, as commands. A
/// half-written last line waits for the next call; a file that shrank (the
/// pane was restarted) is read from its start.
pub fn read_new_commands(path: &Path, offset: &mut u64) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        *offset = 0;
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len < *offset {
        *offset = 0;
    }
    if len == *offset || f.seek(SeekFrom::Start(*offset)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.take(len - *offset).read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let Some(end) = buf.iter().rposition(|b| *b == b'\n') else {
        return Vec::new();
    };
    *offset += end as u64 + 1;
    parse_hist_log(&String::from_utf8_lossy(&buf[..=end]))
}

/// bash: run at every prompt (shell builtins only, no extra process).
///
/// It remembers the last exit status, reports the active Python/conda env for
/// the chips (`${var##*/}` keeps the venv's folder name) and, with
/// `OXISHELL_COMPACT_PROMPT=1`, sets the prompt to `❯` (green, or red after a
/// failed command). With `OXISHELL_HIGHLIGHT_CMD=1` as well, the typed command is
/// bold cyan and `PS0` resets the colour before the command's output (bash 4.4+
/// only: without `PS0`, as in macOS's bash 3.2, output would stay cyan). With
/// `OXISHELL_BLOCKS=1` the prompt starts with an empty row, where oxishell draws
/// the command block's separator and header.
///
/// The env file's third line is the last exit status: its modification time
/// tells oxishell when a command finished, and the status how.
pub const BASH_PROMPT_COMMAND: &str = concat!(
    r#"__oxishell_status=$?; "#,
    // Every command bash adds to its history is appended to the pane's log
    // (`fc` and the tests are builtins: no extra process). The first prompt
    // only records the history number, so the previous session's last command
    // is not logged again; Enter on an empty line and lines bash does not keep
    // (HISTCONTROL) leave HISTCMD unchanged and log nothing.
    r#"if [ -n "$OXISHELL_ENV_FILE" ] && [ "${__oxishell_hn-}" != "$HISTCMD" ]; then [ -n "${__oxishell_hn+x}" ] && builtin fc -l -1 >> "${OXISHELL_ENV_FILE%.env}.hist" 2>/dev/null; __oxishell_hn=$HISTCMD; fi; "#,
    r#"[ -n "$OXISHELL_ENV_FILE" ] && builtin printf '%s\n%s\n%s\n' "${VIRTUAL_ENV##*/}" "$CONDA_DEFAULT_ENV" "$__oxishell_status" >| "$OXISHELL_ENV_FILE" 2>/dev/null; "#,
    r#"if [ "$OXISHELL_COMPACT_PROMPT" = 1 ]; then if [ "$__oxishell_status" = 0 ]; then PS1='\[\e[1;32m\]❯\[\e[0m\] '; else PS1='\[\e[1;31m\]❯\[\e[0m\] '; fi; "#,
    r#"if [ "$OXISHELL_BLOCKS" = 1 ]; then PS1='\n'"$PS1"; fi; "#,
    r#"if [ "$OXISHELL_HIGHLIGHT_CMD" = 1 ] && (( BASH_VERSINFO[0] * 100 + BASH_VERSINFO[1] >= 404 )); then PS1="$PS1"'\[\e[1;36m\]'; PS0='\e[0m'; fi; fi"#
);

pub fn snippet(shell: &str) -> Option<String> {
    Some(match shell {
        "bash" => format!(
            "# oxishell context chips (already automatic unless your config sets PROMPT_COMMAND)\nPROMPT_COMMAND='{}'\"${{PROMPT_COMMAND:+; $PROMPT_COMMAND}}\"\n",
            BASH_PROMPT_COMMAND.replace('\'', r"'\''")
        ),
        "zsh" => r#"# oxishell context chips and compact prompt — add to ~/.zshrc
if [[ -n "$OXISHELL_ENV_FILE" ]]; then
  # Every command line zsh runs is appended to the pane's history log.
  _oxishell_preexec() { __oxishell_cmd=$1 }
  _oxishell_precmd() {
    local s=$?
    if [[ -n "$__oxishell_cmd" && "$__oxishell_cmd" != *$'\n'* ]]; then
      builtin print -r -- "$HISTCMD"$'\t '"$__oxishell_cmd" >>|"${OXISHELL_ENV_FILE%.env}.hist" 2>/dev/null
    fi
    __oxishell_cmd=
    builtin printf '%s\n%s\n%s\n' "${VIRTUAL_ENV:t}" "$CONDA_DEFAULT_ENV" "$s" >| "$OXISHELL_ENV_FILE" 2>/dev/null
    [[ "$OXISHELL_COMPACT_PROMPT" == 1 ]] && PROMPT='%(?.%F{green}.%F{red})%B❯%b%f ' && RPROMPT=''
    [[ "$OXISHELL_COMPACT_PROMPT" == 1 && "$OXISHELL_BLOCKS" == 1 ]] && PROMPT=$'\n'"$PROMPT"
    return 0
  }
  autoload -Uz add-zsh-hook && add-zsh-hook precmd _oxishell_precmd && add-zsh-hook preexec _oxishell_preexec
  # Typed command in bold cyan (zsh draws the input line itself).
  [[ "$OXISHELL_COMPACT_PROMPT" == 1 && "$OXISHELL_HIGHLIGHT_CMD" == 1 ]] && zle_highlight=(default:fg=cyan,bold)
fi
"#
        .to_string(),
        "fish" => r#"# oxishell context chips and compact prompt — add to ~/.config/fish/config.fish
if set -q OXISHELL_ENV_FILE
    # Every command fish runs is appended to the pane's history log.
    function _oxishell_postexec --on-event fish_postexec
        set -g __oxishell_cmd $argv[1]
    end
    function _oxishell_report_env --on-event fish_prompt
        set -l s $status
        if set -q __oxishell_cmd; and not string match -q -- "*"\n"*" $__oxishell_cmd
            set -q __oxishell_seq; or set -g __oxishell_seq 0
            set -g __oxishell_seq (math $__oxishell_seq + 1)
            printf '%s\t %s\n' $__oxishell_seq $__oxishell_cmd >> (string replace -r '\.env$' .hist -- $OXISHELL_ENV_FILE) 2>/dev/null
        end
        set -e __oxishell_cmd
        set -l venv ''
        set -q VIRTUAL_ENV; and set venv (path basename $VIRTUAL_ENV)
        printf '%s\n%s\n%s\n' "$venv" "$CONDA_DEFAULT_ENV" $s > $OXISHELL_ENV_FILE 2>/dev/null
    end
    if test "$OXISHELL_COMPACT_PROMPT" = 1
        function fish_prompt
            set -l s $status
            test "$OXISHELL_BLOCKS" = 1; and printf '\n'
            if test $s -eq 0; set_color --bold green; else; set_color --bold red; end
            printf '❯ '
            set_color normal
        end
    end
end
"#
        .to_string(),
        _ => return None,
    })
}

/// `.zshenv` of the private `ZDOTDIR` that zsh panes start with. It puts the
/// user's own `ZDOTDIR` back (saved in `OXISHELL_ORIG_ZDOTDIR`, or none = $HOME)
/// and runs their `.zshenv`, so zsh then reads their `.zprofile`, `.zshrc` and
/// `.zlogin` as usual; interactive shells also get the zsh snippet.
pub fn zshenv() -> String {
    format!(
        r#"# Written by oxishell for its zsh panes; regenerated at every start.
if [[ -n "${{OXISHELL_ORIG_ZDOTDIR+x}}" ]]; then
  ZDOTDIR=$OXISHELL_ORIG_ZDOTDIR
  unset OXISHELL_ORIG_ZDOTDIR
else
  unset ZDOTDIR
fi
if [[ -r "${{ZDOTDIR:-$HOME}}/.zshenv" ]]; then
  builtin source "${{ZDOTDIR:-$HOME}}/.zshenv"
fi
if [[ -o interactive ]]; then
{}fi
"#,
        snippet("zsh").unwrap_or_default()
    )
}

/// The private `ZDOTDIR` for zsh panes, with an up-to-date `.zshenv`.
pub fn zsh_zdotdir() -> std::io::Result<PathBuf> {
    let dir = crate::paths::runtime_dir().join("zsh");
    crate::paths::ensure_private_dir(&dir)?;
    let file = dir.join(".zshenv");
    let text = zshenv();
    if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
        crate::paths::write_atomic(&file, text.as_bytes(), 0o600)?;
    }
    Ok(dir)
}

/// The environment name to show: the virtualenv, else a conda env other than `base`.
pub fn parse_env_file(text: &str) -> Option<String> {
    let mut lines = text.lines().map(str::trim);
    let venv = lines.next().unwrap_or("");
    let conda = lines.next().unwrap_or("");
    let pick = if !venv.is_empty() {
        venv
    } else if !conda.is_empty() && conda != "base" {
        conda
    } else {
        return None;
    };
    let clean: String = pick.chars().filter(|c| !c.is_control()).take(64).collect();
    (!clean.is_empty()).then_some(clean)
}

/// Exit status of the last command (third line of the env file).
pub fn parse_status(text: &str) -> Option<i32> {
    text.lines().nth(2)?.trim().parse().ok()
}

pub fn read_env(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_env_file(&text)
}

pub fn shell_name(shell: &str) -> &str {
    Path::new(shell).file_name().and_then(|f| f.to_str()).unwrap_or(shell)
}

/// What has the keyboard in a pane, for the "new line in prompt" keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foreground<'a> {
    /// A coding agent (Claude Code, Codex, Gemini CLI, OpenCode, ...).
    Agent,
    /// The shell itself, at its prompt (its program name, e.g. "bash").
    Shell(&'a str),
    /// Any other program (vim, a REPL, ...).
    Program,
}

/// Bytes that start a new line in the input without submitting it, or
/// `None` to leave the key alone.
///
/// - Agents: Ctrl+J (line feed), the newline key of Claude Code, Codex,
///   Gemini CLI and OpenCode in every terminal.
/// - bash/zsh: Ctrl+V Ctrl+J (quoted-insert of a line feed) puts a line break
///   into the command being typed; Enter then runs all of it.
/// - fish: Alt+Enter, fish's own "insert newline" binding.
/// - Other programs: they get the plain key, as before.
pub fn newline_input(fg: Foreground) -> Option<&'static [u8]> {
    match fg {
        Foreground::Agent => Some(b"\n"),
        Foreground::Shell("bash" | "zsh") => Some(b"\x16\n"),
        Foreground::Shell("fish") => Some(b"\x1b\r"),
        Foreground::Shell(_) | Foreground::Program => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newline_input_depends_on_what_has_the_keyboard() {
        assert_eq!(newline_input(Foreground::Agent), Some(&b"\n"[..]));
        assert_eq!(newline_input(Foreground::Shell("bash")), Some(&b"\x16\n"[..]));
        assert_eq!(newline_input(Foreground::Shell("zsh")), Some(&b"\x16\n"[..]));
        assert_eq!(newline_input(Foreground::Shell("fish")), Some(&b"\x1b\r"[..]));
        assert_eq!(newline_input(Foreground::Shell("dash")), None);
        assert_eq!(newline_input(Foreground::Program), None);
    }

    /// bash really treats Ctrl+V Ctrl+J as a line break inside one command line.
    #[test]
    fn bash_quoted_newline_runs_both_lines() {
        let out = std::process::Command::new("bash")
            .args(["--norc", "--noprofile", "-c", "eval \"$(printf 'echo one\\necho two')\""])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "one\ntwo\n");
    }

    /// A zsh pane runs the user's own startup files and reports to the env file.
    #[test]
    fn zsh_zdotdir_keeps_user_config_and_reports_status() {
        if std::process::Command::new("zsh").arg("-fc").arg("true").status().is_err() {
            eprintln!("zsh not installed; skipping");
            return;
        }
        let root = std::env::temp_dir().join(format!("oxishell-zdotdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (ours, home) = (root.join("ours"), root.join("home"));
        std::fs::create_dir_all(&ours).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(ours.join(".zshenv"), zshenv()).unwrap();
        std::fs::write(home.join(".zshenv"), "export FROM_ZSHENV=1\n").unwrap();
        std::fs::write(home.join(".zshrc"), "FROM_ZSHRC=1\n").unwrap();
        let file = root.join("p.env");
        let out = std::process::Command::new("zsh")
            .args([
                "-i",
                "-c",
                "(exit 4); for f in $precmd_functions; do $f; done; print -r -- \"$FROM_ZSHENV$FROM_ZSHRC:${ZDOTDIR-unset}\"",
            ])
            .env("HOME", &home)
            .env("ZDOTDIR", &ours)
            .env_remove("OXISHELL_ORIG_ZDOTDIR")
            .env_remove("OXISHELL_COMPACT_PROMPT")
            .env_remove("OXISHELL_BLOCKS")
            .env_remove("OXISHELL_HIGHLIGHT_CMD")
            .env("OXISHELL_ENV_FILE", &file)
            .env("VIRTUAL_ENV", "/x/venv")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(stdout.lines().last(), Some("11:unset"), "{stdout}");
        let report = std::fs::read_to_string(&file).unwrap();
        assert_eq!(parse_env_file(&report), Some("venv".into()));
        assert_eq!(parse_status(&report), Some(4));
        // Every command line zsh ran is logged (even with `setopt noclobber`);
        // an empty Enter runs no preexec and logs nothing.
        let script = "setopt noclobber; for c in 'git status' 'echo a b'; do for f in $preexec_functions; do $f $c; done; for f in $precmd_functions; do $f; done; done; for f in $precmd_functions; do $f; done";
        let ok = std::process::Command::new("zsh")
            .args(["-i", "-c", script])
            .env("HOME", &home)
            .env("ZDOTDIR", &ours)
            .env_remove("OXISHELL_ORIG_ZDOTDIR")
            .env_remove("OXISHELL_COMPACT_PROMPT")
            .env_remove("OXISHELL_BLOCKS")
            .env_remove("OXISHELL_HIGHLIGHT_CMD")
            .env("OXISHELL_ENV_FILE", &file)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let mut off = 0;
        assert_eq!(
            read_new_commands(&file.with_extension("hist"), &mut off),
            vec!["git status", "echo a b"]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn parses_env_names() {
        assert_eq!(parse_env_file("CommonVenv\n\n"), Some("CommonVenv".into()));
        assert_eq!(parse_env_file("\nml\n"), Some("ml".into()));
        assert_eq!(parse_env_file("\nbase\n"), None);
        assert_eq!(parse_env_file("\n\n"), None);
        assert_eq!(parse_env_file(""), None);
        assert_eq!(parse_env_file("a\u{1b}b\n"), Some("ab".into()));
        assert_eq!(parse_status("\n\n1\n"), Some(1));
        assert_eq!(parse_status("venv\n\n0\n"), Some(0));
        assert_eq!(parse_status("\n\n"), None);
    }

    #[test]
    fn bash_hook_reports_activated_venv() {
        let dir = std::env::temp_dir().join(format!("oxishell-shellint-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("p.env");
        // Simulate `source myenv/bin/activate` (sets VIRTUAL_ENV) and one prompt.
        let script = "VIRTUAL_ENV=/home/u/proj/CommonVenv; eval \"$PROMPT_COMMAND\"";
        let ok = std::process::Command::new("bash")
            .args(["--norc", "--noprofile", "-c", script])
            .env("PROMPT_COMMAND", BASH_PROMPT_COMMAND)
            .env("OXISHELL_ENV_FILE", &file)
            .env_remove("CONDA_DEFAULT_ENV")
            .status()
            .unwrap()
            .success();
        assert!(ok);
        assert_eq!(read_env(&file), Some("CommonVenv".into()));
        // The last command's exit status is reported too.
        let ok = std::process::Command::new("bash")
            .args(["--norc", "--noprofile", "-c", "(exit 3); eval \"$PROMPT_COMMAND\""])
            .env("PROMPT_COMMAND", BASH_PROMPT_COMMAND)
            .env("OXISHELL_ENV_FILE", &file)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        assert_eq!(parse_status(&std::fs::read_to_string(&file).unwrap()), Some(3));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bash_compact_prompt_reflects_exit_status() {
        let run = |cmd: &str| {
            let out = std::process::Command::new("bash")
                .args([
                    "--norc",
                    "--noprofile",
                    "-c",
                    &format!("{cmd}; eval \"$PROMPT_COMMAND\"; printf %s \"$PS1\""),
                ])
                .env("PROMPT_COMMAND", BASH_PROMPT_COMMAND)
                .env("OXISHELL_COMPACT_PROMPT", "1")
                .env_remove("OXISHELL_ENV_FILE")
                .env_remove("OXISHELL_HIGHLIGHT_CMD")
                .env_remove("OXISHELL_BLOCKS")
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        let ok = run("true");
        assert!(ok.contains("1;32m") && ok.contains('❯'), "{ok}");
        assert!(run("false").contains("1;31m"), "red after a failed command");
        // Highlighted command: PS1 ends in bold cyan and PS0 resets before output.
        let out = std::process::Command::new("bash")
            .args([
                "--norc",
                "--noprofile",
                "-c",
                "eval \"$PROMPT_COMMAND\"; printf '%s|%s' \"$PS1\" \"$PS0\"",
            ])
            .env("PROMPT_COMMAND", BASH_PROMPT_COMMAND)
            .env("OXISHELL_COMPACT_PROMPT", "1")
            .env("OXISHELL_HIGHLIGHT_CMD", "1")
            .env_remove("OXISHELL_ENV_FILE")
            .output()
            .unwrap();
        let out = String::from_utf8(out.stdout).unwrap();
        let version = std::process::Command::new("bash")
            .args([
                "--norc",
                "--noprofile",
                "-c",
                "printf %s $((BASH_VERSINFO[0] * 100 + BASH_VERSINFO[1]))",
            ])
            .output()
            .unwrap();
        let has_ps0 = String::from_utf8(version.stdout).unwrap().parse::<u32>().unwrap() >= 404;
        if has_ps0 {
            assert!(out.ends_with(r"\[\e[1;36m\]|\e[0m"), "{out}");
        } else {
            assert!(!out.contains("36m"), "no highlight without PS0: {out}");
        }
        assert!(!ok.contains("36m"), "no highlight without the flag");
        // Command blocks: an empty row above the prompt for the block header.
        let out = std::process::Command::new("bash")
            .args(["--norc", "--noprofile", "-c", "eval \"$PROMPT_COMMAND\"; printf %s \"$PS1\""])
            .env("PROMPT_COMMAND", BASH_PROMPT_COMMAND)
            .env("OXISHELL_COMPACT_PROMPT", "1")
            .env("OXISHELL_BLOCKS", "1")
            .env_remove("OXISHELL_ENV_FILE")
            .env_remove("OXISHELL_HIGHLIGHT_CMD")
            .output()
            .unwrap();
        assert!(String::from_utf8(out.stdout).unwrap().starts_with("\\n\\[\\e[1;32m"));
        assert!(!ok.starts_with("\\n"), "no spacer without the flag");
        // Without the flag the user's prompt is untouched.
        let out = std::process::Command::new("bash")
            .args([
                "--norc",
                "--noprofile",
                "-c",
                "PS1='mine$ '; eval \"$PROMPT_COMMAND\"; printf %s \"$PS1\"",
            ])
            .env("PROMPT_COMMAND", BASH_PROMPT_COMMAND)
            .env_remove("OXISHELL_COMPACT_PROMPT")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "mine$ ");
    }

    #[test]
    fn parses_history_log() {
        assert_eq!(parse_hist_log("  5\t git status\n6\t*ls -la\n"), vec!["git status", "ls -la"]);
        // A tab inside the command stays; a line without a number continues the previous one.
        assert_eq!(
            parse_hist_log("7\t printf 'a\tb'\n8\t for i in 1\ndo :\n"),
            vec!["printf 'a\tb'", "for i in 1\ndo :"]
        );
        assert!(parse_hist_log("garbage\n").is_empty());
    }

    #[test]
    fn reads_only_new_complete_lines() {
        let dir = std::env::temp_dir().join(format!("oxishell-histlog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("p.hist");
        let mut off = 0;
        assert!(read_new_commands(&f, &mut off).is_empty(), "no file yet");
        std::fs::write(&f, "1\t one\n2\t tw").unwrap();
        assert_eq!(read_new_commands(&f, &mut off), vec!["one"], "half-written line waits");
        std::fs::write(&f, "1\t one\n2\t two\n3\t three\n").unwrap();
        assert_eq!(read_new_commands(&f, &mut off), vec!["two", "three"]);
        assert!(read_new_commands(&f, &mut off).is_empty());
        std::fs::write(&f, "1\t fresh\n").unwrap();
        assert_eq!(read_new_commands(&f, &mut off), vec!["fresh"], "restarted pane");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Commands built with Tab, arrows, Ctrl+R or paste reach the log too: the
    /// shell reports what it ran, not what was typed.
    #[test]
    fn bash_hook_logs_every_command_it_keeps() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("oxishell-bashlog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old_history"), "echo from-an-old-session\n").unwrap();
        // An interactive bash on a pipe runs the real hook before every prompt.
        let mut bash = std::process::Command::new("bash")
            .args(["--norc", "--noprofile", "-i"])
            .env("PROMPT_COMMAND", BASH_PROMPT_COMMAND)
            .env("HISTFILE", dir.join("old_history"))
            .env("HISTCONTROL", "ignorespace")
            .env_remove("OXISHELL_COMPACT_PROMPT")
            .env_remove("OXISHELL_BLOCKS")
            .env_remove("OXISHELL_HIGHLIGHT_CMD")
            .env("OXISHELL_ENV_FILE", dir.join("p.env"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // An empty line and a leading-space line are not kept by bash.
        bash.stdin
            .take()
            .unwrap()
            .write_all(b"echo one\n\n echo hidden\nfalse\necho two\n")
            .unwrap();
        bash.wait().unwrap();
        let mut off = 0;
        assert_eq!(
            read_new_commands(&dir.join("p.hist"), &mut off),
            vec!["echo one", "false", "echo two"],
            "the old session's last command is not logged at the first prompt"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn snippets() {
        assert!(snippet("zsh").unwrap().contains("add-zsh-hook precmd"));
        assert!(snippet("fish").unwrap().contains("fish_prompt"));
        assert!(snippet("bash").unwrap().contains("PROMPT_COMMAND="));
        assert!(snippet("tcsh").is_none());
        assert_eq!(shell_name("/usr/bin/zsh"), "zsh");
    }
}
