# Claude Code widget for rigtop

A rigtop [widget](../../../README.md#widgets) showing the subscription usage limits (5-hour and weekly windows, with reset times), the running Claude Code sessions per project, and their active subagents.

```
╭ Claude Code ───────────────────────────╮
│5h       ■■■■■■■■■■ 42% · resets 05:51  │
│week     ■■■■■■■■■■ 72% · resets Thu 15…│
│my-obsidian-vault                       │
│idle     my-obsidian-vault-d8           │
│rigtop                                  │
│busy     rigtop-06                      │
│  agent  claude-code-guide: Claude Code…│
╰────────────────────────────────────────╯
```

It reads files Claude Code writes, some of which are internal and may change between versions; it then shows fewer rows rather than failing. Requires Python 3 (standard library only).

## Setup

1. Add the widget to rigtop's configuration (`~/.config/rigtop/config.toml`):

   ```toml
   [[widgets]]
   name = "Claude Code"
   command = "/path/to/rigtop/contrib/widgets/claude-code/claude-code-widget"
   ```

   Widgets run on the machine that displays them. If Claude Code runs on another machine (rigtop-gui on Windows, Claude Code on a Linux box), run the widget there over SSH:

   ```toml
   command = "ssh zefat /path/to/rigtop/contrib/widgets/claude-code/claude-code-widget"
   ```

2. For the usage limits, make `statusline-save-limits` Claude Code's status line command in `~/.claude/settings.json`:

   ```json
   "statusLine": {
     "type": "command",
     "command": "/path/to/rigtop/contrib/widgets/claude-code/statusline-save-limits"
   }
   ```

   It saves the limits to `~/.cache/rigtop/claude-rate-limits.json` and prints a short status line. To keep an existing status line, pass its command as arguments: `statusline-save-limits /path/to/my-statusline`.

## Where the data comes from

| Rows | Source | Notes |
|---|---|---|
| Usage limits | `rate_limits` in the JSON Claude Code passes to the status line (documented) | Pro and Max plans signed in with Claude Code (not API billing); refreshed only while a session talks to the API, so the widget marks old data with "as of". The terminal UI runs the status line; the VS Code extension did not (Claude Code 2.1.310), so sessions used only from VS Code leave the limits empty. |
| Sessions | `claude agents --json` (documented); `~/.claude/sessions/` when `claude` is not on the PATH | `busy` (green), `waiting` (yellow), `idle` (grey) |
| Subagents | `~/.claude/projects/<project>/<session>/subagents/` | Shown while their transcript changed within the last 2 minutes (`RIGTOP_CLAUDE_ACTIVE_SECS`); a subagent waiting on a long command may drop out early. |

The widget does not use undocumented usage endpoints or Claude Code's credentials.
