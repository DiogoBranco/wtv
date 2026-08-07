import "./style.css";
import { EditorView, lineNumbers } from "@codemirror/view";
import { EditorState, type Extension } from "@codemirror/state";
import { LanguageDescription, syntaxHighlighting } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { MergeView } from "@codemirror/merge";
import { oneDarkHighlightStyle } from "@codemirror/theme-one-dark";

interface Worktree {
  path: string;
  branch: string | null;
  head: string;
}

interface RepoWorktrees {
  repo: string;
  name: string;
  worktrees: Worktree[];
}

interface ChangedFile {
  path: string;
  additions: number | null;
  deletions: number | null;
}

interface SideContent {
  exists: boolean;
  content: string | null;
}

const worktreeSelect = document.getElementById("worktree-select") as HTMLSelectElement;
const browseButton = document.getElementById("mode-browse") as HTMLButtonElement;
const diffButton = document.getElementById("mode-diff") as HTMLButtonElement;
const baseSelect = document.getElementById("base-select") as HTMLSelectElement;
const sidebar = document.getElementById("sidebar")!;
const content = document.getElementById("content")!;
const accentPicker = document.getElementById("accent-picker") as HTMLInputElement;
const prompt = document.getElementById("prompt") as HTMLFormElement;
const question = document.getElementById("question") as HTMLInputElement;
const agent = document.getElementById("agent") as HTMLSelectElement;
const status = document.getElementById("status")!;

let mode: "browse" | "diff" = "browse";
let view: EditorView | null = null;
let merge: MergeView | null = null;
let activeEntry: HTMLElement | null = null;
let activePath: string | null = null;
let socket: WebSocket | null = null;
let promptLines: string | null = null;
let refreshTimer = 0;

const blackTheme = EditorView.theme(
  {
    "&": { backgroundColor: "#0d1117", color: "#c8c8c8", height: "100%" },
    ".cm-gutters": { backgroundColor: "#0d1117", color: "#555", border: "none" },
    ".cm-scroller": { fontFamily: "inherit" },
    "&.cm-focused": { outline: "none" },
  },
  { dark: true },
);

const LANG_COLORS: Record<string, string> = {
  py: "#3572a5",
  ts: "#3178c6",
  tsx: "#3178c6",
  js: "#f1e05a",
  jsx: "#f1e05a",
  rs: "#dea584",
  go: "#00add8",
  java: "#b07219",
  rb: "#701516",
  c: "#555555",
  h: "#555555",
  cpp: "#f34b7d",
  cs: "#178600",
  php: "#4f5d95",
  swift: "#f05138",
  kt: "#a97bff",
  sh: "#89e051",
  sql: "#e38c00",
  html: "#e34c26",
  css: "#663399",
  scss: "#c6538c",
  svelte: "#ff3e00",
  vue: "#41b883",
  md: "#519aba",
  json: "#cbcb41",
  yml: "#cb171e",
  yaml: "#cb171e",
  toml: "#9c4221",
  lock: "#9c4221",
  svg: "#ffb13b",
  png: "#a074c4",
  jpg: "#a074c4",
  ipynb: "#da5b0b",
};

function svgIcon(path: string, color: string): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("class", "icon");
  const shape = document.createElementNS("http://www.w3.org/2000/svg", "path");
  shape.setAttribute("d", path);
  shape.setAttribute("fill", color);
  svg.appendChild(shape);
  return svg;
}

function fileIcon(name: string): SVGSVGElement {
  const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : "";
  return svgIcon(
    "M4 1.5h4.8L12.5 5.2V14a.6.6 0 0 1-.6.6H4.6A.6.6 0 0 1 4 14V2.1a.6.6 0 0 1 .6-.6z M8.8 1.5v3.7h3.7",
    LANG_COLORS[ext] ?? "#5a5a5a",
  );
}

function folderIcon(): SVGSVGElement {
  return svgIcon(
    "M1.5 4a1 1 0 0 1 1-1h3.4l1.5 1.8h6.1a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1V4z",
    "#767676",
  );
}

