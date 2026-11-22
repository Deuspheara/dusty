//! A small Agent Client Protocol client (JSON-RPC over the agent's stdin and stdout), enough for one
//! session of plain text questions. See <https://agentclientprotocol.com>.
//!
//! Dusty offers the agent nothing: no file system, no terminal. Every permission the agent asks for is
//! refused, and any other request from it gets "method not found". When the agent has modes, the session is
//! switched to a read-only one before the first message (and isn't used at all if there is none). If the
//! agent still reports changing a file, the session stops.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How to start an agent.
#[derive(Clone, Debug)]
pub struct AgentCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// `PATH` for the agent (the login shell's, so Node.js resolves).
    pub path_env: String,
    /// Agent-specific options for `session/new` (`_meta`).
    pub session_meta: Value,
    /// Read-only mode ids to switch to, safest first. Empty: no switch needed (the agent has no tools).
    pub safe_modes: Vec<&'static str>,
    /// Instructions for the session: as its system prompt when the agent takes one, else before the first message.
    pub system_prompt: Option<String>,
}

/// Why a session couldn't go on.
#[derive(Debug, PartialEq)]
pub enum AcpError {
    /// The agent wants a sign-in first.
    SignIn,
    /// The cancel flag was set.
    Stopped,
    /// The agent has no read-only mode to switch to.
    NoSafeMode,
    /// The agent reported changing, moving or deleting a file.
    Meddled,
    Failed(String),
}

pub struct Session {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    stderr: Arc<Mutex<Vec<String>>>,
    next_id: u64,
    id: String,
    /// Instructions still to send with the first message.
    preface: Option<String>,
    /// The account the agent said it uses ("Claude Pro", "ChatGPT Team").
    pub account: Option<String>,
    /// Tokens the agent reported for this session.
    pub tokens: u64,
    /// Times the agent used a tool of its own (read-only ones; anything else stops the session).
    pub own_looks: usize,
}

impl Session {
    /// Starts the agent and opens a session in `cwd` (an empty folder, so there is nothing to look at).
    pub fn start(cmd: &AgentCommand, cwd: &Path, cancel: &AtomicBool) -> Result<Session, AcpError> {
        let mut child = Command::new(&cmd.program)
            .args(&cmd.args)
            .env("PATH", &cmd.path_env)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // its own process group, so stopping it also stops whatever it started (npx → node)
            .process_group(0)
            .spawn()
            .map_err(|e| AcpError::Failed(format!("couldn't start {} ({e})", cmd.program.display())))?;
        let stdin = child.stdin.take().ok_or_else(|| AcpError::Failed("no stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| AcpError::Failed("no stdout".into()))?;
        let err = child.stderr.take();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                // anything that isn't JSON (a stray log line) is ignored
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
        let stderr = Arc::new(Mutex::new(vec![]));
        if let Some(err) = err {
            let tail = stderr.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let mut t = tail.lock().unwrap();
                    t.push(line);
                    if t.len() > 20 {
                        t.remove(0);
                    }
                }
            });
        }
        let mut s = Session { child, stdin, rx, stderr, next_id: 0, id: String::new(), preface: None, account: None, tokens: 0, own_looks: 0 };
        let caps = json!({ "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false });
        let info = json!({ "name": "dusty", "title": "Dusty", "version": env!("CARGO_PKG_VERSION") });
        // the first start may download the adapter: give it time
        let init = s.request("initialize", json!({ "protocolVersion": 1, "clientCapabilities": caps, "clientInfo": info }), Duration::from_secs(180), cancel, &mut |_| {})?;
        if init.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
            return Err(AcpError::Failed("the agent speaks another version of the protocol".into()));
        }
        let mut params = json!({ "cwd": cwd, "mcpServers": [] });
        if !cmd.session_meta.is_null() {
            params["_meta"] = cmd.session_meta.clone();
        }
        let takes_prompt = init.pointer("/agentCapabilities/sessionCapabilities/systemPrompt").is_some();
        match (&cmd.system_prompt, takes_prompt) {
            // added to the agent's own instructions (ACP's only mode for it)
            (Some(sp), true) => params["systemPrompt"] = json!({ "mode": "append", "content": [{ "type": "text", "text": sp }] }),
            (Some(sp), false) => s.preface = Some(sp.clone()),
            _ => {}
        }
        let new = s.request("session/new", params, Duration::from_secs(120), cancel, &mut |_| {})?;
        s.id = new.get("sessionId").and_then(Value::as_str).ok_or_else(|| AcpError::Failed("the agent opened no session".into()))?.to_string();
        if !cmd.safe_modes.is_empty() {
            s.lock_down(&new, &cmd.safe_modes, cancel)?;
        }
        Ok(s)
    }

