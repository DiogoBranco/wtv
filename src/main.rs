use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use imara_diff::{Algorithm, Diff, InternedInput};
use notify::RecommendedWatcher;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SyntectColor, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use wtv::core::{self, ChangedFile, Config, RepoWorktrees};
use wtv::relay;

#[derive(Clone, Copy, PartialEq)]
enum Mode { Browse, Diff }

#[derive(Clone, Copy, PartialEq)]
enum Focus { Sidebar, Content }

#[derive(Clone)]
enum Node {
    Dir { label: String, path: String, depth: usize, open: bool },
    File { label: String, path: String, depth: usize, stats: Option<(Option<u32>, Option<u32>)> },
}

#[derive(Default)]
struct Tree {
    dirs: BTreeMap<String, Tree>,
    files: Vec<String>,
}

#[derive(Clone)]
enum DiffRow {
    Pair { old_no: Option<usize>, old: String, new_no: Option<usize>, new: String, deleted: bool, added: bool },
    Fold { old_start: usize, new_start: usize, count: usize },
}

enum Popup { Worktree, Base, PullRequest }

#[derive(Clone, Copy)]
enum Drag { Sidebar, Split }

#[derive(Clone, Copy, PartialEq)]
enum Side { Old, New }

struct Prompt {
    text: String,
    agent: usize,
    lines: String,
}

struct App {
    config: Config,
    repos: Vec<RepoWorktrees>,
    repo_index: usize,
    worktree_index: usize,
    mode: Mode,
    focus: Focus,
    base: Option<String>,
    branches: Vec<String>,
    paths: Vec<String>,
    changed: Vec<ChangedFile>,
    nodes: Vec<Node>,
    open_dirs: HashSet<String>,
    selected: usize,
    active_path: Option<String>,
    content: Vec<String>,
    highlighted: Vec<Vec<(String, Color)>>,
    old_highlighted: Vec<Vec<(String, Color)>>,
    new_highlighted: Vec<Vec<(String, Color)>>,
    diff_rows: Vec<DiffRow>,
    expanded: HashSet<(usize, usize)>,
    scroll: usize,
    cursor: usize,
    anchor: Option<usize>,
    visual: bool,
    select_side: Side,
    view_height: usize,
    view_width: usize,
    row_map: Vec<usize>,
    rendered_rows: usize,
    sidebar_width: u16,
    split_pct: u16,
    dragging: Option<Drag>,
    horizontal: usize,
    accent: Color,
    status: String,
    popup: Option<Popup>,
    popup_index: usize,
    open_worktrees: HashSet<PathBuf>,
    pulls: Vec<core::PullRequest>,
    status_shown: String,
    status_at: Instant,
    prompt: Option<Prompt>,
    new_branch: Option<String>,
    shell_pane: Option<String>,
    watcher: Option<RecommendedWatcher>,
    watch_rx: Option<Receiver<()>>,
    dirty_since: Option<Instant>,
    syntax_set: SyntaxSet,
    theme: Theme,
    quit: bool,
}

impl Tree {
    fn insert(&mut self, path: &str) {
        let mut parts = path.split('/').peekable();
        let mut node = self;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                node.files.push(part.to_string());
            } else {
                node = node.dirs.entry(part.to_string()).or_default();
            }
        }
    }
}

fn hex_color(value: &str) -> Color {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&value[0..2], 16),
            u8::from_str_radix(&value[2..4], 16),
            u8::from_str_radix(&value[4..6], 16),
        ) { return Color::Rgb(r, g, b); }
    }
    Color::Rgb(152, 195, 121)
}

fn extension_color(path: &str) -> Color {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let hex = match ext.as_str() {
        "py" => "3572a5", "ts" | "tsx" => "3178c6", "js" | "jsx" => "f1e05a",
        "rs" => "dea584", "go" => "00add8", "java" => "b07219", "rb" => "701516",
        "c" | "h" => "555555", "cpp" => "f34b7d", "cs" => "178600", "php" => "4f5d95",
        "swift" => "f05138", "kt" => "a97bff", "sh" => "89e051", "sql" => "e38c00",
        "html" => "e34c26", "css" => "663399", "scss" => "c6538c", "svelte" => "ff3e00",
        "vue" => "41b883", "md" => "519aba", "json" => "cbcb41", "yml" | "yaml" => "cb171e",
        "toml" | "lock" => "9c4221", "svg" => "ffb13b", "png" | "jpg" => "a074c4",
        "ipynb" => "da5b0b", _ => "5a5a5a",
    };
    hex_color(hex)
}

fn syntect_color(color: SyntectColor) -> Color {
    Color::Rgb(color.r, color.g, color.b)
}

fn wtv_theme() -> Theme {
    let color = |hex: u32| SyntectColor {
        r: (hex >> 16) as u8,
        g: (hex >> 8) as u8,
        b: hex as u8,
        a: 255,
    };
    let item = |scopes: &str, fg: u32, font: Option<FontStyle>| ThemeItem {
        scope: scopes.parse::<ScopeSelectors>().unwrap_or_default(),
        style: StyleModifier {
            foreground: Some(color(fg)),
            background: None,
            font_style: font,
        },
    };
    let mut theme = Theme::default();
    theme.settings.foreground = Some(color(0xc8c8c8));
    theme.scopes = vec![
        item("comment", 0x768390, Some(FontStyle::ITALIC)),
        item("string", 0xa5d6ff, None),
        item("constant", 0x79c0ff, None),
        item("keyword, storage", 0xff7b72, None),
        item("entity.name.function, support.function, variable.function", 0xd2a8ff, None),
        item("entity.name.type, support.type, support.class, storage.type", 0xffa657, None),
        item("entity.name.tag", 0x7ee787, None),
        item("entity.other.attribute-name", 0x79c0ff, None),
        item("markup.heading, punctuation.definition.heading", 0xd2a8ff, Some(FontStyle::BOLD)),
        item("markup.raw, markup.inline.raw", 0xa5d6ff, None),
        item("markup.underline.link, string.other.link, meta.link", 0x58a6ff, None),
        item("punctuation.definition.list_item", 0xff7b72, None),
        item("markup.quote", 0x768390, Some(FontStyle::ITALIC)),
        item("meta.separator", 0x768390, None),
    ];
    theme
}