function baseExtensions(): Extension[] {
  return [
    lineNumbers(),
    blackTheme,
    syntaxHighlighting(oneDarkHighlightStyle),
    EditorState.readOnly.of(true),
    EditorView.editable.of(false),
  ];
}

async function api<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status}`);
  return res.json();
}

function query(values: Record<string, string>): string {
  return new URLSearchParams(values).toString();
}

function current(key: { worktree: string; mode: string; path?: string; base?: string }): boolean {
  return key.worktree === worktreeSelect.value
    && key.mode === mode
    && (key.path === undefined || key.path === activePath)
    && (key.base === undefined || key.base === baseSelect.value);
}

function clearEditor() {
  view?.destroy();
  merge?.destroy();
  view = null;
  merge = null;
  content.innerHTML = "";
}

async function language(path: string): Promise<Extension> {
  const desc = LanguageDescription.matchFilename(languages, path);
  return desc ? (await desc.load()) : [];
}

async function loadAccent() {
  const { accent } = await api<{ accent: string }>("/api/config");
  document.documentElement.style.setProperty("--accent", accent);
  accentPicker.value = accent;
}

accentPicker.addEventListener("change", async () => {
  document.documentElement.style.setProperty("--accent", accentPicker.value);
  await fetch("/api/config", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ accent: accentPicker.value }),
  });
});

async function loadWorktrees() {
  const repos = await api<RepoWorktrees[]>("/api/worktrees");
  worktreeSelect.innerHTML = "";
  for (const repo of repos) {
    const group = document.createElement("optgroup");
    group.label = repo.name;
    for (const wt of repo.worktrees) {
      const option = document.createElement("option");
      option.value = wt.path;
      option.textContent = wt.branch ?? wt.path.split("/").pop() ?? wt.path;
      group.appendChild(option);
    }
    worktreeSelect.appendChild(group);
  }
  if (worktreeSelect.value) await selectWorktree();
}

async function selectWorktree() {
  activePath = null;
  activeEntry = null;
  clearEditor();
  connectWatch();
  if (mode === "diff") await loadBranches();
  await loadList(false);
}

worktreeSelect.addEventListener("change", selectWorktree);

type Tree = Map<string, Tree | null>;

function buildTree(paths: string[]): Tree {
  const root: Tree = new Map();
  for (const path of paths) {
    const parts = path.split("/");
    let node = root;
    for (let i = 0; i < parts.length; i++) {
      if (i === parts.length - 1) {
        node.set(parts[i], null);
      } else {
        if (!(node.get(parts[i]) instanceof Map)) node.set(parts[i], new Map());
        node = node.get(parts[i]) as Tree;
      }
    }
  }
  return root;
}

interface TreeOptions {
  stats?: Map<string, ChangedFile>;
  open?: boolean;
}

function renderTree(tree: Tree, dir: string, container: HTMLElement, opts: TreeOptions) {
  const entries = [...tree.entries()].sort((a, b) => {
    const aDir = a[1] instanceof Map ? 0 : 1;
    const bDir = b[1] instanceof Map ? 0 : 1;
    return aDir - bDir || a[0].localeCompare(b[0]);
  });
  for (let [name, child] of entries) {
    let path = dir ? `${dir}/${name}` : name;
    if (child instanceof Map) {
      while (child.size === 1) {
        const [childName, grandchild] = [...child.entries()][0];
        if (!(grandchild instanceof Map)) break;
        name = `${name}/${childName}`;
        path = `${path}/${childName}`;
        child = grandchild;
      }
      const details = document.createElement("details");
      if (opts.open) details.open = true;
      const summary = document.createElement("summary");
      summary.append(folderIcon(), `${name}/`);
      details.appendChild(summary);
      const inner = document.createElement("div");
      inner.className = "children";
      renderTree(child, path, inner, opts);
      details.appendChild(inner);
      container.appendChild(details);
    } else {
      const entry = document.createElement("div");
      entry.className = "file";
      entry.dataset.path = path;
      const label = document.createElement("span");
      label.append(fileIcon(name), name);
      entry.appendChild(label);
      const file = opts.stats?.get(path);
      if (file) {
        const stats = document.createElement("span");
        stats.className = "stats";
        stats.textContent = file.additions === null ? "binary" : `+${file.additions} −${file.deletions}`;
        entry.appendChild(stats);
      }
      const target = path;
      entry.addEventListener("click", () => openPath(target, entry, false));
      container.appendChild(entry);
    }
  }
}

async function loadList(preserve: boolean) {
  const key = { worktree: worktreeSelect.value, mode, base: baseSelect.value };
  if (!key.worktree) return;
  const oldPath = preserve ? activePath : null;
  if (mode === "browse") {
    const paths = await api<string[]>(`/api/files?${query({ worktree: key.worktree })}`);
    if (!current(key)) return;
    sidebar.innerHTML = "";
    renderTree(buildTree(paths), "", sidebar, {});
  } else {
    const files = await api<ChangedFile[]>(`/api/changed?${query({ worktree: key.worktree, base: key.base })}`);
    if (!current(key)) return;
    sidebar.innerHTML = "";
    if (files.length === 0) {
      sidebar.innerHTML = `<p class="empty">No changes against ${key.base}</p>`;
    } else {
      renderTree(buildTree(files.map((f) => f.path)), "", sidebar, {
        stats: new Map(files.map((f) => [f.path, f])),
        open: true,
      });
    }
  }
  activeEntry = null;
  if (oldPath) {
    const entry = [...sidebar.querySelectorAll<HTMLElement>("[data-path]")].find((v) => v.dataset.path === oldPath)
      ?? null;
    if (entry) {
      entry.classList.add("active");
      activeEntry = entry;
    } else {
      activePath = null;
      clearEditor();
    }
  } else {
    activePath = null;
    clearEditor();
  }
}

async function openPath(path: string, entry: HTMLElement | null, preserveScroll: boolean) {
  activeEntry?.classList.remove("active");
  entry?.classList.add("active");
  activeEntry = entry;
  activePath = path;
  if (mode === "browse") await openFile(path, preserveScroll);
  else await openDiff(path, preserveScroll);
}

async function openFile(path: string, preserveScroll: boolean) {
  const key = { worktree: worktreeSelect.value, mode, path };
  const scroll = preserveScroll ? view?.scrollDOM.scrollTop ?? 0 : 0;
  const { content: text } = await api<{ content: string | null }>(
    `/api/file?${query({ worktree: key.worktree, path })}`,
  );
  const lang = text === null ? [] : await language(path);
  if (!current(key)) return;
  clearEditor();
  if (text === null) {
    content.innerHTML = '<p class="binary">binary file</p>';
    return;
  }
  view = new EditorView({
    state: EditorState.create({ doc: text, extensions: [...baseExtensions(), lang] }),
    parent: content,
  });
  requestAnimationFrame(() => {
    if (view) view.scrollDOM.scrollTop = scroll;
  });
}

async function openDiff(path: string, preserveScroll: boolean) {
  const key = { worktree: worktreeSelect.value, mode, path, base: baseSelect.value };
  const oldScroll = preserveScroll ? merge?.a.scrollDOM.scrollTop ?? 0 : 0;
  const newScroll = preserveScroll ? merge?.b.scrollDOM.scrollTop ?? 0 : 0;
  const data = await api<{ old: SideContent; new: SideContent }>(
    `/api/diff-file?${query({ worktree: key.worktree, base: key.base, path })}`,
  );
  const lang = await language(path);
  if (!current(key)) return;
  clearEditor();
  if ((data.old.exists && data.old.content === null) || (data.new.exists && data.new.content === null)) {
    content.innerHTML = '<p class="binary">binary file</p>';
    return;
  }
  merge = new MergeView({
    a: { doc: data.old.content ?? "", extensions: [...baseExtensions(), lang] },
    b: { doc: data.new.content ?? "", extensions: [...baseExtensions(), lang] },
    parent: content,
    collapseUnchanged: { margin: 3, minSize: 4 },
  });
  requestAnimationFrame(() => {
    if (merge) {
      merge.a.scrollDOM.scrollTop = oldScroll;
      merge.b.scrollDOM.scrollTop = newScroll;
    }
  });
}

async function loadBranches() {
  const worktree = worktreeSelect.value;
  const data = await api<{ default: string | null; branches: string[] }>(
    `/api/branches?${query({ worktree })}`,
  );
  if (worktree !== worktreeSelect.value || mode !== "diff") return;
  baseSelect.innerHTML = "";
  for (const branch of data.branches) {
    const option = document.createElement("option");
    option.value = branch;
    option.textContent = branch;
    baseSelect.appendChild(option);
  }
  if (data.default) baseSelect.value = data.default;
}

async function setMode(next: "browse" | "diff") {
  if (mode === next) return;
  mode = next;
  browseButton.classList.toggle("active", mode === "browse");
  diffButton.classList.toggle("active", mode === "diff");
  baseSelect.hidden = mode !== "diff";
  activePath = null;
  activeEntry = null;
  clearEditor();
  if (mode === "diff") await loadBranches();
  await loadList(false);
}

browseButton.addEventListener("click", () => setMode("browse"));
diffButton.addEventListener("click", () => setMode("diff"));
baseSelect.addEventListener("change", () => {
  activePath = null;
  loadList(false);
});

function connectWatch() {
  socket?.close();
  if (!worktreeSelect.value) return;
  const protocol = location.protocol === "https:" ? "wss:" : "ws:";
  socket = new WebSocket(`${protocol}//${location.host}/api/watch?${query({ worktree: worktreeSelect.value })}`);
  socket.addEventListener("message", () => {
    window.clearTimeout(refreshTimer);
    refreshTimer = window.setTimeout(refresh, 50);
  });
}

