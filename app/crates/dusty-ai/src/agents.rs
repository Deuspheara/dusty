//! The agents Dusty can ask: the best-known ones from the ACP registry, each with the person's own sign-in
//! and subscription, and each locked down before it sees anything.
//!
//! Two kinds of lock:
//! - [`Lock::NoTools`]: the agent's tools are switched off (Claude: `tools: []`, no settings, no MCP
//!   servers). It only knows what Dusty sends.
//! - [`Lock::ReadOnly`]: the agent is put in its read-only mode (`read-only`, `plan`, `ask`…) before the
//!   first message; it may look at files on its own, but can't change anything. An agent that offers no
//!   such mode isn't asked at all.
//!
//! Either way Dusty offers no file system or terminal, refuses every permission, and stops the trip if the
//! agent reports a change to a file.

use crate::acp::AgentCommand;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    /// No tools at all.
    NoTools,
    /// Read-only mode: may look, can't change.
    ReadOnly,
}

impl Lock {
    /// What the lock means, for people.
    pub fn promise(self, name: &str) -> String {
        match self {
            Lock::NoTools => format!("{name} gets no tools. It can't open files or run anything, it can only answer me."),
            Lock::ReadOnly => format!("{name} runs in its read-only mode: it may peek at files itself, but it can't change anything, and I refuse whatever it asks for."),
        }
    }
    /// What the agent learns about your files.
    pub fn sees(self, name: &str) -> String {
        match self {
            Lock::NoTools => format!("I show {name} the big folders in your Library and hidden folders: names, sizes and ages. Never what's inside a file."),
            Lock::ReadOnly => format!("I show {name} the big folders in your Library and hidden folders: names, sizes and ages. It may open a file itself to be sure, so pick an agent with no file access if that matters to you."),
        }
    }
    pub fn chip(self) -> &'static str {
        match self {
            Lock::NoTools => "NO FILE ACCESS",
            Lock::ReadOnly => "READ-ONLY",
        }
    }
    /// The promise in a few words, under the picked agent.
    pub fn short(self) -> &'static str {
        match self {
            Lock::NoTools => "Can't open files or run anything: it only answers me.",
            Lock::ReadOnly => "May look at files itself, but can't change anything.",
        }
    }
}

enum Launch {
    /// An npm package run with `npx` (or its binary, when it's installed).
    Npx { bin: &'static str, package: &'static str, args: &'static [&'static str] },
    /// A program the person installed.
    Bin { names: &'static [&'static str], args: &'static [&'static str], install: &'static str },
}

pub struct Agent {
    /// Stable id (saved as the person's choice, `dusty explore --agent <id>`).
    pub id: &'static str,
    pub name: &'static str,
    /// Whose subscription it uses ("Claude Pro or Max").
    pub plan: &'static str,
    pub lock: Lock,
    /// How to sign in, from Terminal.
    pub login: &'static str,
    launch: Launch,
}

/// Mode ids that can't change files, safest first. The first one an agent offers is used.
const SAFE_MODES: &[&str] = &["read-only", "readonly", "read_only", "plan", "ask", "default"];

/// Pinned versions (the ACP registry's at the time): an update is a reviewed change, not a surprise.
pub const AGENTS: &[Agent] = &[
    Agent {
        id: "claude",
        name: "Claude",
        plan: "Claude Pro or Max",
        lock: Lock::NoTools,
        login: "claude",
        launch: Launch::Npx { bin: "claude-agent-acp", package: "@agentclientprotocol/claude-agent-acp@0.109.1", args: &[] },
    },
    Agent {
        id: "codex",
        name: "Codex",
        plan: "ChatGPT plan",
        lock: Lock::ReadOnly,
        login: "codex login",
        launch: Launch::Npx { bin: "codex-acp", package: "@agentclientprotocol/codex-acp@2.9.0", args: &[] },
    },
    Agent {
        id: "gemini",
        name: "Gemini CLI",
        // Google moved personal accounts to Antigravity: this works with an API key or Code Assist for business
        plan: "Gemini API key or Code Assist plan",
        lock: Lock::ReadOnly,
        login: "gemini",
        launch: Launch::Npx { bin: "", package: "@google/gemini-cli@0.70.0", args: &["--acp"] },
    },
    Agent {
        id: "copilot",
        name: "GitHub Copilot",
        plan: "Copilot plan",
        lock: Lock::ReadOnly,
        login: "copilot (then /login)",
        launch: Launch::Npx { bin: "", package: "@github/copilot@1.0.114", args: &["--acp"] },
    },
    Agent {
        id: "cursor",
        name: "Cursor",
        plan: "Cursor plan",
        lock: Lock::ReadOnly,
        login: "cursor-agent login",
        launch: Launch::Bin { names: &["cursor-agent"], args: &["acp"], install: "Not installed: get Cursor's CLI at cursor.com/cli" },
    },
    Agent {
        id: "opencode",
        name: "OpenCode",
        plan: "OpenCode providers",
        lock: Lock::ReadOnly,
        login: "opencode auth login",
        launch: Launch::Bin { names: &["opencode"], args: &["acp"], install: "Not installed: get it at opencode.ai" },
    },
];