fn compress_tree(tree: &Tree, prefix: &str, depth: usize, open: &HashSet<String>, stats: &HashMap<String, (Option<u32>, Option<u32>)>, force_open: bool, out: &mut Vec<Node>) {
    for (name, child) in &tree.dirs {
        let mut label = name.clone();
        let mut path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let mut current = child;
        while current.files.is_empty() && current.dirs.len() == 1 {
            let (next, next_tree) = current.dirs.iter().next().unwrap();
            label.push('/');
            label.push_str(next);
            path.push('/');
            path.push_str(next);
            current = next_tree;
        }
        let is_open = force_open || open.contains(&path);
        out.push(Node::Dir { label: format!("{label}/"), path: path.clone(), depth, open: is_open });
        if is_open { compress_tree(current, &path, depth + 1, open, stats, force_open, out); }
    }
    let mut files = tree.files.clone();
    files.sort();
    for name in files {
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        out.push(Node::File { label: name, path: path.clone(), depth, stats: stats.get(&path).copied() });
    }
}

fn collapse_segments(old: &str, new: &str, expanded: &HashSet<(usize, usize)>) -> Vec<DiffRow> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let input = InternedInput::new(old, new);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let hunks: Vec<_> = diff.hunks().collect();
    let mut rows = Vec::new();
    let mut oi = 0usize;
    let mut ni = 0usize;
    for hunk in hunks.iter().chain(std::iter::once(&imara_diff::Hunk { before: old_lines.len() as u32..old_lines.len() as u32, after: new_lines.len() as u32..new_lines.len() as u32 })) {
        let end_o = hunk.before.start as usize;
        let end_n = hunk.after.start as usize;
        let unchanged = end_o.saturating_sub(oi).min(end_n.saturating_sub(ni));
        if unchanged > 7 && !expanded.contains(&(oi, ni)) {
            for n in 0..3 { rows.push(pair(&old_lines, &new_lines, oi + n, ni + n, false, false)); }
            rows.push(DiffRow::Fold { old_start: oi, new_start: ni, count: unchanged - 6 });
            for n in unchanged - 3..unchanged { rows.push(pair(&old_lines, &new_lines, oi + n, ni + n, false, false)); }
        } else {
            for n in 0..unchanged { rows.push(pair(&old_lines, &new_lines, oi + n, ni + n, false, false)); }
        }
        oi = end_o;
        ni = end_n;
        let removed = hunk.before.end.saturating_sub(hunk.before.start) as usize;
        let added = hunk.after.end.saturating_sub(hunk.after.start) as usize;
        for n in 0..removed.max(added) {
            let old_no = (n < removed).then_some(oi + n + 1);
            let new_no = (n < added).then_some(ni + n + 1);
            rows.push(DiffRow::Pair {
                old_no,
                old: old_no.map(|v| old_lines[v - 1].to_string()).unwrap_or_default(),
                new_no,
                new: new_no.map(|v| new_lines[v - 1].to_string()).unwrap_or_default(),
                deleted: n < removed,
                added: n < added,
            });
        }
        oi = hunk.before.end as usize;
        ni = hunk.after.end as usize;
    }
    rows
}

fn pair(old: &[&str], new: &[&str], oi: usize, ni: usize, deleted: bool, added: bool) -> DiffRow {
    DiffRow::Pair { old_no: Some(oi + 1), old: old[oi].to_string(), new_no: Some(ni + 1), new: new[ni].to_string(), deleted, added }
}

impl App {
    fn new(config: Config) -> Result<Self, String> {
        let repos = core::repo_worktrees(&config);
        let cwd = std::fs::canonicalize(std::env::current_dir().map_err(|e| e.to_string())?).ok();
        let mut repo_index = 0;
        let mut worktree_index = 0;
        let mut best = 0;
        for (ri, repo) in repos.iter().enumerate() {
            for (wi, wt) in repo.worktrees.iter().enumerate() {
                if cwd.as_ref().is_some_and(|cwd| cwd.starts_with(&wt.path)) && wt.path.len() > best {
                    best = wt.path.len();
                    repo_index = ri;
                    worktree_index = wi;
                }
            }
        }
        let accent = hex_color(&config.accent);
        let mut app = Self {
            config, repos, repo_index, worktree_index, mode: Mode::Browse, focus: Focus::Sidebar,
            base: None, branches: Vec::new(), paths: Vec::new(), changed: Vec::new(), nodes: Vec::new(),
            open_dirs: HashSet::new(), selected: 0, active_path: None, content: Vec::new(), highlighted: Vec::new(), old_highlighted: Vec::new(), new_highlighted: Vec::new(),
            diff_rows: Vec::new(), expanded: HashSet::new(), scroll: 0, cursor: 0, anchor: None, visual: false, select_side: Side::New, view_height: 0, view_width: 0, row_map: Vec::new(), rendered_rows: 0, sidebar_width: 34, split_pct: 50, dragging: None, horizontal: 0, accent,
            status: String::new(), popup: None, popup_index: 0, open_worktrees: HashSet::new(), pulls: Vec::new(),
            status_shown: String::new(), status_at: Instant::now(), prompt: None, new_branch: None, shell_pane: None, watcher: None,
            watch_rx: None, dirty_since: None, syntax_set: SyntaxSet::load_defaults_newlines(),
            theme: wtv_theme(), quit: false,
        };
        if app.worktree().is_none() { return Err("no Owned Worktrees found".into()); }
        app.select_worktree()?;
        Ok(app)
    }

    fn worktree(&self) -> Option<&core::Worktree> {
        self.repos.get(self.repo_index)?.worktrees.get(self.worktree_index)
    }

    fn worktree_path(&self) -> PathBuf { PathBuf::from(&self.worktree().unwrap().path) }

