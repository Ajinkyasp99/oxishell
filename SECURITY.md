# Security policy

## Reporting a vulnerability

Please **do not** open a public issue. Use GitHub's private
[security advisory form](https://github.com/Ajinkyasp99/oxishell/security/advisories/new) instead.
Include the affected version, the steps to reproduce, and the impact. You should get a reply within 7 days.

## Supported versions

Only the latest release receives security fixes.

## Security model

- oxishell opens no network ports. Automation goes through a Unix socket at
  `$XDG_RUNTIME_DIR/oxishell/oxishell.sock` (mode 0600, in a 0700 directory). Every connection's peer uid is
  checked, and requests are validated against a fixed method list with size limits.
- Injecting keystrokes into panes (`oxi pane send`) is disabled unless `[ipc] allow_send = true`.
- Session restore never executes commands. Saved output is stripped of escape sequences and displayed only.
  Agent resume commands are typed for the user to confirm, never run, and their session ids are validated.
- Text from terminals and agents shown in notifications is sanitized and length-limited.
- Agent hook installation only edits entries tagged `oxishell-hook`, backs up the original file once,
  and never overwrites files it cannot parse.
