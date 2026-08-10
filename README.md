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
- **Switching brings the agents with you.** Change worktree and wtv switches to
  that worktree's workspace, with its own agent panes and shell.
- **It cannot break your code.** wtv never writes to a worktree and never touches
  a local branch. It reads files, runs read-only git commands, and writes only its
  own config — the one exception is `r`, which fetches, updating remote-tracking
  refs under `.git`.
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

That is the whole cold start. It creates that worktree's tmux session, builds the layout —
viewer left, claude and codex stacked on the right, a shell at the bottom — and
attaches you to it. Run it again later and it attaches to the same session.

There is one tmux session per worktree. Switching worktrees switches sessions, so
clients in other worktrees do not move. Quitting wtv with `q` closes that workspace
and every pane in it, agents included; other worktree sessions keep running.
Detaching (`C-b d`) leaves the workspace running, as usual.

wtv does this itself, when started as `wtv --panes --close-window`. Started any
other way it leaves your windows alone.

Add worktree names to ensure one workspace for each and attach to the first:

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
| `w` | worktree picker (switching moves to that worktree's session) |
| | a worktree with a running workspace is marked `· open` and remains selectable |
| | ends with `+ new worktree` and `↓ from my pull requests` |
| `n` | new worktree — type a branch name; same as `+` in the picker |
| | (any `post_create` hooks in `.workmux.yaml` run in the shell pane) |
| `r` | fetch, then recompute the diff against the refreshed base |
| `d` | switch between Browse and Diff |
| `b` | pick the diff base (for stacked PRs) |
| `j` `k` / arrows | move; `Tab` switches between tree and code |
| `Enter` | open a file, open or close a folder, or expand a collapsed diff region |
| `v` then `j` `k` | select lines; `h` / `l` choose the base or updated side |
| `y` | copy the selection to the system clipboard |
| `a` | ask about the cursor line or selection; `Tab` picks claude or codex |
| `A` | open the agent panes — builds the layout, or brings back one you closed |
| `[` `]` | resize the sidebar; `{` `}` move the diff divider |
| `q` | quit |

The mouse works too: the wheel scrolls the code without moving your selection,
click selects a line, drag selects a range, drag a border to resize, right-click
to ask.

## Getting a worktree

The picker ends with two ways to add one.

**`+ new worktree`** takes a name. What it does depends on what already exists:

| The name is | What you get |
| --- | --- |
| new | a new branch off your default branch |
| an existing local branch | that branch checked out |
| only on the remote | a local branch tracking `origin/<name>` |

So the same box both starts work and pulls down work that already exists. It never
recreates a name you already have, and never leaves you on an empty branch that
merely shares a name with the remote one.

**`↓ from my pull requests`** lists your open PRs for that repo, newest first, each
with its review state:

```
#3771  feat/aq-search-cost-convergence  · approved
#3887  feat/dev-1582-structural-chunk-metadata  · draft
```

Pick one and you get a worktree on its branch. This is the quick path back into a
review on a machine that has none of your worktrees yet. It shells out to `gh`, so
you need the GitHub CLI logged in; without it the row reports that and nothing else
changes.

## Keeping the diff honest

The diff base is a remote-tracking ref — `origin/HEAD`, else `origin/main`. Those
are local mirrors: they move only when something fetches, and nothing here fetches
on its own. So when a colleague merges to main, your diff keeps using the old
merge-base until you say otherwise, without any hint that it has aged.

**`r`** is that hint's cure: it fetches, re-reads the refs, re-derives the base if
the old one has gone away, and recomputes the diff. A base you picked yourself with
`b` is kept. Nothing fetches in the background — no surprise network calls — so a
stale base is always exactly one keystroke old.

## Only one session per worktree

Each worktree has one four-pane tmux workspace. wtv identifies it by the canonical
worktree path stored on the session, not by the human-readable session name. Picking
an already-open worktree switches to its existing workspace instead of creating a
second copy or blocking the selection. Detached sessions count and are reused.

## Layout

The layout is the viewer on the left, an agent above another agent on the right,
and a shell at the bottom. You do not have to build it yourself.

Each pane carries its name in the border — `claude`, `codex` — so you can tell the
agents apart at a glance. tmux would otherwise label both panes with the shell that
launched them (`zsh`). wtv turns border titles on for its own windows only; your
other windows keep whatever your tmux config says.

**Press `A`** inside wtv and it opens whatever is missing: run wtv alone in a tmux
window and `A` gives you the full layout; close the codex pane by accident and `A`
brings it back, resuming that worktree's session. It only opens agents you actually
have installed — with Claude Code but no Codex, you get wtv, claude and a shell.

Everything below is optional.

### Worktree layouts with workmux

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

Then `workmux add my-branch` (or `workmux open <existing>`) gives you a layout per
worktree with that layout. `scripts/wtv-up` is a small helper that creates the tmux
sessions for the worktrees you name.

`n` inside wtv is the in-place alternative: it creates the worktree, opens its
session, and runs the repo's `post_create` hooks in the shell pane, so
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
that is already tagged cannot be forwarded again, `ask` is capped at 12 exchanges
per agent per 15 minutes in a worktree, and you can always interrupt the caller.

Note the cap applies to `ask` only — **`say` has no rate limit.** Two agents that
each answer the other's answer will keep going until something runs out, so tell
them to reply once rather than to hold a conversation.

Every `ask` discussion is written to a markdown transcript outside the worktree, at
`~/.local/share/wtv/<worktree>/<date>-<topic>.md`, and the path is printed with each
reply — so afterwards you can tell either agent "re-read the argument and implement
the option that won".

The agents do not know these commands exist. You must put them in each agent's
instruction file — `CLAUDE.md` for Claude Code, `AGENTS.md` for Codex, either in
the repo or globally (`~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`). `ask` works
with just the first half; `say` needs both halves on **both** agents, or a `say`
never comes back:

```md
To get a second opinion from the other agent in this worktree, run
`wtv ask codex "<question>"` (or `wtv ask claude`). It prints the reply.
Use `wtv say <agent> "<message>"` to send something without waiting.

When text prefixed `[from …]` appears in your pane, that is the other agent asking
you directly. Do the work, then send your answer back to its pane with
`wtv say <that agent> "<answer>"`. Reply once, then stop.
```

Claude Code will ask permission for the command until you allowlist it in
`~/.claude/settings.json`:

```json
{ "permissions": { "allow": ["Bash(wtv ask:*)", "Bash(wtv say:*)"] } }
```

### Codex needs its sandbox opened

Codex sandboxes the commands it runs, and the sandbox blocks **both** things wtv
needs to find a pane:

```
error connecting to /private/tmp/tmux-501/default (Operation not permitted)
zsh:1: operation not permitted: ps
```

The symptom is misleading — codex runs the command, wtv starts, `tmux list-panes` is
denied, and you get `no claude pane found for this worktree` while the pane is
plainly on screen. The same command from your own shell works, which makes it look
like wtv is at fault.

There is no narrow fix: adding the socket directory to `sandbox_workspace_write.writable_roots`
leaves both restrictions in place, and `ps` is needed whenever the pane's process
name is not literally `claude` or `codex` — which is the case for Claude Code's
versioned binary (`~/.local/bin/claude` → `…/versions/2.1.226`). Only full access
works:

```toml
# ~/.codex/config.toml
sandbox_mode = "danger-full-access"
```

Confirm with `codex doctor` — it should report `filesystem sandbox: unrestricted`.

**Weigh this before you set it.** It removes sandboxing from every command codex
runs, anywhere on disk, not just the wtv ones.

`ask` is not a way around it. The sandbox also blocks network, so a sandboxed codex
cannot run the other agent either:

```
example.com  unsandboxed → 200
example.com  sandboxed   → 000
```

So while codex is sandboxed it cannot start an exchange at all, in either shape.
Claude → codex still works throughout, because claude is the one running the
command and it is not sandboxed. If you would rather leave codex sandboxed, keep
the traffic one-way: claude sends with `say`, then reads the answer out of codex's
pane itself with `tmux capture-pane -p -t <pane>`.

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
- Each worktree workspace keeps its own panes and processes running when you switch
  to another session.

## License

MIT — see [LICENSE](LICENSE).
