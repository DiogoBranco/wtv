use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

#[derive(Deserialize, Serialize, Clone)]
pub struct Config {
    pub repos: Vec<PathBuf>,
    #[serde(default = "default_accent")]
    pub accent: String,
    #[serde(default = "default_claude")]
    pub claude: String,
    #[serde(default = "default_codex")]
    pub codex: String,
}

fn default_claude() -> String {
    "claude".to_string()
}

fn default_codex() -> String {
    "codex".to_string()
}

#[derive(Clone, Serialize)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    pub head: String,
}

#[derive(Clone, Serialize)]
pub struct RepoWorktrees {
    pub repo: String,
    pub name: String,
    pub worktrees: Vec<Worktree>,
}

#[derive(Clone, Serialize)]
pub struct ChangedFile {
    pub path: String,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

#[derive(Clone, Serialize)]
pub struct SideContent {
    pub exists: bool,
    pub content: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct DiffContent {
    pub old: SideContent,
    pub new: SideContent,
}

pub fn default_accent() -> String {
    "#98c379".to_string()
}

pub fn config_path(args: impl Iterator<Item = String>) -> Result<PathBuf, String> {
    let mut args = args.skip(1);
    let mut path = None;
    while let Some(arg) = args.next() {
        if arg == "--config" {
            path = Some(PathBuf::from(args.next().ok_or("--config requires a path")?));
        } else {
            return Err(format!("unknown argument: {arg}"));
        }
    }
    Ok(path.unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").expect("HOME not set"))
            .join(".config/wtv/config.toml")
    }))
}

pub fn load_config(path: &Path) -> Result<Config, String> {
    if !path.exists() {
        return create_config(path);
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    toml::from_str(&raw).map_err(|e| format!("invalid config: {e}"))
}

fn current_repo() -> Option<PathBuf> {
    let dir = std::env::current_dir().ok()?;
    let common = git(&dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let common = PathBuf::from(common.trim());
    std::fs::canonicalize(common.parent()?).ok()
}

fn create_config(path: &Path) -> Result<Config, String> {
    let repo = current_repo().ok_or_else(|| {
        format!(
            "No config at {} and this directory is not a git repository.\n\nCreate the file with the repos you want to browse:\n\n  repos = [\"/path/to/your/repo\"]\n",
            path.display()
        )
    })?;
    let config = Config {
        repos: vec![repo],
        accent: default_accent(),
        claude: default_claude(),
        codex: default_codex(),
    };
    let body = toml::to_string_pretty(&config).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, body).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(config)
}

fn command(dir: &Path, args: &[&str]) -> Option<std::process::Output> {
    Command::new("git").arg("-C").arg(dir).args(args).output().ok()
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = command(dir, args)?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    porcelain
        .split("\n\n")
        .filter_map(|block| {
            let mut path = None;
            let mut head = String::new();
            let mut branch = None;
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("worktree ") {
                    path = Some(v.to_string());
                } else if let Some(v) = line.strip_prefix("HEAD ") {
                    head = v.to_string();
                } else if let Some(v) = line.strip_prefix("branch ") {
                    branch = Some(v.trim_start_matches("refs/heads/").to_string());
                }
            }
            path.map(|path| Worktree { path, branch, head })
        })
        .collect()
}

fn owned_by_me(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.uid() == unsafe { libc::getuid() })
        .unwrap_or(false)
}

pub fn list_owned_worktrees(repo: &Path) -> Vec<Worktree> {
    let Some(out) = git(repo, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    parse_worktrees(&out)
        .into_iter()
        .filter_map(|mut worktree| {
            let path = std::fs::canonicalize(&worktree.path).ok()?;
            if !owned_by_me(&path) {
                return None;
            }
            worktree.path = path.display().to_string();
            Some(worktree)
        })
        .collect()
}

pub fn repo_worktrees(config: &Config) -> Vec<RepoWorktrees> {
    config
        .repos
        .iter()
        .map(|repo| RepoWorktrees {
            repo: repo.display().to_string(),
            name: repo
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| repo.display().to_string()),
            worktrees: list_owned_worktrees(repo),
        })
        .collect()
}

