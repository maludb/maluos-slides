//! The office this desk is enrolled with and its agents (step 4.5).
//!
//! A MaluOS desktop joins a Business OS *office* as a desk (`sudo maluctl office enroll`). The
//! enrolment leaves `/etc/maluos/office.yaml` (no secret in it) and the desk token in the owner's
//! vault compartment as `office-desk-token`. The office publishes the agents a desk may use at
//! `endpoints.agents`; each agent takes tasks over the MaluOS task protocol (`maluos-tasks/0.1`,
//! JSON-RPC 2.0 at its `tasks` URL, the desk token as a Bearer header): `agent/card`,
//! `tasks/send`, `tasks/get`, `tasks/list`. A picture comes back as an artifact of the answer.
//!
//! Slides uses this for one thing: a picture from the office's image agent, chosen by capability
//! (`image*` or `picture*`), never by a fixed id. Nothing is fetched from the public web: an
//! artifact given as a URL is read only when the URL is under the office's own URL. The Phase 13
//! delegation envelope is not shipped yet; when it lands it goes on the same requests.
//!
//! The token comes from `MALUOS_OFFICE_TOKEN` when set (the platform's own convention for agent
//! units), else from the vault over its Unix socket (`MALUOS_VAULT_SOCKET`, default
//! `/run/maluos/vault.sock`) with `mode=read`, which the owner's own processes may do.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The enrolment file `maluctl office` writes.
pub const DEFAULT_CONF: &str = "/etc/maluos/office.yaml";
/// The vault socket.
pub const DEFAULT_VAULT_SOCKET: &str = "/run/maluos/vault.sock";
/// The desk token's name in the owner's compartment.
pub const DESK_TOKEN_SECRET: &str = "office-desk-token";
/// Most bytes a picture artifact may carry.
pub const MAX_PICTURE_BYTES: usize = 16 * 1024 * 1024;

/// Why the office could not be used; the text is what the model and the user see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfficeError {
    /// No enrolment on this desk.
    NotEnrolled(String),
    /// The office is enrolled but publishes no agent that makes pictures.
    NoImageAgent(String),
    /// The token could not be read.
    Token(String),
    /// The office did not answer, or answered badly.
    Office(String),
    /// The agent's task failed or produced nothing usable.
    Task(String),
}

impl std::fmt::Display for OfficeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OfficeError::NotEnrolled(s) => write!(f, "this desk is not enrolled with an office ({s}); sudo maluctl office enroll URL --code CODE"),
            OfficeError::NoImageAgent(name) => write!(f, "the office {name} publishes no image agent to this desk"),
            OfficeError::Token(s) => write!(f, "the desk token could not be read: {s}"),
            OfficeError::Office(s) => write!(f, "the office did not answer: {s}"),
            OfficeError::Task(s) => write!(f, "the agent's task failed: {s}"),
        }
    }
}

impl std::error::Error for OfficeError {}

/// Where the desk token comes from.
#[derive(Debug, Clone)]
enum TokenSource {
    /// `MALUOS_OFFICE_TOKEN`, or a token handed in directly.
    Given(String),
    /// Read from the vault socket at this path.
    Vault(String),
}

/// The office this desk is enrolled with.
#[derive(Debug, Clone)]
pub struct Office {
    /// The office's display name.
    pub name: String,
    /// The office's base URL; artifacts by URL must live under it.
    pub url: String,
    /// `endpoints.agents`, when the office publishes agents to desks.
    pub agents_url: Option<String>,
    token: TokenSource,
    /// How long to wait for an agent's task.
    pub task_timeout: Duration,
    /// How often to ask for a working task.
    pub poll_every: Duration,
}

/// One agent the office publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub description: String,
    pub capabilities: Vec<String>,
    /// Its task URL.
    pub tasks: String,
}

impl Agent {
    /// Whether the agent says it makes pictures.
    pub fn makes_pictures(&self) -> bool {
        self.capabilities.iter().any(|c| {
            let c = c.to_ascii_lowercase();
            c == "image" || c.starts_with("image.") || c.starts_with("image/") || c.starts_with("picture") || c == "illustrate"
        })
    }
}