    /// Switches the session to the first read-only mode it offers (as a mode, or as a "mode" option).
    fn lock_down(&mut self, new: &Value, safe: &[&str], cancel: &AtomicBool) -> Result<(), AcpError> {
        let ids = |v: Option<&Value>, key: &str| -> Vec<String> { v.and_then(Value::as_array).map(|a| a.iter().filter_map(|m| m.get(key)?.as_str().map(String::from)).collect()).unwrap_or_default() };
        let modes = ids(new.pointer("/modes/availableModes"), "id");
        let option = new.get("configOptions").and_then(Value::as_array).and_then(|a| a.iter().find(|o| o.get("id").and_then(Value::as_str) == Some("mode")).cloned());
        let values = ids(option.as_ref().and_then(|o| o.get("options")), "value");
        let t = Duration::from_secs(30);
        if let Some(m) = safe.iter().find(|m| modes.iter().any(|x| x == *m)) {
            self.request("session/set_mode", json!({ "sessionId": self.id, "modeId": m }), t, cancel, &mut |_| {})?;
        } else if let Some(m) = safe.iter().find(|m| values.iter().any(|x| x == *m)) {
            self.request("session/set_config_option", json!({ "sessionId": self.id, "configId": "mode", "value": m }), t, cancel, &mut |_| {})?;
        } else {
            return Err(AcpError::NoSafeMode);
        }
        Ok(())
    }

    /// Sends one message and returns the agent's whole answer, calling `on_text` with each piece as it arrives.
    pub fn prompt(&mut self, text: &str, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> Result<String, AcpError> {
        let mut answer = String::new();
        let text = match self.preface.take() {
            Some(p) => format!("{p}\n\n---\n\n{text}"),
            None => text.to_string(),
        };
        let params = json!({ "sessionId": self.id, "prompt": [{ "type": "text", "text": text }] });
        let mut looks = 0;
        let mut meddled = false;
        let r = self.request("session/prompt", params, Duration::from_secs(300), cancel, &mut |u| match u.get("sessionUpdate").and_then(Value::as_str) {
            Some("agent_message_chunk") => {
                if let Some(t) = u.pointer("/content/text").and_then(Value::as_str) {
                    answer.push_str(t);
                    on_text(t);
                }
            }
            Some("tool_call" | "tool_call_update") => {
                let kind = u.get("kind").and_then(Value::as_str);
                if matches!(kind, Some("edit" | "delete" | "move")) && u.get("status").and_then(Value::as_str) != Some("failed") {
                    meddled = true;
                } else if u.get("sessionUpdate").and_then(Value::as_str) == Some("tool_call") {
                    looks += 1;
                }
            }
            _ => {}
        });
        self.own_looks += looks;
        if meddled {
            return Err(AcpError::Meddled);
        }
        let r = r?;
        self.tokens += r.pointer("/usage/totalTokens").and_then(Value::as_u64).unwrap_or(0);
        match r.get("stopReason").and_then(Value::as_str) {
            Some("end_turn") => Ok(answer),
            Some("cancelled") => Err(AcpError::Stopped),
            Some("refusal") => Err(AcpError::Failed("the agent declined to answer".into())),
            Some(other) => Err(AcpError::Failed(format!("the agent stopped early ({other})"))),
            None => Err(AcpError::Failed("the agent's answer had no end".into())),
        }
    }

    fn send(&mut self, v: Value) -> Result<(), AcpError> {
        writeln!(self.stdin, "{v}").and_then(|_| self.stdin.flush()).map_err(|_| self.gone())
    }

    /// One request, answering whatever the agent asks meanwhile, until its response or `timeout`.
    fn request(&mut self, method: &str, params: Value, timeout: Duration, cancel: &AtomicBool, on_update: &mut dyn FnMut(&Value)) -> Result<Value, AcpError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let start = Instant::now();
        let mut cancelled_at: Option<Instant> = None;
        loop {
            if cancel.load(Ordering::Relaxed) {
                match cancelled_at {
                    None => {
                        cancelled_at = Some(Instant::now());
                        if !self.id.is_empty() {
                            let _ = self.send(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": self.id } }));
                        }
                    }
                    // the agent gets a moment to wind down, then it's stopped anyway
                    Some(t) if t.elapsed() > Duration::from_secs(3) => return Err(AcpError::Stopped),
                    _ => {}
                }
            }
            if start.elapsed() > timeout {
                return Err(AcpError::Failed("the agent took too long".into()));
            }
            let m = match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Err(self.gone()),
            };
            let method_of = m.get("method").and_then(Value::as_str).map(str::to_string);
            match (method_of, m.get("id").cloned()) {
                // our response
                (None, Some(rid)) if rid.as_u64() == Some(id) => {
                    if let Some(e) = m.get("error") {
                        let code = e.get("code").and_then(Value::as_i64).unwrap_or(0);
                        let msg = e.get("message").and_then(Value::as_str).unwrap_or("error");
                        // -32000 is "auth required", but agents also use it for other refusals: trust the words
                        let low = msg.to_lowercase();
                        let needs_login = code == -32000 && ["auth", "log in", "login", "sign in", "credential"].iter().any(|w| low.contains(w)) && !low.contains("no longer supported");
                        return Err(if needs_login { AcpError::SignIn } else { AcpError::Failed(msg.chars().take(240).collect()) });
                    }
                    if cancelled_at.is_some() {
                        return Err(AcpError::Stopped);
                    }
                    return Ok(m.get("result").cloned().unwrap_or(Value::Null));
                }
                (Some(meth), None) if meth == "session/update" => {
                    if let Some(u) = m.pointer("/params/update") {
                        on_update(u);
                    }
                }
                (Some(meth), None) if meth.ends_with("auth/status_update") => {
                    if let Some(label) = account_label(&m) {
                        self.account = Some(label);
                    }
                }
                (Some(meth), Some(rid)) => {
                    let reply = if meth == "session/request_permission" { refuse(&m, cancelled_at.is_some()) } else { json!({ "error": { "code": -32601, "message": "Dusty offers no tools" } }) };
                    let mut r = json!({ "jsonrpc": "2.0", "id": rid });
                    r.as_object_mut().unwrap().extend(reply.as_object().cloned().unwrap_or_default());
                    self.send(r)?;
                }
                _ => {}
            }
        }
    }

