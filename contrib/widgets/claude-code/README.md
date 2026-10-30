# Claude Code widget for rigtop

A rigtop [widget](../../../README.md#widgets) showing the subscription usage limits (5-hour and weekly windows, with reset times), the running Claude Code sessions per project, how full each one's context window is, and their active subagents.

```
╭ Claude Code ───────────────────────────╮
│5h       ■■■■■■■■■■ 99% left · 09:40    │
│week     ■■■■■■■■■■ 95% left · Sun 15:00│
│rigtop                                  │
│busy     7a · 1 agent                   │
│  ctx    ■■■■■■···· 61% · 606K/1M       │
│  agent  Explore: find the config loader│
│idle     06 · waiting on 1 task · 3m    │
│  ctx    ■■■■■■■■■· 93% · 930K/1M       │
│my-obsidian-vault                       │
│idle     d8 · your turn · 12m           │
│  ctx    ■········· 12% · 24K/200K      │
│idle     5e · your turn · 48m           │
│+2 idle over 2h                         │
╰────────────────────────────────────────╯
```

- Usage windows show what is **left** (100% when unused) and when they reset; `RIGTOP_CLAUDE_QUOTA=used` shows usage instead.
- `ctx` is how much of the session's context window its conversation fills, in the session's color but yellow from 70% and red from 90%, when compacting it or starting a new session is worth a thought. The window is 1M tokens for current Claude models (Opus 4.6 and later, Sonnet 4.6 and later, Haiku 5.5, Fable) and models chosen with `[1m]`, 200K for older ones; `RIGTOP_CLAUDE_CONTEXT_WINDOW=1000000` sets it for every session. Other models (e.g. through Ollama) show the token count only.
- Sessions are named without the project prefix, under the project's name. An idle session tells whether its turn ended with work still running in the background (`waiting on 1 task, 2 agents`) or it waits for you (`your turn`), and for how long.
- Working sessions (busy, or with background work) come first, in green; then the ones waiting for you, most recent first. One stopped at a permission prompt (`waiting`) stays yellow; one whose turn ended (`your turn`) is yellow for 30 minutes (`RIGTOP_CLAUDE_FRESH_SECS`), then grey without its `ctx` row, and left out after 2 hours (`RIGTOP_CLAUDE_HIDE_SECS`, 0 to keep them all), counted on the last line. A project's name is repeated where this order moves to another project and back.

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

2. For the usage limits, make `statusline-save-limits` Claude Code's status line command, in `~/.claude/settings.json`:

   ```json
   "statusLine": {
     "type": "command",
     "command": "/path/to/rigtop/contrib/widgets/claude-code/statusline-save-limits"
   }
   ```

   Claude Code passes the limits to its status line; the script saves them to `~/.cache/rigtop/claude-rate-limits.json` for the widget and prints a short status line. To keep an existing status line, pass its command as arguments: `statusline-save-limits /path/to/my-statusline`.

   This needs a Pro or Max plan signed in with Claude Code (API billing has no such limits) and the terminal `claude`: the VS Code extension does not run status line commands. The limits refresh while a session talks to the API; older data is marked "as of".

   Already running [claude-spillway](https://github.com/akivajp/claude-spillway)? Then the widget reads the limits from its status endpoint (`http://127.0.0.1:8787/_spillway/status`; `RIGTOP_CLAUDE_SPILLWAY_URL` for another address, empty to skip it) without the status line, and also shows when traffic spilled over to Ollama, with Ollama's usage.

## Where the data comes from

| Rows | Source | Notes |
|---|---|---|
| Usage limits | `rate_limits` in the JSON Claude Code passes to the status line (documented), saved by `statusline-save-limits`; or claude-spillway's status endpoint when it runs, which reads them from Anthropic's response headers | Pro and Max plans signed in with Claude Code (not API billing); refreshed only while a session talks to the API, so the widget marks old data with "as of". The terminal UI runs the status line; the VS Code extension did not (Claude Code 2.1.310), so sessions used only from VS Code leave the limits empty. |
| Sessions | `claude agents --json` (documented); `~/.claude/sessions/` when `claude` is not on the PATH | `busy`, `waiting` (needs you, e.g. a permission prompt), `idle`. How long a session is idle comes from `statusUpdatedAt` in `~/.claude/sessions/<pid>.json` (undocumented). |
| Background tasks | Shells the session's Bash tool started (descendant processes running through `~/.claude/shell-snapshots/`, Linux) | Counted for idle sessions, as `waiting on N tasks`. |
| Context window use | The token counts of the last reply in the session's transcript, `~/.claude/projects/<project>/<session>.jsonl` (undocumented; only its last 512 KB is read) | Input plus cached input of that reply; right after a compaction, the size it left. |
| Subagents | `~/.claude/projects/<project>/<session>/subagents/` | Shown while their transcript changed within the last 2 minutes (`RIGTOP_CLAUDE_ACTIVE_SECS`); a subagent waiting on a long command may drop out early. |

The widget does not use undocumented usage endpoints or Claude Code's credentials.