/// A picture an agent produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub name: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
    /// What the agent said beside the picture.
    pub text: String,
    /// The task id at the office.
    pub task_id: String,
}

static OVERRIDE: Mutex<Option<Office>> = Mutex::new(None);

impl Office {
    /// The office from the enrolment on this desk: `MALUOS_OFFICE_CONF` (default
    /// [`DEFAULT_CONF`]) for the endpoints, `MALUOS_OFFICE_TOKEN` or the vault for the token.
    /// An office installed with [`Office::set_override`] wins (tests, a CLI pointed elsewhere).
    pub fn discover() -> Result<Office, OfficeError> {
        if let Ok(guard) = OVERRIDE.lock()
            && let Some(o) = guard.as_ref()
        {
            return Ok(o.clone());
        }
        let path = std::env::var("MALUOS_OFFICE_CONF").unwrap_or_else(|_| DEFAULT_CONF.to_string());
        let text = std::fs::read_to_string(&path).map_err(|e| OfficeError::NotEnrolled(format!("{path}: {e}")))?;
        let mut office = Office::from_yaml(&text)?;
        if let Ok(t) = std::env::var("MALUOS_OFFICE_TOKEN")
            && !t.is_empty()
        {
            office.token = TokenSource::Given(t);
        } else {
            office.token = TokenSource::Vault(std::env::var("MALUOS_VAULT_SOCKET").unwrap_or_else(|_| DEFAULT_VAULT_SOCKET.to_string()));
        }
        Ok(office)
    }

    /// Use this office everywhere instead of the enrolment on disk (`None` restores discovery).
    pub fn set_override(office: Option<Office>) {
        if let Ok(mut guard) = OVERRIDE.lock() {
            *guard = office;
        }
    }

    /// The enrolment file's fields: `url`, `office.name`, `endpoints.agents`. The file is written
    /// by `maluctl office` in a fixed, flat shape, so a small reader is enough.
    pub fn from_yaml(text: &str) -> Result<Office, OfficeError> {
        let url = yaml_get(text, &["url"]).ok_or_else(|| OfficeError::NotEnrolled("the enrolment file has no url".into()))?;
        let name = yaml_get(text, &["office", "name"]).unwrap_or_else(|| url.clone());
        let agents_url = yaml_get(text, &["endpoints", "agents"]).filter(|s| s != "null" && !s.is_empty());
        Ok(Office {
            name,
            url: url.trim_end_matches('/').to_string(),
            agents_url,
            token: TokenSource::Vault(DEFAULT_VAULT_SOCKET.to_string()),
            task_timeout: Duration::from_secs(180),
            poll_every: Duration::from_millis(500),
        })
    }

    /// An office at `url` that publishes agents at `agents_url`, with the token given.
    pub fn with_token(name: &str, url: &str, agents_url: Option<&str>, token: &str) -> Office {
        Office {
            name: name.into(),
            url: url.trim_end_matches('/').into(),
            agents_url: agents_url.map(str::to_string),
            token: TokenSource::Given(token.into()),
            task_timeout: Duration::from_secs(180),
            poll_every: Duration::from_millis(500),
        }
    }

    /// Read the token from the vault at `socket` instead.
    pub fn with_vault(mut self, socket: &str) -> Office {
        self.token = TokenSource::Vault(socket.into());
        self
    }

    /// The desk token.
    pub fn token(&self) -> Result<String, OfficeError> {
        match &self.token {
            TokenSource::Given(t) => Ok(t.clone()),
            TokenSource::Vault(socket) => vault_value(socket, DESK_TOKEN_SECRET, "MaluOS Slides: a picture from the office's image agent"),
        }
    }