    fn select_worktree(&mut self) -> Result<(), String> {
        let wt = self.worktree_path();
        self.branches = core::branch_refs(&wt);
        self.base = core::default_branch(&wt, &self.branches);
        self.open_dirs.clear();
        self.active_path = None;
        self.content.clear();
        self.diff_rows.clear();
        self.scroll = 0;
        let (tx, rx) = mpsc::channel();
        self.watcher = core::watch(&wt, tx).ok();
        self.watch_rx = Some(rx);
        self.refresh()
    }

    fn fetch_and_refresh(&mut self) {
        let wt = self.worktree_path();
        if let Err(e) = core::fetch(&wt) {
            self.status = e;
            return;
        }
        self.branches = core::branch_refs(&wt);
        // Keep a base picked with `b`; only re-derive one that has gone away.
        if !self.base.as_ref().is_some_and(|b| self.branches.contains(b)) {
            self.base = core::default_branch(&wt, &self.branches);
        }
        self.status = match &self.base {
            Some(base) => format!("fetched · base {base}"),
            None => "fetched".to_string(),
        };
        if let Err(e) = self.refresh() {
            self.status = e;
        }
    }

    fn refresh(&mut self) -> Result<(), String> {
        let wt = self.worktree_path();
        match self.mode {
            Mode::Browse => self.paths = core::files(&wt).map_err(str::to_string)?,
            Mode::Diff => {
                self.changed = self.base.as_ref().map(|b| core::changed_files(&wt, b)).transpose().map_err(str::to_string)?.unwrap_or_default();
                self.paths = self.changed.iter().map(|f| f.path.clone()).collect();
            }
        }
        self.rebuild_nodes();
        if let Some(path) = self.active_path.clone() {
            if self.paths.contains(&path) {
                let (scroll, cursor, anchor) = (self.scroll, self.cursor, self.anchor);
                let horizontal = self.horizontal;
                self.open(&path)?;
                let max = if self.mode == Mode::Browse { self.content.len() } else { self.diff_rows.len() }.saturating_sub(1);
                self.scroll = scroll.min(max);
                self.cursor = cursor.min(max);
                self.anchor = anchor.map(|a| a.min(max));
                self.horizontal = horizontal;
            } else { self.active_path = None; self.content.clear(); self.diff_rows.clear(); }
        }
        self.selected = self.selected.min(self.nodes.len().saturating_sub(1));
        Ok(())
    }

    fn rebuild_nodes(&mut self) {
        let mut tree = Tree::default();
        for path in &self.paths { tree.insert(path); }
        let stats: HashMap<_, _> = self.changed.iter().map(|f| (f.path.clone(), (f.additions, f.deletions))).collect();
        let mut nodes = Vec::new();
        compress_tree(&tree, "", 0, &self.open_dirs, &stats, self.mode == Mode::Diff, &mut nodes);
        self.nodes = nodes;
    }

    fn open(&mut self, path: &str) -> Result<(), String> {
        if self.active_path.as_deref() != Some(path) {
            self.expanded.clear();
        }
        self.active_path = Some(path.to_string());
        self.scroll = 0;
        self.cursor = 0;
        self.horizontal = 0;
        let wt = self.worktree_path();
        if self.mode == Mode::Browse {
            let content = core::file_content(&wt, path).map_err(str::to_string)?;
            self.content = content.map(|v| v.lines().map(str::to_string).collect()).unwrap_or_default();
            self.highlighted = self.highlight(path, &self.content.join("\n"));
        } else if let Some(base) = &self.base {
            let diff = core::diff_content(&wt, base, path).map_err(str::to_string)?;
            let old = diff.old.content.unwrap_or_default();
            let new = diff.new.content.unwrap_or_default();
            self.old_highlighted = self.highlight(path, &old);
            self.new_highlighted = self.highlight(path, &new);
            self.diff_rows = collapse_segments(&old, &new, &self.expanded);
        }
        Ok(())
    }

    fn highlight(&self, path: &str, content: &str) -> Vec<Vec<(String, Color)>> {
        let syntax = self.syntax_set.find_syntax_for_file(path).ok().flatten().unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        LinesWithEndings::from(content)
            .map(|line| highlighter.highlight_line(line, &self.syntax_set).unwrap_or_default().into_iter().map(|(style, text)| (text.trim_end_matches(['\r', '\n']).to_string(), syntect_color(style.foreground))).collect())
            .collect()
    }

    fn activate(&mut self, expand: bool) {
        let Some(node) = self.nodes.get(self.selected).cloned() else { return };
        match node {
            Node::Dir { path, open, .. } => {
                if expand && !open { self.open_dirs.insert(path); } else if !expand && open { self.open_dirs.remove(&path); } else if open { self.open_dirs.remove(&path); } else { self.open_dirs.insert(path); }
                self.rebuild_nodes();
            }
            Node::File { path, .. } => if let Err(e) = self.open(&path) { self.status = e; },
        }
    }

    fn move_cursor(&mut self, amount: isize) {
        match self.focus {
            Focus::Sidebar => self.selected = self.selected.saturating_add_signed(amount).min(self.nodes.len().saturating_sub(1)),
            Focus::Content => {
                if !self.visual {
                    self.anchor = None;
                }
                let max = if self.mode == Mode::Browse { self.content.len() } else { self.diff_rows.len() };
                self.cursor = self.cursor.saturating_add_signed(amount).min(max.saturating_sub(1));
                if self.cursor < self.scroll {
                    self.scroll = self.cursor;
                } else if self.rendered_rows > 0 && self.cursor >= self.scroll + self.rendered_rows {
                    self.scroll = self.cursor + 1 - self.rendered_rows;
                }
            }
        }
    }

    fn toggle_mode(&mut self) {
        self.mode = if self.mode == Mode::Browse { Mode::Diff } else { Mode::Browse };
        self.active_path = None;
        self.content.clear();
        self.diff_rows.clear();
        self.scroll = 0;
        self.selected = 0;
        if let Err(e) = self.refresh() { self.status = e; }
    }