    /// The agent quit: its last words on stderr say why.
    fn gone(&self) -> AcpError {
        let tail = self.stderr.lock().unwrap().iter().rev().find(|l| !l.trim().is_empty()).cloned().unwrap_or_default();
        AcpError::Failed(if tail.is_empty() { "the agent quit".into() } else { format!("the agent quit: {}", tail.chars().take(200).collect::<String>()) })
    }
}

/// The account an auth status notification names: `_auth/status_update` (`authStatus.label`) or
/// `auth/status_update` (the active entry of `accounts`).
fn account_label(m: &Value) -> Option<String> {
    let p = m.get("params")?;
    let label = p.pointer("/authStatus/label").and_then(Value::as_str).or_else(|| {
        let accounts = p.get("accounts")?.as_array()?;
        accounts.iter().find(|a| a.get("active").and_then(Value::as_bool) == Some(true)).or(accounts.first())?.get("label")?.as_str()
    })?;
    let label: String = label.chars().filter(|c| !c.is_control()).take(40).collect();
    (!label.trim().is_empty()).then_some(label)
}

/// A permission request, refused: the "reject" option the agent offered, or `cancelled`.
fn refuse(req: &Value, cancelled: bool) -> Value {
    let opts = req.pointer("/params/options").and_then(Value::as_array).cloned().unwrap_or_default();
    let reject = opts.iter().find(|o| o.get("kind").and_then(Value::as_str).is_some_and(|k| k.starts_with("reject"))).and_then(|o| o.get("optionId").cloned());
    match reject.filter(|_| !cancelled) {
        Some(id) => json!({ "result": { "outcome": { "outcome": "selected", "optionId": id } } }),
        None => json!({ "result": { "outcome": { "outcome": "cancelled" } } }),
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let pid = self.child.id() as i32;
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_millis(800);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_are_refused() {
        let req = json!({ "params": { "options": [{ "optionId": "yes", "kind": "allow_once" }, { "optionId": "no", "kind": "reject_once" }] } });
        assert_eq!(refuse(&req, false), json!({ "result": { "outcome": { "outcome": "selected", "optionId": "no" } } }));
        assert_eq!(refuse(&req, true)["result"]["outcome"]["outcome"], "cancelled");
        // no reject option offered: cancelled, never allowed
        let only_allow = json!({ "params": { "options": [{ "optionId": "yes", "kind": "allow_always" }] } });
        assert_eq!(refuse(&only_allow, false)["result"]["outcome"]["outcome"], "cancelled");
    }

    /// A fake agent (a shell script) that asks for a permission and a file, then answers.
    #[test]
    fn a_session_with_a_fake_agent() {
        let dir = std::env::temp_dir().join(format!("dusty-acp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("agent.sh");
        let log = dir.join("replies.log");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
read init; echo '{{"jsonrpc":"2.0","id":0,"result":{{"protocolVersion":1}}}}'
read new; echo '{{"jsonrpc":"2.0","method":"auth/status_update","params":{{"accounts":[{{"label":"Plan X","active":true}}]}}}}'
echo '{{"jsonrpc":"2.0","id":1,"result":{{"sessionId":"s1","modes":{{"currentModeId":"agent","availableModes":[{{"id":"agent"}},{{"id":"plan"}}]}}}}}}'
read mode; echo "$mode" >> "{log}"; echo '{{"jsonrpc":"2.0","id":2,"result":{{}}}}'
read prompt; echo "$prompt" >> "{log}"
echo 'not json, ignored'
echo '{{"jsonrpc":"2.0","id":"p1","method":"session/request_permission","params":{{"options":[{{"optionId":"ok","kind":"allow_once"}},{{"optionId":"no","kind":"reject_once"}}]}}}}'
read reply; echo "$reply" >> "{log}"
echo '{{"jsonrpc":"2.0","id":"f1","method":"fs/read_text_file","params":{{"path":"/etc/hosts"}}}}'
read reply; echo "$reply" >> "{log}"
echo '{{"jsonrpc":"2.0","method":"session/update","params":{{"update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"hel"}}}}}}}}'
echo '{{"jsonrpc":"2.0","method":"session/update","params":{{"update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"lo"}}}}}}}}'
echo '{{"jsonrpc":"2.0","method":"session/update","params":{{"update":{{"sessionUpdate":"tool_call","toolCallId":"t","kind":"read","title":"ls"}}}}}}'
echo '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn","usage":{{"totalTokens":120}}}}}}'
read p2
echo '{{"jsonrpc":"2.0","method":"session/update","params":{{"update":{{"sessionUpdate":"tool_call","toolCallId":"e","kind":"edit","title":"write"}}}}}}'
echo '{{"jsonrpc":"2.0","id":4,"result":{{"stopReason":"end_turn"}}}}'
read never
"#,
                log = log.display()
            ),
        )
        .unwrap();
        let cmd = AgentCommand { program: "/bin/sh".into(), args: vec![script.to_string_lossy().into()], path_env: "/usr/bin:/bin".into(), session_meta: json!({}), safe_modes: vec!["read-only", "plan"], system_prompt: Some("be nice".into()) };
        let cancel = AtomicBool::new(false);
        let mut s = Session::start(&cmd, &dir, &cancel).unwrap();
        let mut pieces = vec![];
        assert_eq!(s.prompt("hi", &cancel, &mut |t| pieces.push(t.to_string())).unwrap(), "hello");
        assert_eq!(pieces, ["hel", "lo"]);
        assert_eq!((s.account.as_deref(), s.tokens, s.own_looks), (Some("Plan X"), 120, 1));
        // a change to a file ends it
        assert_eq!(s.prompt("again", &cancel, &mut |_| {}), Err(AcpError::Meddled));
        drop(s);
        let replies = std::fs::read_to_string(&log).unwrap();
        assert!(replies.contains(r#""modeId":"plan""#), "switched to the read-only mode: {replies}");
        assert!(replies.contains("be nice"), "instructions go with the first message: {replies}");
        assert!(replies.contains(r#""optionId":"no""#) && !replies.contains(r#""optionId":"ok""#), "{replies}");
        assert!(replies.contains("-32601"), "{replies}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