    /// The agents the office publishes to this desk.
    pub fn agents(&self) -> Result<Vec<Agent>, OfficeError> {
        let Some(url) = &self.agents_url else { return Ok(vec![]) };
        let token = self.token()?;
        let v = http_json("GET", url, &token, None, self.task_timeout)?;
        let Some(list) = v.get("agents").and_then(Value::as_array) else {
            return Err(OfficeError::Office(format!("{url}: no agents list in the answer")));
        };
        Ok(list
            .iter()
            .filter_map(|a| {
                Some(Agent {
                    id: a.get("id")?.as_str().map(str::to_string).or_else(|| a.get("id").map(Value::to_string))?,
                    name: a.get("name").and_then(Value::as_str).unwrap_or("").into(),
                    description: a.get("description").and_then(Value::as_str).unwrap_or("").into(),
                    capabilities: a
                        .get("capabilities")
                        .and_then(Value::as_array)
                        .map(|c| c.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default(),
                    tasks: a.get("tasks").and_then(Value::as_str)?.into(),
                })
            })
            .collect())
    }

    /// The first agent that makes pictures.
    pub fn image_agent(&self) -> Result<Agent, OfficeError> {
        self.agents()?.into_iter().find(Agent::makes_pictures).ok_or_else(|| OfficeError::NoImageAgent(self.name.clone()))
    }

    /// Ask `agent` for a picture of `prompt`: `tasks/send`, then `tasks/get` until the task is
    /// done, then the first image artifact of its output.
    pub fn picture(&self, agent: &Agent, prompt: &str) -> Result<Picture, OfficeError> {
        let token = self.token()?;
        let send = json!({"jsonrpc": "2.0", "id": 1, "method": "tasks/send",
            "params": {"message": {"parts": [{"text": prompt}]}, "kind": "image"}});
        let mut task = rpc_result(http_json("POST", &agent.tasks, &token, Some(&send), self.task_timeout)?)?;
        let id = task.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let started = Instant::now();
        while matches!(task.get("state").and_then(Value::as_str), Some("submitted" | "working")) {
            if started.elapsed() > self.task_timeout {
                return Err(OfficeError::Task(format!("task {id} still {} after {:?}", task["state"], self.task_timeout)));
            }
            std::thread::sleep(self.poll_every);
            let get = json!({"jsonrpc": "2.0", "id": 2, "method": "tasks/get", "params": {"id": id}});
            task = rpc_result(http_json("POST", &agent.tasks, &token, Some(&get), self.task_timeout)?)?;
        }
        if task.get("state").and_then(Value::as_str) != Some("completed") {
            let err = task.get("error").map(Value::to_string).unwrap_or_default();
            return Err(OfficeError::Task(format!("task {id} {}: {err}", task.get("state").and_then(Value::as_str).unwrap_or("?"))));
        }
        let output = task.get("output").cloned().unwrap_or(Value::Null);
        let text = output.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        let artifacts = output.get("artifacts").and_then(Value::as_array).cloned().unwrap_or_default();
        for a in &artifacts {
            if let Some(p) = self.picture_artifact(a, &token, &id, &text)? {
                return Ok(p);
            }
        }
        Err(OfficeError::Task(format!(
            "task {id} completed without a picture ({} artifact(s); the agent said: {})",
            artifacts.len(),
            if text.is_empty() { "nothing" } else { text.as_str() }
        )))
    }

    /// One artifact as a picture, when it is one: `{kind|type: image, media_type|mime|mediaType,
    /// data (base64) | url (under the office), name?}`; a media type that starts with `image/`
    /// counts too.
    fn picture_artifact(&self, a: &Value, token: &str, task_id: &str, text: &str) -> Result<Option<Picture>, OfficeError> {
        let kind = a.get("kind").or_else(|| a.get("type")).and_then(Value::as_str).unwrap_or("");
        let media_type = ["media_type", "mediaType", "mime", "mime_type", "content_type"]
            .iter()
            .find_map(|k| a.get(k).and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        if kind != "image" && !media_type.starts_with("image/") {
            return Ok(None);
        }
        let name = a.get("name").and_then(Value::as_str).unwrap_or("picture").to_string();
        let bytes = if let Some(data) = a.get("data").and_then(Value::as_str) {
            maluos_slides_engine::cmd::base64_decode(data).ok_or_else(|| OfficeError::Task(format!("artifact {name}: bad base64")))?
        } else if let Some(url) = a.get("url").and_then(Value::as_str) {
            if !under(url, &self.url) {
                return Err(OfficeError::Task(format!("artifact {name} points outside the office ({url}); nothing is fetched from the web")));
            }
            http_bytes(url, token, self.task_timeout)?
        } else {
            return Err(OfficeError::Task(format!("artifact {name} has neither data nor url")));
        };
        if bytes.len() > MAX_PICTURE_BYTES {
            return Err(OfficeError::Task(format!("artifact {name}: {} bytes, more than {MAX_PICTURE_BYTES}", bytes.len())));
        }
        let media_type = if media_type.is_empty() { "image/png".to_string() } else { media_type };
        Ok(Some(Picture { name, media_type, bytes, text: text.to_string(), task_id: task_id.to_string() }))
    }
}

/// `url` is the office's URL or a path below it.
fn under(url: &str, office: &str) -> bool {
    let office = office.trim_end_matches('/');
    url == office || url.starts_with(&format!("{office}/"))
}

fn rpc_result(v: Value) -> Result<Value, OfficeError> {
    if let Some(e) = v.get("error") {
        let msg = e.get("message").and_then(Value::as_str).unwrap_or("");
        let code = e.get("code").map(Value::to_string).unwrap_or_default();
        return Err(OfficeError::Task(format!("{msg} ({code})")));
    }
    v.get("result").cloned().ok_or_else(|| OfficeError::Office("the answer has no result".into()))
}

/// The four-level reader the enrolment file needs: `key:` at one indent, `key: value` below it.
fn yaml_get(text: &str, path: &[&str]) -> Option<String> {
    let mut depth = 0usize;
    let mut indent_of_level: Vec<usize> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let body = line.trim_start();
        let (key, value) = body.split_once(':')?;
        let value = value.trim();
        // Back out of nested maps we have left.
        while depth > 0 && indent <= indent_of_level[depth - 1] {
            depth -= 1;
            indent_of_level.pop();
        }
        if depth < path.len() && key.trim() == path[depth] {
            if depth + 1 == path.len() {
                return Some(unquote(value));
            }
            if value.is_empty() {
                indent_of_level.push(indent);
                depth += 1;
            }
        }
    }
    None
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2) || (s.starts_with('"') && s.ends_with('"') && s.len() >= 2) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

// ---------- the vault over its Unix socket ----------

/// `GET /v1/secrets/<name>/value?mode=read&purpose=…` as the calling user; the vault identifies
/// the caller by peer credentials, so there is no key to this door.
pub fn vault_value(socket: &str, name: &str, purpose: &str) -> Result<String, OfficeError> {
    let path = format!("/v1/secrets/{}/value?mode=read&purpose={}", percent(name), percent(purpose));
    let (status, body) = unix_get(socket, &path).map_err(|e| OfficeError::Token(format!("vault {socket}: {e}")))?;
    let v: Value = serde_json::from_slice(&body).map_err(|e| OfficeError::Token(format!("vault answered badly: {e}")))?;
    if status != 200 {
        let msg = v.get("message").and_then(Value::as_str).unwrap_or("");
        let code = v.get("error").and_then(Value::as_str).unwrap_or("error");
        return Err(OfficeError::Token(format!("vault {code} ({status}): {msg}")));
    }
    v.get("value").and_then(Value::as_str).map(str::to_string).ok_or_else(|| OfficeError::Token("vault answered without a value".into()))
}

fn percent(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// One HTTP/1.1 GET over a Unix socket: status and body.
fn unix_get(socket: &str, path: &str) -> std::io::Result<(u16, Vec<u8>)> {
    let mut s = UnixStream::connect(socket)?;
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    s.set_write_timeout(Some(Duration::from_secs(30)))?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: maluos-vault\r\nAccept: application/json\r\nConnection: close\r\n\r\n")?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| std::io::Error::other("no HTTP header in the answer"))?;
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let status = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut body = raw[head_end + 4..].to_vec();
    if let Some(len) = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
    }) {
        body.truncate(len);
    }
    Ok((status, body))
}