    fn selection_range(&self) -> (usize, usize) {
        match self.anchor {
            Some(anchor) => (anchor.min(self.cursor), anchor.max(self.cursor)),
            None => (self.cursor, self.cursor),
        }
    }

    fn row_line(&self, row: usize) -> Option<usize> {
        if self.mode == Mode::Browse {
            (row < self.content.len()).then_some(row + 1)
        } else {
            self.diff_rows.get(row).and_then(|r| match (r, self.select_side) {
                (DiffRow::Pair { old_no, .. }, Side::Old) => *old_no,
                (DiffRow::Pair { new_no, .. }, Side::New) => *new_no,
                _ => None,
            })
        }
    }

    fn selected_lines(&self) -> String {
        let (from, to) = self.selection_range();
        let numbers: Vec<usize> = (from..=to).filter_map(|row| self.row_line(row)).collect();
        let label = match (numbers.first(), numbers.last()) {
            (Some(a), Some(b)) if a != b => format!("{a}-{b}"),
            (Some(a), _) => a.to_string(),
            _ => "1".to_string(),
        };
        if self.mode == Mode::Diff && self.select_side == Side::Old {
            format!("{label} in base")
        } else {
            label
        }
    }

    fn selection_text(&self) -> String {
        let (from, to) = self.selection_range();
        (from..=to)
            .filter_map(|row| {
                if self.mode == Mode::Browse {
                    self.content.get(row).cloned()
                } else {
                    self.diff_rows.get(row).and_then(|r| match (r, self.select_side) {
                        (DiffRow::Pair { old_no: Some(_), old, .. }, Side::Old) => Some(old.clone()),
                        (DiffRow::Pair { new_no: Some(_), new, .. }, Side::New) => Some(new.clone()),
                        _ => None,
                    })
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn copy_selection(&mut self) {
        if self.active_path.is_none() { return; }
        let (from, to) = self.selection_range();
        let text = self.selection_text();
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        use std::io::Write;
        let mut out = stdout();
        let _ = write!(out, "\x1b]52;c;{encoded}\x07");
        let _ = out.flush();
        self.status = format!("copied {} lines", to - from + 1);
        self.visual = false;
        self.anchor = None;
    }

    fn ask(&mut self) {
        if self.active_path.is_some() {
            let lines = self.selected_lines();
            self.prompt = Some(Prompt { text: String::new(), agent: 0, lines });
        }
    }

    fn send_prompt(&mut self) {
        let Some(prompt) = self.prompt.take() else { return };
        let Some(path) = self.active_path.clone() else { return };
        let agent = ["claude", "codex"][prompt.agent];
        let base = (self.mode == Mode::Diff).then(|| self.base.as_deref()).flatten();
        match core::inject(&self.worktree_path(), agent, &path, Some(&prompt.lines), base, &prompt.text) {
            Ok(()) => {
                self.status = format!("sent to {agent}");
                self.visual = false;
                self.anchor = None;
            }
            Err(e) => self.status = e,
        }
    }

    fn sync_agents(&mut self) {
        let wt = self.worktree_path();
        match core::sync_window(&wt, &self.config.claude, &self.config.codex, true) {
            Ok(sync) => {
                self.shell_pane = sync.shell;
                self.status = if sync.started.is_empty() {
                    "agent panes already open".to_string()
                } else {
                    format!("started {}", sync.started.join(" and "))
                };
            }
            Err(e) => self.status = e,
        }
    }

    fn open_pull_requests(&mut self) {
        let Some(repo) = self.repos.get(self.repo_index).map(|r| PathBuf::from(&r.repo)) else { return };
        match core::my_pull_requests(&repo) {
            Ok(pulls) if pulls.is_empty() => self.status = "no open pull requests of yours".into(),
            Ok(pulls) => {
                self.pulls = pulls;
                self.popup = Some(Popup::PullRequest);
                self.popup_index = 0;
            }
            Err(e) => self.status = e,
        }
    }

    fn create_worktree(&mut self) {
        let Some(branch) = self.new_branch.take() else { return };
        let Some(repo) = self.repos.get(self.repo_index).map(|r| PathBuf::from(&r.repo)) else { return };
        match core::create_worktree(&repo, &branch) {
            Ok(path) => {
                let session = match core::ensure_session(&path, &self.config.claude, &self.config.codex) {
                    Ok(session) => session,
                    Err(e) => {
                        self.status = e;
                        return;
                    }
                };
                let setup = core::post_create_commands(&repo);
                self.status = match (setup.is_empty(), core::session_shell(&session)) {
                    (true, _) => format!("created {branch}"),
                    (false, None) => format!("created {branch} · run setup yourself, no shell pane found"),
                    (false, Some(pane)) => match core::send_to_pane(&pane, &setup.join(" && ")) {
                        Ok(()) => format!("created {branch} · setup running in the shell pane"),
                        Err(e) => format!("created {branch} · {e}"),
                    },
                };
                if let Err(e) = core::switch_to(&session) { self.status = e; }
            }
            Err(e) => self.status = e,
        }
    }

    fn poll_watch(&mut self) {
        if self.watch_rx.as_ref().is_some_and(|rx| rx.try_recv().is_ok()) { self.dirty_since = Some(Instant::now()); }
        if self.dirty_since.is_some_and(|at| at.elapsed() >= Duration::from_millis(150)) {
            if let Some(rx) = &self.watch_rx { while rx.try_recv().is_ok() {} }
            self.dirty_since = None;
            if let Err(e) = self.refresh() { self.status = e; }
        }
    }

    fn popup_entries(&self) -> Vec<(usize, usize, String, bool)> {
        let mut entries = Vec::new();
        for (ri, repo) in self.repos.iter().enumerate() {
            for (wi, wt) in repo.worktrees.iter().enumerate() {
                let name = wt.branch.as_deref().unwrap_or(&wt.path);
                let open = self.open_worktrees.contains(Path::new(&wt.path));
                entries.push((ri, wi, format!("{}  {}", repo.name, name), open));
            }
        }
        entries
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if let Some(prompt) = &mut self.prompt {
            match key.code {
                KeyCode::Esc => self.prompt = None,
                KeyCode::Tab => prompt.agent = 1 - prompt.agent,
                KeyCode::Enter => self.send_prompt(),
                KeyCode::Backspace => { prompt.text.pop(); }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => prompt.text.push(c),
                _ => {}
            }
            return;
        }
        if let Some(name) = &mut self.new_branch {
            match key.code {
                KeyCode::Esc => self.new_branch = None,
                KeyCode::Enter => self.create_worktree(),
                KeyCode::Backspace => { name.pop(); }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => name.push(c),
                _ => {}
            }
            return;
        }
        if self.popup.is_some() {
            let len = match self.popup { Some(Popup::Worktree) => self.popup_entries().len() + 2, Some(Popup::Base) => self.branches.len(), Some(Popup::PullRequest) => self.pulls.len(), None => 0 };
            match key.code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Up | KeyCode::Char('k') => self.popup_index = self.popup_index.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => self.popup_index = (self.popup_index + 1).min(len.saturating_sub(1)),
                KeyCode::Enter => {
                    match self.popup.take() {
                        Some(Popup::Worktree) => {
                            let entries = self.popup_entries();
                            if self.popup_index == entries.len() {
                                self.new_branch = Some(String::new());
                                return;
                            }
                            if self.popup_index > entries.len() {
                                self.open_pull_requests();
                                return;
                            }
                            let entry = entries.get(self.popup_index).cloned();
                            let target = entry.map(|(ri, wi, _, _)| (ri, wi));
                            if let Some(wt) = target
                                .and_then(|(ri, wi)| self.repos.get(ri)?.worktrees.get(wi))
                                .map(|wt| PathBuf::from(&wt.path))
                            {
                                if let Err(e) = core::ensure_session(&wt, &self.config.claude, &self.config.codex)
                                    .and_then(|session| core::switch_to(&session))
                                {
                                    self.status = e;
                                }
                            }
                        }
                        Some(Popup::Base) => if let Some(base) = self.branches.get(self.popup_index).cloned() { self.base = Some(base); self.active_path = None; if let Err(e) = self.refresh() { self.status = e; } },
                        Some(Popup::PullRequest) => if let Some(pr) = self.pulls.get(self.popup_index).cloned() {
                            self.new_branch = Some(pr.branch);
                            self.create_worktree();
                        },
                        None => {}
                    }
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc => { self.visual = false; self.anchor = None; }
            KeyCode::Char('v') if self.focus == Focus::Content => { self.visual = true; self.anchor = Some(self.cursor); }
            KeyCode::Char('y') if self.focus == Focus::Content => self.copy_selection(),
            KeyCode::Tab => self.focus = if self.focus == Focus::Sidebar { Focus::Content } else { Focus::Sidebar },
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::PageDown => self.move_cursor(10),
            KeyCode::PageUp => self.move_cursor(-10),
            KeyCode::Left | KeyCode::Char('h') => if self.focus == Focus::Sidebar { self.activate(false) } else if self.mode == Mode::Browse { self.horizontal = self.horizontal.saturating_sub(4) } else { self.select_side = Side::Old },
            KeyCode::Right | KeyCode::Char('l') => if self.focus == Focus::Sidebar { self.activate(true) } else if self.mode == Mode::Browse { self.horizontal += 4 } else { self.select_side = Side::New },
            KeyCode::Enter => if self.focus == Focus::Sidebar { self.activate(true) } else if let Some(DiffRow::Fold { old_start, new_start, .. }) = self.diff_rows.get(self.cursor) { self.expanded.insert((*old_start, *new_start)); let (scroll, cursor) = (self.scroll, self.cursor); if let Some(path) = self.active_path.clone() { let _ = self.open(&path); } self.scroll = scroll; self.cursor = cursor; },
            KeyCode::Char('d') => self.toggle_mode(),
            KeyCode::Char('w') => { self.open_worktrees = core::session_worktrees(); self.popup = Some(Popup::Worktree); self.popup_index = 0; },
            KeyCode::Char('n') => self.new_branch = Some(String::new()),
            KeyCode::Char('r') => self.fetch_and_refresh(),
            KeyCode::Char('A') => self.sync_agents(),
            KeyCode::Char('b') if self.mode == Mode::Diff => { self.popup = Some(Popup::Base); self.popup_index = self.base.as_ref().and_then(|b| self.branches.iter().position(|v| v == b)).unwrap_or(0); },
            KeyCode::Char('a') => self.ask(),
            KeyCode::Char('[') => self.sidebar_width = self.sidebar_width.saturating_sub(2).max(20),
            KeyCode::Char(']') => self.sidebar_width += 2,
            KeyCode::Char('{') if self.mode == Mode::Diff => self.split_pct = self.split_pct.saturating_sub(5).max(15),
            KeyCode::Char('}') if self.mode == Mode::Diff => self.split_pct = (self.split_pct + 5).min(85),
            _ => {}
        }
    }

    fn clicked_row(&self, term_row: u16) -> Option<usize> {
        self.row_map
            .get((term_row as usize).saturating_sub(1))
            .copied()
            .or_else(|| self.row_map.last().copied())
    }

    fn split_col(&self) -> Option<u16> {
        (self.mode == Mode::Diff && self.active_path.is_some())
            .then(|| self.sidebar_width + 1 + (self.view_width as u16).saturating_mul(self.split_pct) / 100)
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent, size: Rect) {
        let sidebar = self.sidebar_width;
        match mouse.kind {
            MouseEventKind::Up(MouseButton::Left) => self.dragging = None,
            MouseEventKind::ScrollDown => { self.focus = if mouse.column < sidebar { Focus::Sidebar } else { Focus::Content }; self.move_cursor(3); }
            MouseEventKind::ScrollUp => { self.focus = if mouse.column < sidebar { Focus::Sidebar } else { Focus::Content }; self.move_cursor(-3); }
            MouseEventKind::Down(MouseButton::Left) if mouse.row > 0 && mouse.row < size.height.saturating_sub(1) => {
                if mouse.column + 1 == sidebar || mouse.column == sidebar {
                    self.dragging = Some(Drag::Sidebar);
                } else if self.split_col().is_some_and(|col| mouse.column >= col.saturating_sub(1) && mouse.column <= col + 1) {
                    self.dragging = Some(Drag::Split);
                } else if mouse.column < sidebar {
                    self.focus = Focus::Sidebar;
                    self.selected = (mouse.row as usize - 1 + self.selected.saturating_sub((size.height as usize).saturating_sub(3))).min(self.nodes.len().saturating_sub(1));
                    self.activate(true);
                } else if let Some(row) = self.clicked_row(mouse.row) {
                    self.focus = Focus::Content;
                    self.visual = false;
                    if let Some(col) = self.split_col() {
                        self.select_side = if mouse.column < col { Side::Old } else { Side::New };
                    }
                    self.cursor = row;
                    self.anchor = Some(row);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match self.dragging {
                Some(Drag::Sidebar) => self.sidebar_width = mouse.column.clamp(20, size.width.saturating_sub(40)),
                Some(Drag::Split) => {
                    if self.view_width > 0 {
                        let rel = mouse.column.saturating_sub(sidebar) as usize * 100 / self.view_width;
                        self.split_pct = (rel as u16).clamp(15, 85);
                    }
                }
                None => {
                    if self.focus == Focus::Content && mouse.column >= sidebar && mouse.row > 0 {
                        if let Some(row) = self.clicked_row(mouse.row) {
                            self.cursor = row;
                        }
                    }
                }
            },
            MouseEventKind::Down(MouseButton::Right) if mouse.column >= sidebar && mouse.row > 0 => {
                self.focus = Focus::Content;
                if let Some(row) = self.clicked_row(mouse.row) {
                    let (from, to) = self.selection_range();
                    if self.anchor.is_none() || row < from || row > to {
                        self.cursor = row;
                        self.anchor = None;
                    }
                }
                self.ask();
            }
            _ => {}
        }
    }
}

fn render_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.mode == Mode::Diff && app.nodes.is_empty() {
        frame.render_widget(Paragraph::new(format!("No changes against {}", app.base.as_deref().unwrap_or("none"))).style(Style::default().fg(Color::DarkGray)), area);
        return;
    }
    let items: Vec<ListItem> = app.nodes.iter().map(|node| {
        let (depth, line) = match node {
            Node::Dir { label, depth, open, .. } => (*depth, Line::from(vec![Span::styled("│ ".repeat(*depth), Style::default().fg(Color::Rgb(35, 35, 35))), Span::styled(format!("{} ", if *open { "▾" } else { "▸" }), Style::default().fg(Color::Rgb(90, 124, 166))), Span::styled(label.clone(), Style::default().fg(Color::DarkGray))])),
            Node::File { label, path, depth, stats } => {
                let mut spans = vec![Span::styled("│ ".repeat(*depth), Style::default().fg(Color::Rgb(35,35,35))), Span::styled("● ", Style::default().fg(extension_color(path))), Span::raw(label.clone())];
                if let Some((a, d)) = stats {
                    let value = format!("+{} −{}", a.map(|v| v.to_string()).unwrap_or_else(|| "?".into()), d.map(|v| v.to_string()).unwrap_or_else(|| "?".into()));
                    let used = depth * 2 + 2 + label.chars().count() + value.chars().count();
                    spans.push(Span::raw(" ".repeat((app.sidebar_width as usize).saturating_sub(4).saturating_sub(used))));
                    spans.push(Span::styled(value, Style::default().fg(Color::DarkGray)));
                }
                (*depth, Line::from(spans))
            }
        };
        let _ = depth;
        ListItem::new(line)
    }).collect();
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(List::new(items).highlight_style(Style::default().fg(app.accent).add_modifier(Modifier::BOLD)), area, &mut state);
}

fn colored_crop(parts: &[Vec<(String, Color)>], line: Option<usize>, start: usize, width: usize, background: Option<Color>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut skipped = start;
    let mut left = width;
    if let Some(parts) = line.and_then(|n| parts.get(n.saturating_sub(1))) {
        for (text, color) in parts {
            let chars = text.chars().count();
            if skipped >= chars { skipped -= chars; continue; }
            let value: String = text.chars().skip(skipped).take(left).collect();
            skipped = 0;
            left = left.saturating_sub(value.chars().count());
            let mut style = Style::default().fg(*color);
            if let Some(bg) = background { style = style.bg(bg); }
            spans.push(Span::styled(value, style));
            if left == 0 { break; }
        }
    }
    if left > 0 {
        let mut style = Style::default();
        if let Some(bg) = background { style = style.bg(bg); }
        spans.push(Span::styled(" ".repeat(left), style));
    }
    spans
}

fn render_content(frame: &mut Frame, app: &mut App, area: Rect) {
    app.view_height = area.height as usize;
    app.view_width = area.width as usize;
    if app.active_path.is_none() {
        frame.render_widget(Paragraph::new("Select a file").style(Style::default().fg(Color::DarkGray)), area);
        return;
    }
    let height = area.height as usize;
    let cursor_bg = Color::Rgb(30, 34, 42);
    let select_bg = Color::Rgb(48, 54, 61);
    let focus_content = app.focus == Focus::Content;
    let cursor = app.cursor;
    let accent = app.accent;
    let on_cursor = move |index: usize| focus_content && index == cursor;
    let selection = app.anchor.map(|a| (a.min(app.cursor), a.max(app.cursor)));
    let row_bg = move |index: usize| {
        if on_cursor(index) {
            Some(cursor_bg)
        } else if selection.is_some_and(|(from, to)| index >= from && index <= to) {
            Some(select_bg)
        } else {
            None
        }
    };
    if app.mode == Mode::Browse {
        let gutter = app.content.len().max(1).to_string().len();
        let lines: Vec<Line> = (app.scroll..app.content.len()).take(height).map(|index| {
            let bg = row_bg(index);
            let mut number_style = if on_cursor(index) { Style::default().fg(app.accent) } else { Style::default().fg(Color::DarkGray) };
            if let Some(bg) = bg { number_style = number_style.bg(bg); }
            let mut spans = vec![Span::styled(format!("{:>gutter$} │ ", index + 1), number_style)];
            if let Some(parts) = app.highlighted.get(index) {
                let mut skipped = app.horizontal;
                let mut left = area.width.saturating_sub((gutter + 3) as u16) as usize;
                for (text, color) in parts {
                    let chars = text.chars().count();
                    if skipped >= chars { skipped -= chars; continue; }
                    let value: String = text.chars().skip(skipped).take(left).collect();
                    skipped = 0;
                    left = left.saturating_sub(value.chars().count());
                    spans.push(Span::styled(value, Style::default().fg(*color)));
                    if left == 0 { break; }
                }
            }
            let mut line = Line::from(spans);
            if let Some(bg) = row_bg(index) { line = line.style(Style::default().bg(bg)); }
            line
        }).collect();
        app.row_map = (app.scroll..app.content.len()).take(height).collect();
        app.rendered_rows = app.row_map.len().max(1);
        frame.render_widget(Paragraph::new(lines), area);
    } else {
        let old_width = area.width as usize * app.split_pct as usize / 100;
        let old_text = old_width.saturating_sub(7).max(1);
        let new_text = (area.width as usize).saturating_sub(old_width).saturating_sub(9).max(1);
        let divider = Style::default().fg(Color::Rgb(51, 58, 66));
        let in_sel = move |index: usize| selection.is_some_and(|(from, to)| index >= from && index <= to);
        let side = app.select_side;
        let mut lines: Vec<Line> = Vec::new();
        let mut row_map: Vec<usize> = Vec::new();
        let mut index = app.scroll;
        while lines.len() < height && index < app.diff_rows.len() {
            let cur_bg = on_cursor(index).then_some(cursor_bg);
            let old_sel = (in_sel(index) && side == Side::Old).then_some(select_bg);
            let new_sel = (in_sel(index) && side == Side::New).then_some(select_bg);
            let old_row_bg = cur_bg.or(old_sel);
            let new_row_bg = cur_bg.or(new_sel);
            let bg = cur_bg.or(in_sel(index).then_some(select_bg));
            match &app.diff_rows[index] {
                DiffRow::Fold { count, .. } => {
                    let style = match bg {
                        Some(b) => Style::default().fg(if on_cursor(index) { accent } else { Color::DarkGray }).bg(b).add_modifier(Modifier::ITALIC),
                        None => Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    };
                    let label = format!("· {count} unchanged lines · Enter expands ·");
                    lines.push(Line::from(vec![
                        Span::styled(format!("{label:<width$}", width = old_width), style),
                        Span::styled("│", divider),
                    ]));
                    row_map.push(index);
                }
                DiffRow::Pair { old_no, old, new_no, new, deleted, added } => {
                    let old_style = if *deleted { Style::default().bg(Color::Rgb(45, 18, 20)).fg(Color::Rgb(235, 190, 190)) } else { Style::default() };
                    let new_style = if *added { Style::default().bg(Color::Rgb(15, 42, 24)).fg(Color::Rgb(190, 235, 195)) } else { Style::default() };
                    let old_bg = deleted.then_some(Color::Rgb(45, 18, 20));
                    let new_bg = added.then_some(Color::Rgb(15, 42, 24));
                    let old_gutter = match old_row_bg { Some(b) => Style::default().fg(if on_cursor(index) { accent } else { Color::Gray }).bg(b), None => old_style };
                    let new_gutter = match new_row_bg { Some(b) => Style::default().fg(if on_cursor(index) { accent } else { Color::Gray }).bg(b), None => new_style };
                    let wraps = old.chars().count().div_ceil(old_text).max(new.chars().count().div_ceil(new_text)).max(1);
                    for wrap in 0..wraps {
                        if lines.len() >= height { break; }
                        let old_label = if wrap == 0 { old_no.map(|v| v.to_string()).unwrap_or_default() } else { String::new() };
                        let new_label = if wrap == 0 { new_no.map(|v| v.to_string()).unwrap_or_default() } else { String::new() };
                        let mut spans = vec![Span::styled(format!("{old_label:>4} │ "), old_gutter)];
                        spans.extend(colored_crop(&app.old_highlighted, *old_no, wrap * old_text, old_text, old_sel.or(old_bg).or(cur_bg)));
                        spans.push(Span::styled("│", divider));
                        spans.push(Span::styled(format!(" {new_label:>4} │ "), new_gutter));
                        spans.extend(colored_crop(&app.new_highlighted, *new_no, wrap * new_text, new_text, new_sel.or(new_bg).or(cur_bg)));
                        lines.push(Line::from(spans));
                        row_map.push(index);
                    }
                }
            }
            index += 1;
        }
        app.rendered_rows = index.saturating_sub(app.scroll).max(1);
        app.row_map = row_map;
        frame.render_widget(Paragraph::new(lines), area);
    }
}

fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let v = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage((100 - height) / 2), Constraint::Percentage(height), Constraint::Percentage((100 - height) / 2)]).split(area);
    Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage((100 - width) / 2), Constraint::Percentage(width), Constraint::Percentage((100 - width) / 2)]).split(v[1])[1]
}

fn render_popup(frame: &mut Frame, app: &App, area: Rect) {
    let Some(popup) = &app.popup else { return };
    let (title, mut entries): (&str, Vec<(String, Option<String>)>) = match popup {
        Popup::Worktree => (" Worktrees ", app.popup_entries().into_iter().map(|v| (v.2, v.3.then_some("open".to_string()))).collect()),
        Popup::Base => (" Base ", app.branches.iter().map(|b| (b.clone(), None)).collect()),
        Popup::PullRequest => (" My pull requests ", app.pulls.iter().map(|p| (p.label(), None)).collect()),
    };
    if matches!(popup, Popup::Worktree) {
        entries.push(("+  new worktree".to_string(), None));
        entries.push(("↓  from my pull requests".to_string(), None));
    }
    let popup_area = centered(70, 60, area);
    frame.render_widget(Clear, popup_area);
    let items: Vec<ListItem> = entries
        .into_iter()
        .map(|(label, holder)| match holder {
            Some(state) => ListItem::new(format!("{label}  · {state}")),
            None => ListItem::new(label),
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.popup_index));
    frame.render_stateful_widget(List::new(items).block(Block::bordered().border_type(BorderType::Rounded).title(title).border_style(Style::default().fg(app.accent))).highlight_style(Style::default().fg(app.accent).add_modifier(Modifier::BOLD)), popup_area, &mut state);
}

fn draw(frame: &mut Frame, app: &mut App) {
    let vertical = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(1), Constraint::Length(1)]).split(frame.area());
    app.sidebar_width = app.sidebar_width.clamp(20, frame.area().width.saturating_sub(40).max(20));
    let body = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Length(app.sidebar_width), Constraint::Min(1)]).split(vertical[0]);
    let frame_style = |focused: bool| if focused { Style::default().fg(app.accent) } else { Style::default().fg(Color::Rgb(51, 58, 66)) };
    let title_style = |focused: bool| if focused { Style::default().fg(app.accent) } else { Style::default().fg(Color::Rgb(145, 152, 161)) };
    let repo = app.repos.get(app.repo_index).map(|r| r.name.clone()).unwrap_or_default();
    let branch = app.worktree().and_then(|w| w.branch.clone()).unwrap_or_else(|| "detached".into());
    let base = app.base.as_deref().unwrap_or("none").to_string();
    let content_title = match (&app.active_path, app.mode) {
        (Some(path), Mode::Browse) => format!(" {path} "),
        (Some(path), Mode::Diff) => format!(" {path} · base {base} "),
        (None, Mode::Browse) => " Browse ".to_string(),
        (None, Mode::Diff) => format!(" Diff · base {base} "),
    };
    let sidebar_block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(frame_style(app.focus == Focus::Sidebar))
        .title(Span::styled(format!(" {repo} · {branch} "), title_style(app.focus == Focus::Sidebar)));
    let content_block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(frame_style(app.focus == Focus::Content))
        .title(Span::styled(content_title, title_style(app.focus == Focus::Content)));
    let sidebar_inner = sidebar_block.inner(body[0]);
    let content_inner = content_block.inner(body[1]);
    frame.render_widget(sidebar_block, body[0]);
    frame.render_widget(content_block, body[1]);
    render_sidebar(frame, app, sidebar_inner);
    render_content(frame, &mut *app, content_inner);
    // A status set at startup used to sit there forever, hiding the key hints.
    if app.status != app.status_shown {
        app.status_shown = app.status.clone();
        app.status_at = Instant::now();
    }
    let faded = app.status_at.elapsed() > Duration::from_secs(6);
    let status = if let Some(name) = &app.new_branch { format!("New branch: {name}") } else if let Some(prompt) = &app.prompt { format!("Ask {} [{}] {}", prompt.lines, ["claude", "codex"][prompt.agent], prompt.text) } else if app.status.is_empty() || faded { "q quit · d mode · w worktree · n new · r fetch · b base · v select · y copy · a ask · A panes".into() } else { app.status.clone() };
    let status_style = if app.prompt.is_some() || app.new_branch.is_some() { Style::default().fg(app.accent) } else { Style::default().fg(Color::DarkGray) };
    frame.render_widget(Paragraph::new(status).style(status_style), vertical[1]);
    render_popup(frame, app, frame.area());
}