pub fn validate_worktree(config: &Config, worktree: &Path) -> Result<PathBuf, &'static str> {
    let known: HashSet<PathBuf> = config
        .repos
        .iter()
        .flat_map(|r| list_owned_worktrees(r))
        .filter_map(|w| std::fs::canonicalize(w.path).ok())
        .collect();
    let canon = std::fs::canonicalize(worktree).map_err(|_| "not found")?;
    known.contains(&canon).then_some(canon).ok_or("forbidden")
}

pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

pub fn files(wt: &Path) -> Result<Vec<String>, &'static str> {
    let out = git(wt, &["ls-files", "--cached", "--others", "--exclude-standard"])
        .ok_or("git failed")?;
    let mut list: Vec<String> = out.lines().map(str::to_string).collect();
    list.sort();
    Ok(list)
}

pub fn file_content(wt: &Path, path: &str) -> Result<Option<String>, &'static str> {
    if !valid_path(path) {
        return Err("forbidden");
    }
    let full = std::fs::canonicalize(wt.join(path)).map_err(|_| "not found")?;
    if !full.starts_with(wt) {
        return Err("forbidden");
    }
    let bytes = std::fs::read(full).map_err(|_| "not found")?;
    Ok(String::from_utf8(bytes).ok())
}

pub fn branch_refs(wt: &Path) -> Vec<String> {
    let out = git(wt, &["for-each-ref", "--format=%(refname:short)", "refs/heads", "refs/remotes"])
        .unwrap_or_default();
    let mut refs: Vec<String> = out
        .lines()
        .filter(|r| !r.ends_with("/HEAD"))
        .map(str::to_string)
        .collect();
    refs.sort();
    refs.dedup();
    refs
}

pub fn default_branch(wt: &Path, refs: &[String]) -> Option<String> {
    if let Some(v) = git(wt, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]) {
        let value = v.trim().to_string();
        if refs.contains(&value) {
            return Some(value);
        }
    }
    ["origin/main", "origin/master", "main", "master"]
        .into_iter()
        .find(|r| refs.iter().any(|v| v == r))
        .map(str::to_string)
}

pub fn checked_base(wt: &Path, base: &str) -> Result<String, &'static str> {
    if !branch_refs(wt).iter().any(|r| r == base) {
        return Err("invalid base");
    }
    git(wt, &["merge-base", "HEAD", base])
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or("invalid base")
}