// ---------- the office over HTTP ----------

fn agent_for(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_global(Some(timeout + Duration::from_secs(30)))
        .user_agent("maluos-slides")
        .build()
        .new_agent()
}

fn http_json(method: &str, url: &str, token: &str, body: Option<&Value>, timeout: Duration) -> Result<Value, OfficeError> {
    let agent = agent_for(timeout);
    let bearer = format!("Bearer {token}");
    let result = match (method, body) {
        ("POST", Some(b)) => agent
            .post(url)
            .header("Authorization", &bearer)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(b.to_string().as_str()),
        _ => agent.get(url).header("Authorization", &bearer).header("Accept", "application/json").call(),
    };
    let mut response = result.map_err(|e| OfficeError::Office(format!("{url}: {e}")))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| OfficeError::Office(format!("{url}: {e}")))?;
    let v: Value =
        serde_json::from_str(&text).map_err(|_| OfficeError::Office(format!("{url}: {status} {}", text.chars().take(160).collect::<String>())))?;
    if status >= 400 {
        let msg =
            v.get("message").or_else(|| v.get("error")).map(|m| m.as_str().map(str::to_string).unwrap_or_else(|| m.to_string())).unwrap_or_default();
        return Err(OfficeError::Office(format!("{url}: {status} {msg}")));
    }
    Ok(v)
}

