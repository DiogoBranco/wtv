use crate::core::{self, Config};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const ASK_LIMIT: usize = 12;
const ASK_WINDOW: u64 = 900;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn stamp() -> String {
    Command::new("date")
        .arg("+%Y-%m-%d %H:%M")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn slug(text: &str) -> String {
    let words: Vec<String> = text
        .split_whitespace()
        .take(6)
        .map(|word| word.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase())
        .filter(|word| !word.is_empty())
        .collect();
    if words.is_empty() { "discussion".to_string() } else { words.join("-") }
}

fn state_dir(worktree: &Path) -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set")?;
    let name = worktree.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = PathBuf::from(home).join(".local/share/wtv").join(name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

fn within_rate_limit(dir: &Path, agent: &str) -> Result<(), String> {
    let path = dir.join(format!("{agent}.asks"));
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let cutoff = now().saturating_sub(ASK_WINDOW);
    let mut recent: Vec<u64> = raw.lines().filter_map(|line| line.trim().parse().ok()).filter(|t| *t > cutoff).collect();
    if recent.len() >= ASK_LIMIT {
        return Err(format!(
            "rate limit: {ASK_LIMIT} exchanges with {agent} in the last {} minutes. Stop, or wait.",
            ASK_WINDOW / 60
        ));
    }
    recent.push(now());
    let body = recent.iter().map(|t| t.to_string()).collect::<Vec<_>>().join("\n");
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(())
}

fn reject_relay(text: &str) -> Result<(), String> {
    if text.trim_start().starts_with("[from ") {
        return Err("that message is already a relayed message; answer it yourself instead of passing it on".into());
    }
    Ok(())
}

fn transcript_path(dir: &Path, agent: &str, text: &str, fresh: bool) -> Result<PathBuf, String> {
    let pointer = dir.join(format!("{agent}.transcript"));
    if !fresh {
        if let Ok(existing) = std::fs::read_to_string(&pointer) {
            let path = PathBuf::from(existing.trim());
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    let date = Command::new("date")
        .arg("+%Y-%m-%d-%H%M")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let path = dir.join(format!("{date}-{}.md", slug(text)));
    std::fs::write(&pointer, path.display().to_string()).map_err(|e| e.to_string())?;
    Ok(path)
}

fn append(path: &Path, body: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = write!(file, "{body}");
    }
}

fn session_id(dir: &Path, agent: &str, fresh: bool) -> Option<String> {
    if fresh {
        let _ = std::fs::remove_file(dir.join(format!("{agent}.session")));
        return None;
    }
    std::fs::read_to_string(dir.join(format!("{agent}.session")))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn save_session(dir: &Path, agent: &str, id: &str) {
    let _ = std::fs::write(dir.join(format!("{agent}.session")), id);
}

fn run(program: &str, args: &[String], worktree: &Path) -> Result<String, String> {
    let mut parts = program.split_whitespace();
    let head = parts.next().ok_or("no command configured")?;
    let output = Command::new(head)
        .args(parts)
        .args(args)
        .current_dir(worktree)
        .output()
        .map_err(|e| format!("cannot run {head}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("command failed");
        return Err(reason.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn codex_exchange(command: &str, worktree: &Path, id: Option<String>, text: &str) -> Result<(String, String), String> {
    let mut args: Vec<String> = vec!["exec".into()];
    if let Some(id) = &id {
        args.extend(["resume".to_string(), id.clone()]);
    }
    args.extend(["--json".to_string(), text.to_string()]);
    let stdout = run(command, &args, worktree)?;
    let mut thread = id.unwrap_or_default();
    let mut reply = String::new();
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if event["type"] == "thread.started" {
            if let Some(value) = event["thread_id"].as_str() {
                thread = value.to_string();
            }
        }
        if event["item"]["type"] == "agent_message" {
            if let Some(value) = event["item"]["text"].as_str() {
                reply = value.to_string();
            }
        }
    }
    if reply.is_empty() {
        return Err("codex returned no message".into());
    }
    Ok((thread, reply))
}

fn claude_exchange(command: &str, worktree: &Path, id: Option<String>, text: &str) -> Result<(String, String), String> {
    let resuming = id.is_some();
    let session = id.unwrap_or_else(uuid);
    let flag = if resuming { "--resume" } else { "--session-id" };
    let args: Vec<String> = vec!["-p".into(), flag.into(), session.clone(), text.to_string()];
    let reply = run(command, &args, worktree)?.trim().to_string();
    if reply.is_empty() {
        return Err("claude returned no message".into());
    }
    Ok((session, reply))
}

fn uuid() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_err()
    {
        let seed = now();
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = (seed >> (i % 8)) as u8 ^ i as u8;
        }
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-4{}-8{}-{}", &hex[0..8], &hex[8..12], &hex[13..16], &hex[17..20], &hex[20..32])
}

fn resolve(config: &Config, agent: &str) -> Result<(PathBuf, String), String> {
    if !matches!(agent, "claude" | "codex") {
        return Err("agent must be claude or codex".into());
    }
    let root = core::worktree_root().ok_or("run this inside a git worktree")?;
    let worktree = core::validate_worktree(config, &root)
        .map_err(|_| "this worktree is not in any repo listed in the wtv config".to_string())?;
    let command = if agent == "claude" { config.claude.clone() } else { config.codex.clone() };
    Ok((worktree, command))
}

pub fn say(config: &Config, agent: &str, text: &str) -> Result<String, String> {
    let (worktree, _) = resolve(config, agent)?;
    reject_relay(text)?;
    let sender = core::calling_agent();
    core::message_pane(&worktree, agent, &format!("[from {sender}] {text}"))?;
    Ok(format!("sent to the {agent} pane"))
}

pub fn ask(config: &Config, agent: &str, text: &str, fresh: bool) -> Result<String, String> {
    let (worktree, command) = resolve(config, agent)?;
    reject_relay(text)?;
    let dir = state_dir(&worktree)?;
    within_rate_limit(&dir, agent)?;
    let sender = core::calling_agent();
    let id = session_id(&dir, agent, fresh);
    let first = id.is_none();
    let transcript = transcript_path(&dir, agent, text, fresh || first)?;
    if first {
        let branch = core::list_owned_worktrees(&worktree)
            .into_iter()
            .find(|w| Path::new(&w.path) == worktree)
            .and_then(|w| w.branch)
            .unwrap_or_else(|| "detached".to_string());
        append(
            &transcript,
            &format!("# {}\n\nworktree: {}\nbranch: {}\nstarted: {}\n", slug(text).replace('-', " "), worktree.display(), branch, stamp()),
        );
    }
    let (id, reply) = if agent == "codex" {
        codex_exchange(&command, &worktree, id, text)?
    } else {
        claude_exchange(&command, &worktree, id, text)?
    };
    save_session(&dir, agent, &id);
    append(&transcript, &format!("\n## {sender} → {agent} · {}\n\n{text}\n", stamp()));
    append(&transcript, &format!("\n## {agent} → {sender} · {}\n\n{reply}\n", stamp()));
    Ok(format!("{reply}\n\n---\ntranscript: {}", transcript.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_distinct_uuids_promptly() {
        let a = uuid();
        let b = uuid();
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
        assert_ne!(a, b);
    }

    #[test]
    fn refuses_to_forward_a_relayed_message() {
        assert!(reject_relay("[from codex] pass this on").is_err());
        assert!(reject_relay("my own argument").is_ok());
    }
}
