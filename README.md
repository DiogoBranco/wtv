# wtv — worktree viewer

A read-only code viewer for people who work in many git worktrees at once, with a
coding agent running in each one. It shows any worktree's files, shows its PR diff
against a base branch, and sends questions about the code straight to the Claude
Code or Codex session sitting next to it.

It runs in the terminal, so it lives inside your tmux layout instead of a separate
window.

```
┌──────────────────────┬──────────────────┐
│                      │  claude          │
│  wtv                 ├──────────────────┤
│  tree + code / diff  │  codex           │
│                      ├──────────────────┤
│                      │  shell           │
└──────────────────────┴──────────────────┘
```

## Why

- **One list of your worktrees.** Every worktree of every configured repo, in one
  picker. Switching is a keystroke, not a `cd` and a re-open.
- **The PR diff, not just the file.** A side-by-side diff against the merge-base
  with your default branch — what a reviewer will see. Pick another base for
  stacked PRs.
- **Ask the agent that already has the context.** Right-click a line, type a
  question, and it goes to the Claude or Codex session running in that same
  worktree — the one that knows the work. The answer appears in your terminal,
  where you can keep talking to it.
- **Switching brings the agents with you.** Change worktree and the agent panes
  restart there, resuming that worktree's last session, or starting fresh if it
  has none.
- **It cannot break your code.** wtv never writes to a worktree. It reads files,
  runs read-only git commands, and writes only its own config.
- **Shared machines stay sane.** Only worktrees owned by your user are listed, so
  a repo shared with colleagues shows you your own trees.

## Requirements

- Rust (to build) — `rustup` from https://rustup.rs
- git 2.5+ (worktree support)
- tmux, only for the agent features (asking, switching, opening panes)
- Claude Code and/or Codex — whichever you have. wtv uses the ones it finds and
  ignores the ones it does not.
- Node, only if you also want the optional browser UI