pub fn agent(id: &str) -> Option<&'static Agent> {
    AGENTS.iter().find(|a| a.id == id)
}

/// Whether an agent can be started on this Mac.
#[derive(Clone, Debug, PartialEq)]
pub enum Availability {
    Ready,
    /// Why not, and what to do.
    Missing(String),
}

impl Agent {
    pub fn availability(&self) -> Availability {
        match self.command("") {
            Ok(_) => Availability::Ready,
            Err(why) => Availability::Missing(why),
        }
    }

    /// The command that starts it, locked down. `system_prompt` replaces the agent's own where it allows.
    pub fn command(&self, system_prompt: &str) -> Result<AgentCommand, String> {
        let path_env = login_path();
        let (program, args) = match &self.launch {
            Launch::Npx { bin, package, args } => match find(&path_env, bin).filter(|_| !bin.is_empty()) {
                Some(p) => (p, args.iter().map(|s| s.to_string()).collect()),
                None => {
                    let npx = find(&path_env, "npx").ok_or("Needs Node.js: get it at nodejs.org")?;
                    (npx, ["--yes", package].iter().chain(args.iter()).map(|s| s.to_string()).collect())
                }
            },
            Launch::Bin { names, args, install } => {
                let p = names.iter().find_map(|n| find(&path_env, n)).ok_or(*install)?;
                (p, args.iter().map(|s| s.to_string()).collect())
            }
        };
        let session_meta = match self.id {
            "claude" => json!({ "claudeCode": { "options": {
                "tools": [],
                "settingSources": [],
                "strictMcpConfig": true,
                "allowDangerouslySkipPermissions": false,
                "promptSuggestions": false,
                // folder names are easy work: a smaller model does it well and spends less of the plan
                "model": "sonnet",
                "systemPrompt": system_prompt,
            } } }),
            _ => Value::Null,
        };
        let safe_modes = if self.lock == Lock::ReadOnly { SAFE_MODES.to_vec() } else { vec![] };
        Ok(AgentCommand { program, args, path_env, session_meta, safe_modes, system_prompt: (self.id != "claude").then(|| system_prompt.to_string()) })
    }
}

fn find(path_env: &str, bin: &str) -> Option<PathBuf> {
    if bin.is_empty() {
        return None;
    }
    path_env.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(bin)).find(|p| p.is_file())
}

/// The PATH a Terminal window would have (apps opened from Finder get a bare one, without Node.js), plus
/// the usual places in case the shell can't say.
fn login_path() -> String {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let shell = std::env::var("SHELL").ok().filter(|s| s.starts_with('/')).unwrap_or_else(|| "/bin/zsh".into());
        // a marker, because a chatty profile may print before it
        let r = dusty_core::run(&shell, &["-l".into(), "-c".into(), "printf '\\n__DUSTY_PATH__%s' \"$PATH\"".into()], Duration::from_secs(10), None);
        let from_shell = r.stdout.rsplit_once("__DUSTY_PATH__").map(|(_, p)| p.trim().to_string()).unwrap_or_default();
        let hm = dusty_core::home();
        let extra = [hm.join(".local/bin"), hm.join(".volta/bin"), hm.join(".bun/bin"), hm.join(".opencode/bin"), "/opt/homebrew/bin".into(), "/usr/local/bin".into(), "/usr/bin".into(), "/bin".into()];
        let mut parts: Vec<String> = from_shell.split(':').chain(std::env::var("PATH").unwrap_or_default().split(':')).filter(|s| !s.is_empty()).map(String::from).collect();
        parts.extend(extra.iter().map(|p| p.to_string_lossy().into_owned()));
        let mut seen = std::collections::HashSet::new();
        parts.retain(|p| seen.insert(p.clone()));
        parts.join(":")
    })
    .clone()
}

// ───────────────────────── the person's choice ─────────────────────────

/// Which agent you picked, and the account each one said it uses (`~/.config/dusty/agents.json`).
#[derive(Default, Clone, Debug)]
pub struct AgentPrefs {
    pub selected: Option<String>,
    /// agent id → account label it reported ("Claude Pro", "ChatGPT Team")
    pub accounts: std::collections::BTreeMap<String, String>,
}

fn prefs_file() -> PathBuf {
    dusty_core::home().join(".config/dusty/agents.json")
}

impl AgentPrefs {
    pub fn load() -> Self {
        let v: Value = std::fs::read_to_string(prefs_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        let accounts = v.get("accounts").and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
        AgentPrefs { selected: v.get("selected").and_then(Value::as_str).map(String::from), accounts }
    }

    pub fn save(&self) -> Result<(), String> {
        let f = prefs_file();
        if let Some(d) = f.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(&json!({ "selected": self.selected, "accounts": self.accounts })).map_err(|e| e.to_string())?;
        let tmp = f.with_extension("json.tmp");
        std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &f)).map_err(|e| e.to_string())
    }

    /// The picked agent (Claude when nothing was picked yet).
    pub fn agent(&self) -> &'static Agent {
        self.selected.as_deref().and_then(agent).unwrap_or(&AGENTS[0])
    }
}