pub fn changed_files(wt: &Path, base: &str) -> Result<Vec<ChangedFile>, &'static str> {
    let base = checked_base(wt, base)?;
    let out = git(wt, &["diff", "--no-renames", "--numstat", "-z", &base, "--"])
        .ok_or("git failed")?;
    let mut files = Vec::new();
    for record in out.split('\0').filter(|v| !v.is_empty()) {
        let mut fields = record.splitn(3, '\t');
        let additions = fields.next().unwrap_or("");
        let deletions = fields.next().unwrap_or("");
        let path = fields.next().unwrap_or("");
        if valid_path(path) {
            files.push(ChangedFile {
                path: path.to_string(),
                additions: additions.parse().ok(),
                deletions: deletions.parse().ok(),
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn old_content(wt: &Path, base: &str, path: &str) -> SideContent {
    let empty = || SideContent { exists: false, content: None };
    let Some(tree) = command(wt, &["ls-tree", "-z", base, "--", path]) else { return empty() };
    if !tree.status.success() || tree.stdout.is_empty() {
        return empty();
    }
    let text = String::from_utf8_lossy(&tree.stdout);
    let Some(hash) = text.split_once('\t').and_then(|(m, _)| m.split_whitespace().nth(2)) else { return empty() };
    let Some(blob) = command(wt, &["cat-file", "blob", hash]) else { return empty() };
    SideContent {
        exists: blob.status.success(),
        content: blob.status.success().then(|| String::from_utf8(blob.stdout).ok()).flatten(),
    }
}

fn new_content(wt: &Path, path: &str) -> SideContent {
    let empty = || SideContent { exists: false, content: None };
    let Ok(canon) = std::fs::canonicalize(wt.join(path)) else { return empty() };
    if !canon.starts_with(wt) || !canon.is_file() {
        return empty();
    }
    let Ok(bytes) = std::fs::read(canon) else { return empty() };
    SideContent { exists: true, content: String::from_utf8(bytes).ok() }
}

pub fn diff_content(wt: &Path, base: &str, path: &str) -> Result<DiffContent, &'static str> {
    if !valid_path(path) {
        return Err("forbidden");
    }
    let base = checked_base(wt, base)?;
    Ok(DiffContent { old: old_content(wt, &base, path), new: new_content(wt, path) })
}

fn executable(command: &str) -> &str {
    Path::new(command)
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or(command)
        .trim_start_matches('-')
}

fn process_names(comm: &str, args: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
    const INTERPRETERS: [&str; 8] = ["node", "bun", "deno", "python", "python3", "sh", "bash", "zsh"];
    let name = executable(comm).to_string();
    let mut names = vec![name.clone()];
    if INTERPRETERS.contains(&name.as_str()) {
        names.extend(
            args.map(|token| token.as_ref().to_string())
                .filter(|token| token.contains('/'))
                .filter_map(|token| Path::new(&token).file_name().and_then(|v| v.to_str()).map(str::to_string)),
        );
    }
    names
}

fn pane_commands(root: u32) -> HashSet<String> {
    let Some(output) = Command::new("ps").args(["-eo", "pid=,ppid=,comm=,args="]).output().ok().filter(|v| v.status.success()) else { return HashSet::new() };
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut commands: HashMap<u32, Vec<String>> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|v| v.parse().ok()) else { continue };
        let Some(ppid) = fields.next().and_then(|v| v.parse::<u32>().ok()) else { continue };
        let comm = fields.next().unwrap_or("");
        commands.insert(pid, process_names(comm, fields));
        children.entry(ppid).or_default().push(pid);
    }
    let mut found = HashSet::new();
    let mut pending = vec![root];
    while let Some(pid) = pending.pop() {
        if let Some(names) = commands.get(&pid) {
            found.extend(names.iter().cloned());
        }
        pending.extend(children.get(&pid).into_iter().flatten());
    }
    found
}

fn find_pane(worktree: &Path, agent: &str) -> Option<String> {
    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{pane_id}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_pid}"])
        .output().ok()?;
    if !output.status.success() {
        return None;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 4 { continue; }
        let Ok(path) = std::fs::canonicalize(fields[1]) else { continue };
        let Ok(pid) = fields[3].parse() else { continue };
        if path == worktree && (executable(fields[2]) == agent || pane_commands(pid).contains(agent)) {
            return Some(fields[0].to_string());
        }
    }
    None
}

pub fn inject(worktree: &Path, agent: &str, file: &str, lines: Option<&str>, base: Option<&str>, question: &str) -> Result<(), String> {
    if !matches!(agent, "claude" | "codex") || !valid_path(file) {
        return Err("invalid request".into());
    }
    if let Some(base) = base {
        checked_base(worktree, base).map_err(str::to_string)?;
    }
    let pane = find_pane(worktree, agent).ok_or_else(|| format!("no {agent} pane found for this worktree"))?;
    let question = question.split_whitespace().collect::<Vec<_>>().join(" ");
    if question.is_empty() { return Err("question is required".into()); }
    let lines = lines.map(|v| format!(":{v}")).unwrap_or_default();
    let base = base.map(|v| format!(" diff against {v}")).unwrap_or_default();
    let prompt = format!("{question} {file}{lines}{base}");
    let sent = Command::new("tmux").args(["send-keys", "-t", &pane, "-l", "--", &prompt]).status().map(|s| s.success()).unwrap_or(false);
    if !sent { return Err("injection failed".into()); }
    std::thread::sleep(Duration::from_millis(80));
    Command::new("tmux").args(["send-keys", "-t", &pane, "Enter"]).status().map(|s| s.success()).unwrap_or(false).then_some(()).ok_or_else(|| "injection failed".into())
}