fn http_bytes(url: &str, token: &str, timeout: Duration) -> Result<Vec<u8>, OfficeError> {
    let agent = agent_for(timeout);
    let mut response =
        agent.get(url).header("Authorization", &format!("Bearer {token}")).call().map_err(|e| OfficeError::Office(format!("{url}: {e}")))?;
    let status = response.status().as_u16();
    if status >= 400 {
        return Err(OfficeError::Office(format!("{url}: {status}")));
    }
    response.body_mut().with_config().limit(MAX_PICTURE_BYTES as u64 + 1).read_to_vec().map_err(|e| OfficeError::Office(format!("{url}: {e}")))
}

#[cfg(test)]
mod unit {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn yaml_reader_finds_nested_keys() {
        let text = "# comment\nsurface: 1\nurl: http://127.0.0.1:18080\noffice:\n  name: Mock Office\n  url: http://x\ndesk:\n  id: 42\nendpoints:\n  heartbeat: http://h\n  actions: null\n  models:\n    url: http://m\n  agents: http://127.0.0.1:18080/api/v1/desks/agents\nenrolled_at: '2026-11-03T13:24:08Z'\n";
        assert_eq!(yaml_get(text, &["url"]).as_deref(), Some("http://127.0.0.1:18080"));
        assert_eq!(yaml_get(text, &["office", "name"]).as_deref(), Some("Mock Office"));
        assert_eq!(yaml_get(text, &["endpoints", "agents"]).as_deref(), Some("http://127.0.0.1:18080/api/v1/desks/agents"));
        assert_eq!(yaml_get(text, &["endpoints", "models", "url"]).as_deref(), Some("http://m"));
        assert_eq!(yaml_get(text, &["endpoints", "actions"]).as_deref(), Some("null"));
        assert_eq!(yaml_get(text, &["enrolled_at"]).as_deref(), Some("2026-11-03T13:24:08Z"));
        assert_eq!(yaml_get(text, &["desk", "name"]), None);
        let o = Office::from_yaml(text).unwrap();
        assert_eq!((o.name.as_str(), o.url.as_str()), ("Mock Office", "http://127.0.0.1:18080"));
        assert!(o.agents_url.is_some());
        assert!(Office::from_yaml("surface: 1\n").is_err());
    }

    #[test]
    fn only_urls_under_the_office_are_read() {
        assert!(under("http://o/x/y.png", "http://o"));
        assert!(under("http://o", "http://o/"));
        assert!(!under("http://o.evil/x.png", "http://o"));
        assert!(!under("https://example.com/x.png", "http://o"));
        assert_eq!(percent("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn capability_match() {
        let a = |caps: &[&str]| Agent {
            id: "a".into(),
            name: String::new(),
            description: String::new(),
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            tasks: String::new(),
        };
        assert!(a(&["image.generate"]).makes_pictures());
        assert!(a(&["Picture"]).makes_pictures());
        assert!(!a(&["research", "summarise"]).makes_pictures());
        assert!(!a(&["imagery-reports"]).makes_pictures());
    }
}
