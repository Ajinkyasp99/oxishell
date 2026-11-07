//! Shell integration for context chips.
//!
//! Activating a Python virtualenv or conda environment only changes the
//! shell's own variables, which are not visible from outside (/proc shows the
//! environment the shell *started* with). So the shell reports it: at every
//! prompt it writes the active environment name to a small per-pane file,
//! `$CMUX_ENV_FILE`, using only shell builtins (no extra process).
//!
//! bash is set up automatically through `PROMPT_COMMAND` in the pane
//! environment (unless your own config replaces PROMPT_COMMAND). zsh and fish
//! need one line in their config: `cmux shell-integration zsh|fish`.
//!
//! The same hook optionally replaces the prompt with a compact `❯` (the chips
//! already show folder, branch and environment), only inside cmux-linux panes.

use std::path::{Path, PathBuf};

pub fn env_file(pane: crate::model::PaneId) -> PathBuf {
    crate::paths::runtime_dir().join("panes").join(format!("{pane}.env"))
}

/// bash: run at every prompt (shell builtins only, no extra process).
///
/// It remembers the last exit status, reports the active Python/conda env for
/// the chips (`${var##*/}` keeps the venv's folder name) and, with
/// `CMUX_COMPACT_PROMPT=1`, sets the prompt to `❯` (green, or red after a
/// failed command).
pub const BASH_PROMPT_COMMAND: &str = concat!(
    r#"__cmux_status=$?; "#,
    r#"[ -n "$CMUX_ENV_FILE" ] && builtin printf '%s\n%s\n' "${VIRTUAL_ENV##*/}" "$CONDA_DEFAULT_ENV" >| "$CMUX_ENV_FILE" 2>/dev/null; "#,
    r#"if [ "$CMUX_COMPACT_PROMPT" = 1 ]; then if [ "$__cmux_status" = 0 ]; then PS1='\[\e[1;32m\]❯\[\e[0m\] '; else PS1='\[\e[1;31m\]❯\[\e[0m\] '; fi; fi"#
);

pub fn snippet(shell: &str) -> Option<String> {
    Some(match shell {
        "bash" => format!(
            "# cmux-linux context chips (already automatic unless your config sets PROMPT_COMMAND)\nPROMPT_COMMAND='{}'\"${{PROMPT_COMMAND:+; $PROMPT_COMMAND}}\"\n",
            BASH_PROMPT_COMMAND.replace('\'', r"'\''")
        ),
        "zsh" => r#"# cmux-linux context chips and compact prompt — add to ~/.zshrc
if [[ -n "$CMUX_ENV_FILE" ]]; then
  _cmux_precmd() {
    builtin printf '%s\n%s\n' "${VIRTUAL_ENV:t}" "$CONDA_DEFAULT_ENV" >| "$CMUX_ENV_FILE" 2>/dev/null
    [[ "$CMUX_COMPACT_PROMPT" == 1 ]] && PROMPT='%(?.%F{green}.%F{red})%B❯%b%f ' && RPROMPT=''
  }
  autoload -Uz add-zsh-hook && add-zsh-hook precmd _cmux_precmd
fi
"#
        .to_string(),
        "fish" => r#"# cmux-linux context chips and compact prompt — add to ~/.config/fish/config.fish
if set -q CMUX_ENV_FILE
    function _cmux_report_env --on-event fish_prompt
        set -l venv ''
        set -q VIRTUAL_ENV; and set venv (path basename $VIRTUAL_ENV)
        printf '%s\n%s\n' $venv "$CONDA_DEFAULT_ENV" > $CMUX_ENV_FILE 2>/dev/null
    end
    if test "$CMUX_COMPACT_PROMPT" = 1
        function fish_prompt
            set -l s $status
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

pub fn read_env(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_env_file(&text)
}

pub fn shell_name(shell: &str) -> &str {
    Path::new(shell).file_name().and_then(|f| f.to_str()).unwrap_or(shell)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_names() {
        assert_eq!(parse_env_file("CommonVenv\n\n"), Some("CommonVenv".into()));
        assert_eq!(parse_env_file("\nml\n"), Some("ml".into()));
        assert_eq!(parse_env_file("\nbase\n"), None);
        assert_eq!(parse_env_file("\n\n"), None);
        assert_eq!(parse_env_file(""), None);
        assert_eq!(parse_env_file("a\u{1b}b\n"), Some("ab".into()));
    }

    #[test]
    fn bash_hook_reports_activated_venv() {
        let dir = std::env::temp_dir().join(format!("cmux-shellint-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("p.env");
        // Simulate `source myenv/bin/activate` (sets VIRTUAL_ENV) and one prompt.
        let script = "VIRTUAL_ENV=/home/u/proj/CommonVenv; eval \"$PROMPT_COMMAND\"";
        let ok = std::process::Command::new("bash")
            .args(["--norc", "--noprofile", "-c", script])
            .env("PROMPT_COMMAND", BASH_PROMPT_COMMAND)
            .env("CMUX_ENV_FILE", &file)
            .env_remove("CONDA_DEFAULT_ENV")
            .status()
            .unwrap()
            .success();
        assert!(ok);
        assert_eq!(read_env(&file), Some("CommonVenv".into()));
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
                .env("CMUX_COMPACT_PROMPT", "1")
                .env_remove("CMUX_ENV_FILE")
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        let ok = run("true");
        assert!(ok.contains("1;32m") && ok.contains('❯'), "{ok}");
        assert!(run("false").contains("1;31m"), "red after a failed command");
        // Without the flag the user's prompt is untouched.
        let out = std::process::Command::new("bash")
            .args([
                "--norc",
                "--noprofile",
                "-c",
                "PS1='mine$ '; eval \"$PROMPT_COMMAND\"; printf %s \"$PS1\"",
            ])
            .env("PROMPT_COMMAND", BASH_PROMPT_COMMAND)
            .env_remove("CMUX_COMPACT_PROMPT")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "mine$ ");
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