fn parse_post_create(config: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut inside = false;
    for line in config.lines() {
        if line.starts_with("post_create:") {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if !line.trim().is_empty() && !line.starts_with(char::is_whitespace) {
            break;
        }
        if let Some(item) = line.trim_start().strip_prefix("- ") {
            let item = item.trim().trim_matches(['"', '\'']);
            if !item.is_empty() && item != "<global>" {
                commands.push(item.to_string());
            }
        }
    }
    commands
}

pub fn post_create_commands(repo: &Path) -> Vec<String> {
    parse_post_create(&std::fs::read_to_string(repo.join(".workmux.yaml")).unwrap_or_default())
}

pub fn send_to_pane(pane: &str, text: &str) -> Result<(), String> {
    let sent = Command::new("tmux")
        .args(["send-keys", "-t", pane, "-l", "--", text])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !sent {
        return Err("could not reach the shell pane".into());
    }
    std::thread::sleep(Duration::from_millis(80));
    Command::new("tmux")
        .args(["send-keys", "-t", pane, "Enter"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        .then_some(())
        .ok_or_else(|| "could not reach the shell pane".into())
}

pub fn retarget_window(worktree: &Path, claude: &str, codex: &str) -> Result<Option<String>, String> {
    let own = std::env::var("TMUX_PANE").map_err(|_| "not inside tmux")?;
    let output = Command::new("tmux")
        .args(["list-panes", "-t", &own, "-F", "#{pane_id}\t#{pane_pid}\t#{pane_current_command}"])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("tmux failed".into());
    }
    let mut shell = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 3 || fields[0] == own {
            continue;
        }
        let Ok(pid) = fields[1].parse() else { continue };
        let mut commands = pane_commands(pid);
        let front = executable(fields[2]).to_string();
        commands.insert(front.clone());
        let respawn = if commands.contains("claude") {
            Some(format!("{claude} --continue || exec {claude}"))
        } else if commands.contains("codex") {
            Some(format!("{codex} resume --last || exec {codex}"))
        } else if matches!(front.as_str(), "bash" | "zsh" | "sh" | "fish") && commands.len() == 1 {
            shell = Some(fields[0].to_string());
            None
        } else {
            continue;
        };
        let mut command = Command::new("tmux");
        command.args(["respawn-pane", "-k", "-t", fields[0], "-c"]).arg(worktree);
        if let Some(respawn) = respawn {
            command.arg(respawn);
        }
        let _ = command.status();
    }
    Ok(shell)
}

fn configured_worktree_dir(config: &str) -> Option<String> {
    config.lines().find_map(|line| {
        let value = line.strip_prefix("worktree_dir:")?;
        let value = value.split('#').next().unwrap_or("").trim().trim_matches(['"', '\'']);
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn worktree_dir(repo: &Path) -> PathBuf {
    let config = std::fs::read_to_string(repo.join(".workmux.yaml")).unwrap_or_default();
    if let Some(value) = configured_worktree_dir(&config) {
        return repo.join(value);
    }
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    repo.parent().unwrap_or(repo).join(format!("{name}__worktrees"))
}

pub fn create_worktree(repo: &Path, branch: &str) -> Result<PathBuf, String> {
    if branch.is_empty() || branch.contains(char::is_whitespace) {
        return Err("invalid branch name".into());
    }
    let dir = worktree_dir(repo).join(branch.replace('/', "-"));
    let refs = branch_refs(repo);
    let base = default_branch(repo, &refs).unwrap_or_else(|| "HEAD".to_string());
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "add", "-b", branch])
        .arg(&dir)
        .arg(&base)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .rev()
            .find(|line| line.starts_with("fatal:") || line.starts_with("error:"))
            .or_else(|| stderr.lines().rev().find(|line| !line.trim().is_empty()))
            .unwrap_or("git worktree add failed");
        return Err(reason.trim_start_matches("fatal: ").trim().to_string());
    }
    std::fs::canonicalize(&dir).map_err(|e| e.to_string())
}

pub fn watch(worktree: &Path, tx: Sender<()>) -> Result<RecommendedWatcher, notify::Error> {
    let git_dir = git(worktree, &["rev-parse", "--absolute-git-dir"]).map(|v| PathBuf::from(v.trim()));
    let common_dir = git(worktree, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .map(|v| PathBuf::from(v.trim()))
        .filter(|common| git_dir.as_ref() != Some(common));
    let git_dirs: Vec<PathBuf> = git_dir
        .iter()
        .chain(common_dir.iter())
        .cloned()
        .chain([worktree.join(".git")])
        .collect();
    let mut watcher = notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
        let Ok(event) = event else { return };
        if event.kind.is_access() {
            return;
        }
        let relevant = event.paths.is_empty()
            || event.paths.iter().any(|path| {
                match git_dirs.iter().find_map(|dir| path.strip_prefix(dir).ok()) {
                    Some(rel) => rel == Path::new("HEAD") || rel.starts_with("refs"),
                    None => true,
                }
            });
        if relevant {
            let _ = tx.send(());
        }
    })?;
    watcher.watch(worktree, RecursiveMode::Recursive)?;
    if let Some(git_dir) = git_dir {
        let _ = watcher.watch(&git_dir, RecursiveMode::Recursive);
    }
    if let Some(common_dir) = common_dir {
        let _ = watcher.watch(&common_dir, RecursiveMode::Recursive);
    }
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_flag() {
        let args = vec!["wtv".into(), "--config".into(), "/tmp/x".into()];
        assert_eq!(config_path(args.into_iter()).unwrap(), PathBuf::from("/tmp/x"));
    }

    #[test]
    fn rejects_positional_config() {
        let args = vec!["wtv".into(), "/tmp/x".into()];
        assert!(config_path(args.into_iter()).is_err());
    }

    #[test]
    fn reads_post_create_hooks() {
        let config = "worktree_dir: .worktrees\n\npost_create:\n  - uv venv .venv && make prepare_env\n  - \"<global>\"\n\nfiles:\n  symlink:\n    - .claude/settings.json\n";
        assert_eq!(parse_post_create(config), vec!["uv venv .venv && make prepare_env"]);
        assert!(parse_post_create("main_branch: main\n").is_empty());
        assert!(parse_post_create("post_create: []\n").is_empty());
    }

    #[test]
    fn reads_worktree_dir_from_workmux_config() {
        assert_eq!(configured_worktree_dir("worktree_dir: .worktrees\n").as_deref(), Some(".worktrees"));
        assert_eq!(configured_worktree_dir("# worktree_dir: .worktrees\n"), None);
        assert_eq!(configured_worktree_dir("worktree_dir: trees  # inline\n").as_deref(), Some("trees"));
        assert_eq!(configured_worktree_dir("main_branch: main\n"), None);
    }

    #[test]
    fn finds_agent_behind_an_interpreter() {
        let names = process_names("node", ["node", "/home/u/.npm-global/bin/codex"].into_iter());
        assert!(names.contains(&"codex".to_string()));
        let plain = process_names("vim", ["vim", "/home/u/notes/codex"].into_iter());
        assert!(!plain.contains(&"codex".to_string()));
        let bare = process_names("git", ["git", "diff", "codex"].into_iter());
        assert!(!bare.contains(&"codex".to_string()));
    }

    #[test]
    fn validates_relative_file_paths() {
        assert!(valid_path("src/main.rs"));
        assert!(!valid_path("../secret"));
        assert!(!valid_path("/tmp/secret"));
    }
}