async function refresh() {
  const path = activePath;
  await loadList(true);
  if (path && activePath === path) await openPath(path, activeEntry, true);
}

function selectionFor(editor: EditorView): string | null {
  const range = editor.state.selection.main;
  if (range.empty) return null;
  const from = editor.state.doc.lineAt(range.from).number;
  let to = editor.state.doc.lineAt(range.to).number;
  if (range.to > range.from && range.to === editor.state.doc.line(to).from) to--;
  return from === to ? `${from}` : `${from}-${to}`;
}

content.addEventListener("contextmenu", (event) => {
  if (!activePath) return;
  const target = event.target as HTMLElement;
  const editor = target.closest(".cm-editor");
  if (!editor) return;
  event.preventDefault();
  if (view && view.dom === editor) promptLines = selectionFor(view);
  else if (merge?.a.dom === editor) promptLines = selectionFor(merge.a);
  else if (merge?.b.dom === editor) promptLines = selectionFor(merge.b);
  else promptLines = null;
  prompt.hidden = false;
  prompt.style.left = `${event.clientX}px`;
  prompt.style.top = `${event.clientY}px`;
  question.value = "";
  question.focus();
});

document.addEventListener("pointerdown", (event) => {
  if (!prompt.hidden && !prompt.contains(event.target as Node)) prompt.hidden = true;
});

prompt.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!activePath) return;
  const body = {
    worktree: worktreeSelect.value,
    agent: agent.value,
    file: activePath,
    lines: promptLines,
    base: mode === "diff" ? baseSelect.value : null,
    question: question.value,
  };
  const res = await fetch("/api/ask", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  prompt.hidden = true;
  if (!res.ok) {
    const data = await res.json().catch(() => ({ error: "injection failed" }));
    status.textContent = data.error;
    window.setTimeout(() => {
      status.textContent = "";
    }, 3500);
  }
});

loadAccent();
loadWorktrees();