fn run(mut app: App) -> io::Result<()> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, event::EnableMouseCapture)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let result = loop {
        app.poll_watch();
        terminal.draw(|frame| draw(frame, &mut app))?;
        if app.quit { break Ok(()); }
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == event::KeyEventKind::Press => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse, terminal.size()?.into()),
                _ => {}
            }
        }
    };
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, event::DisableMouseCapture)?;
    terminal.show_cursor()?;
    result
}

fn start() -> Result<(), String> {
    let invocation = core::parse_args(std::env::args())?;
    let config = core::load_config(Path::new(&invocation.config))?;
    match invocation.command {
        core::Action::View { panes, close_workspace } => {
            let mut app = App::new(config)?;
            if panes {
                app.sync_agents();
            }
            let result = run(app).map_err(|e| e.to_string());
            if close_workspace {
                core::close_workspace();
            }
            result
        }
        core::Action::Say { agent, text } => {
            println!("{}", relay::say(&config, &agent, &text)?);
            Ok(())
        }
        core::Action::Ask { agent, text, fresh } => {
            println!("{}", relay::ask(&config, &agent, &text, fresh)?);
            Ok(())
        }
    }
}

fn main() {
    if let Err(e) = start() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compresses_single_child_directories() {
        let mut tree = Tree::default();
        tree.insert("a/b/c/file.rs");
        let mut nodes = Vec::new();
        compress_tree(&tree, "", 0, &HashSet::new(), &HashMap::new(), true, &mut nodes);
        assert!(matches!(&nodes[0], Node::Dir { label, .. } if label == "a/b/c/"));
    }

    #[test]
    fn collapses_long_unchanged_regions() {
        let old = (0..12).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let rows = collapse_segments(&old, &old, &HashSet::new());
        assert!(rows.iter().any(|r| matches!(r, DiffRow::Fold { count: 6, .. })));
    }

    #[test]
    fn expands_folds_by_segment_start() {
        let old = (0..12).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let folded = collapse_segments(&old, &old, &HashSet::new());
        let key = folded.iter().find_map(|r| match r { DiffRow::Fold { old_start, new_start, .. } => Some((*old_start, *new_start)), _ => None }).unwrap();
        let rows = collapse_segments(&old, &old, &HashSet::from([key]));
        assert!(!rows.iter().any(|r| matches!(r, DiffRow::Fold { .. })));
    }

    #[test]
    fn aligns_changed_lines() {
        let rows = collapse_segments("a\nb\nc", "a\nx\nc", &HashSet::new());
        assert!(rows.iter().any(|r| matches!(r, DiffRow::Pair { deleted: true, added: true, .. })));
    }
}