Nothing else is required. [workmux](https://workmux.raine.dev) is supported but
optional — see [Layout](#layout).

## Install

```sh
git clone https://github.com/DiogoBranco/wtv
cd wtv
cargo install --path .
```

That installs `wtv` (the terminal UI) and `wtv-web` (an optional browser UI).

## Run

Open a terminal, go to a repo or worktree, and run:

```sh
wtv-up
```

That is the whole cold start. It creates the tmux session, builds the layout —
viewer left, claude and codex stacked on the right, a shell at the bottom — and
attaches you to it. Run it again later and it attaches to the same session.

Add worktree names to open one window each:

```sh
wtv-up dev-1579-multipass-doc-enrichment dev-1577-eval-search-latency-cost
```

Just the viewer, no panes:

```sh
wtv
```

On the first run wtv writes `~/.config/wtv/config.toml` pointing at the repository
you are standing in, and opens on the worktree of your current directory.

To watch several repos, list them:

```toml
repos = [
  "/home/you/code/api",
  "/home/you/code/web",
]
accent = "#98c379"   # one accent color, used for focus and selection
claude = "claude"    # command used to start/resume Claude Code
codex = "codex"      # command used to start/resume Codex
```

Point at a different config with `wtv --config /path/to/config.toml`.

## Keys

| Key | Action |
| --- | --- |
| `w` | worktree picker (switching also retargets the agent panes) |
| `n` | new worktree — type a branch name, it is created from your default branch |
| | (any `post_create` hooks in `.workmux.yaml` run in the shell pane) |
| `d` | switch between Browse and Diff |
| `b` | pick the diff base (for stacked PRs) |
| `j` `k` / arrows | move; `Tab` switches between tree and code |
| `Enter` | open a file, expand a folder, or expand a collapsed diff region |
| `v` then `j` `k` | select lines; `h` / `l` choose the base or updated side |
| `y` | copy the selection to the system clipboard |
| `a` | ask about the cursor line or selection; `Tab` picks claude or codex |
| `A` | open the agent panes — builds the layout, or brings back one you closed |
| `[` `]` | resize the sidebar; `{` `}` move the diff divider |
| `q` | quit |

The mouse works too: click to move, drag to select, drag a border to resize,
right-click to ask.

## Layout

The layout is the viewer on the left, an agent above another agent on the right,
and a shell at the bottom. You do not have to build it yourself.

**Press `A`** inside wtv and it opens whatever is missing: run wtv alone in a tmux
window and `A` gives you the full layout; close the codex pane by accident and `A`
brings it back, resuming that worktree's session. It only opens agents you actually
have installed — with Claude Code but no Codex, you get wtv, claude and a shell.

Everything below is optional.

### A window per worktree, with workmux

If you use [workmux](https://workmux.raine.dev), it can create the layout for every
worktree window it opens. Put this in `.workmux.yaml` in your repo, or in
`~/.config/workmux/config.yaml` for all repos:

```yaml
panes:
  - command: wtv
    focus: true
  - command: claude
    split: horizontal
  - command: codex
    split: vertical
  - split: vertical
    size: 8
```

Then `workmux add my-branch` (or `workmux open <existing>`) gives you a window per
worktree with that layout. `scripts/wtv-up` is a small helper that creates the tmux
session and opens the worktrees you name.

`n` inside wtv is the in-place alternative: it creates the worktree, switches the
current window to it, and runs the repo's `post_create` hooks in the shell pane, so
a new tree is provisioned the same way workmux would provision it. Use `workmux add`
when you want a separate window per branch.

### Stacked panes

`scripts/wtv-stack` adds zellij-style pane stacking, if you bind it:

```tmux
bind -n M-j select-pane -D \; run-shell 'wtv-stack'
bind -n M-k select-pane -U \; run-shell 'wtv-stack'
bind -n M-m run-shell 'wtv-stack'
bind -n M-e run-shell 'wtv-stack --even'
```

## Agent to agent

The two agents can talk to each other, in two shapes, and you choose per message:

```sh
wtv say codex "review the auth change I just pushed"   # into its live pane, no reply
wtv ask codex "does this caching plan hold up?"        # prints its reply back
```

`say` types into the running pane — that agent answers with its full session context
and you read it there. `ask` talks to a wtv-owned side session instead, so the reply
comes back as clean output the caller can act on, and your live conversations are
never written to by two processes at once.

`ask` is what makes an argument possible: claude states its case, gets codex's
counter as command output, and pushes back. The whole exchange shows up in the
caller's pane as it happens, so you can interrupt whenever you have heard enough.
`--new` starts a fresh discussion instead of continuing the last one.

Messages carry their origin (`[from claude] …`), taken from the calling pane rather
than the sender's word for it. Three things keep this from running away: a message
that is already tagged cannot be forwarded again, exchanges are rate limited per
worktree, and you can always interrupt the caller.

Every `ask` discussion is written to a markdown transcript outside the worktree, at
`~/.local/share/wtv/<worktree>/<date>-<topic>.md`, and the path is printed with each
reply — so afterwards you can tell either agent "re-read the argument and implement
the option that won".

To let them use it, add one line to each agent's instructions:

```md
To get a second opinion from the other agent in this worktree, run
`wtv ask codex "<question>"` (or `wtv ask claude`). It prints the reply.
Use `wtv say <agent> "<message>"` to send something without waiting.
```

Claude Code will ask permission for the command until you allowlist `wtv` in
`~/.claude/settings.json`.

## How the agent link works

wtv finds the pane in its own tmux window whose process is Claude Code or Codex
and types into it. The prompt is a reference, never a code dump — for example:

```
why did this change? src/api/auth.rs:40-58 diff against origin/main
```

The agent reads the file itself, so the prompt stays small and the conversation
stays in your terminal.

## The browser UI

`wtv-web` serves the same views at http://127.0.0.1:7345 with CodeMirror. It needs
the frontend built and must run from the project directory:

```sh
cd web && npm install && npm run build && cd ..
wtv-web
```

The terminal UI is the one that gets the features; the browser UI exists for when
you want a big screen.

## Limitations

- Linux and macOS. The owner check and process scan use Unix APIs.
- Read-only. Editing happens in your editor or your agent.
- Switching worktrees restarts the agents in those panes. Their sessions are saved
  and resume when you switch back, but finish anything mid-flight first.
- Panes running something other than an agent or an idle shell are left alone, so
  a dev server or test run survives a worktree switch.

## License

MIT — see [LICENSE](LICENSE).
