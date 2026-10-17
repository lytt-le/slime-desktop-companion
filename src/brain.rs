//! The slime's voice: fun facts, and chat answered by one of three brains:
//! - your Claude plan, through a Claude Code session kept open while you chat
//!   (`claude -p` with streamed JSON in and out, so only the first answer pays for startup);
//! - the Anthropic Messages API with your own key;
//! - the DeepSeek API with your own key.
//! Requests run on a worker thread (WinHTTP or the Claude Code child process, so no extra
//! native deps); the main loop polls for replies so the slime never stops jiggling while it thinks.

use std::ffi::c_void;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::*;

use super::font::{to_ascii, wrap};
use super::ui::{BUBBLE_COLS, BUBBLE_ROWS};

const MAX_HISTORY: usize = 12;
/// A Claude Code session closes after this long without a question.
const SESSION_IDLE: Duration = Duration::from_secs(15 * 60);
/// Longest wait for one answer before giving up on the session.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

const SYSTEM: &str = "You are Slime, a small, cheerful green slime who lives on the user's Windows desktop. \
You sit on top of their windows, wobble about, and sometimes share fun facts. Answer the user's questions \
helpfully and accurately, in character: warm and a little playful, now and then mentioning being a slime, \
but never at the expense of a clear, correct answer. Your replies appear in a small pixel-art speech bubble, \
so keep them short: usually one to three sentences, under about 300 characters. Write plain text only, using \
ASCII characters: no markdown, lists, emoji or code blocks. If a question genuinely needs a longer answer, \
give the most useful short version.";

/// Added to the system prompt when he may look things up.
const WEB_NOTE: &str = "You can search the web and read web pages. Do so when a question needs current or \
specific information (news, weather, prices, sports, recent releases, or anything you're not sure of), then \
answer briefly in your own words. Don't list sources or URLs unless asked.";

/// Most search/read rounds DeepSeek gets before it must answer.
const MAX_TOOL_ROUNDS: usize = 4;

/// The system prompt: who he is, today's date, and whether he can search.
fn system(web: bool) -> String {
    let (y, m, d) = today();
    let mut s = format!("{SYSTEM} Today's date is {y}-{m:02}-{d:02}.");
    if web {
        s.push(' ');
        s.push_str(WEB_NOTE);
    }
    s
}

/// Today's UTC date as (year, month, day).
fn today() -> (i64, u32, u32) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    civil_from_days((secs / 86_400) as i64)
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub const FACTS: &[&str] = &[
    "Slime molds have no brain, yet they can find the shortest path through a maze to reach food.",
    "A slime mold once grew a network a lot like Tokyo's rail map when oat flakes were placed where the cities are.",
    "Hagfish can turn a bucket of water into slime in seconds to clog a predator's gills.",
    "Snail slime lets them glide and grip at the same time, so they can crawl upside down on glass.",
    "Honey basically never spoils. Edible honey has been found in ancient Egyptian tombs.",
    "Octopuses have three hearts and blue blood.",
    "Botanically, bananas are berries, but strawberries are not.",
    "A day on Venus is longer than its year.",
    "Sharks have been around longer than trees.",
    "Wombats make cube-shaped poop.",
    "There are more trees on Earth than stars in the Milky Way.",
    "Sea otters sometimes hold hands while they sleep so they don't drift apart.",
    "Your nose makes around a litre of mucus a day. Slime keeps you healthy!",
    "Jellyfish have drifted through the oceans for over 500 million years without a brain.",
    "Sunlight takes about 8 minutes to reach Earth.",
    "A group of flamingos is called a flamboyance.",
    "Wood frogs can freeze solid in winter and thaw out alive in spring.",
    "Tardigrades have survived being exposed to the vacuum of space.",
    "Butterflies taste with their feet.",
    "A teaspoon of neutron star would weigh about a billion tonnes.",
    "Koala fingerprints are so like ours they could confuse a crime scene.",
    "A lightning bolt is about five times hotter than the surface of the Sun.",
    "Octopuses can taste with their suckers.",
    "The shortest war on record, between Britain and Zanzibar in 1896, lasted about 38 minutes.",
    "Venus spins backwards compared to most of the planets.",
    "Slime molds can fuse together and share what they have learned.",
    "Saturn is less dense than water. In a big enough bath, it would float.",
    "A blue whale's heart is about the size of a small car.",
    "A pineapple takes around two years to grow.",
    "Honeybees can learn to recognise human faces.",
    "The man who designed the Pringles can asked for some of his ashes to be buried in one.",
    "Glass is made mostly from melted sand.",
    "Some slime molds can grow to cover several square metres.",
    "Earthworms breathe through their skin, which is why they need to stay slimy.",
    "Tip: right-click me to chat, or drag me somewhere comfy!",
];

/// Where chat answers come from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provider {
    /// The user's Claude subscription, via the Claude Code CLI.
    ClaudePlan,
    Anthropic,
    DeepSeek,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::ClaudePlan, Provider::Anthropic, Provider::DeepSeek];

    pub fn label(self) -> &'static str {
        match self {
            Provider::ClaudePlan => "Claude plan",
            Provider::Anthropic => "Anthropic API",
            Provider::DeepSeek => "DeepSeek API",
        }
    }

    /// Stable name for the settings file.
    pub fn id(self) -> &'static str {
        match self {
            Provider::ClaudePlan => "claude-plan",
            Provider::Anthropic => "anthropic",
            Provider::DeepSeek => "deepseek",
        }
    }

    pub fn from_id(id: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.id() == id)
    }

    /// (model id, label) pairs; the first is the default.
    pub fn models(self) -> &'static [(&'static str, &'static str)] {
        match self {
            // Claude Code model aliases.
            Provider::ClaudePlan => &[("haiku", "Haiku"), ("sonnet", "Sonnet"), ("opus", "Opus")],
            Provider::Anthropic => &[
                ("claude-haiku-5-5", "Haiku 5.5"),
                ("claude-sonnet-5-5", "Sonnet 5.5"),
                ("claude-opus-5-5", "Opus 5.5"),
            ],
            Provider::DeepSeek => &[("deepseek-flash", "V4.1 Flash"), ("deepseek-v4-pro", "V4 Pro")],
        }
    }

    pub fn needs_key(self) -> bool {
        self != Provider::ClaudePlan
    }

    fn key_env(self) -> &'static str {
        match self {
            Provider::DeepSeek => "DEEPSEEK_API_KEY",
            _ => "ANTHROPIC_API_KEY",
        }
    }

    fn key_file(self) -> &'static str {
        match self {
            Provider::DeepSeek => "deepseek_key.txt",
            _ => "api_key.txt",
        }
    }

    /// Whether `s` looks like a key for this provider (Anthropic keys start `sk-ant-`,
    /// DeepSeek keys `sk-`).
    pub fn looks_like_key(self, s: &str) -> bool {
        let s = s.trim();
        let shape = s.len() >= 20 && !s.contains(char::is_whitespace);
        match self {
            Provider::ClaudePlan => false,
            Provider::Anthropic => shape && s.starts_with("sk-ant-"),
            Provider::DeepSeek => shape && s.starts_with("sk-") && !s.starts_with("sk-ant-"),
        }
    }
}

/// %APPDATA%\SlimeCompanion, where settings and pasted keys live.
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("SlimeCompanion"))
}

/// Where a key comes from, for showing in the settings panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeySource {
    Env,
    File,
}

pub fn api_key(p: Provider) -> Option<(String, KeySource)> {
    if !p.needs_key() {
        return None;
    }
    // A key pasted into the panel wins over the environment variable.
    let saved = data_dir().and_then(|d| std::fs::read_to_string(d.join(p.key_file())).ok());
    if let Some(k) = saved.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
        return Some((k, KeySource::File));
    }
    let k = std::env::var(p.key_env()).ok()?.trim().to_string();
    (!k.is_empty()).then_some((k, KeySource::Env))
}

/// Saves a pasted key in plain text under %APPDATA%\SlimeCompanion.
pub fn save_key(p: Provider, key: &str) -> Result<(), String> {
    let dir = data_dir().ok_or("no APPDATA folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(p.key_file()), key.trim()).map_err(|e| e.to_string())
}

pub fn forget_key(p: Provider) {
    if let Some(dir) = data_dir() {
        let _ = std::fs::remove_file(dir.join(p.key_file()));
    }
}

/// `sk-...9Qa1`: enough to recognise a key without showing it.
pub fn mask_key(k: &str) -> String {
    let head: String = k.chars().take(3).collect();
    let tail: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}...{tail}")
}

/// The Claude Code CLI, if installed: on PATH, or where its installers put it.
pub fn claude_exe() -> Option<PathBuf> {
    static EXE: OnceLock<Option<PathBuf>> = OnceLock::new();
    EXE.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        if let Some(home) = std::env::var_os("USERPROFILE") {
            dirs.push(PathBuf::from(&home).join(".local").join("bin"));
            dirs.push(PathBuf::from(&home).join(".claude").join("local"));
        }
        dirs.into_iter().map(|d| d.join("claude.exe")).find(|p| p.is_file())
    })
    .clone()
}

/// What a request will actually use: the chosen provider, or the Claude plan standing in
/// for an API provider that has no key yet.
#[derive(Clone, Debug, PartialEq)]
pub enum Backend {
    Cli { exe: PathBuf, model: String },
    Anthropic { key: String, model: String },
    DeepSeek { key: String, model: String },
}

/// Picks the backend for `provider`/`model`, falling back to the Claude plan when an API
/// provider has no key. `Err` explains what's missing.
pub fn resolve(provider: Provider, model: &str, key: Option<String>, exe: Option<PathBuf>) -> Result<Backend, String> {
    let model = model.to_string();
    match (provider, key, exe) {
        (Provider::Anthropic, Some(key), _) => Ok(Backend::Anthropic { key, model }),
        (Provider::DeepSeek, Some(key), _) => Ok(Backend::DeepSeek { key, model }),
        // No key (or the plan was chosen): use Claude Code if it's installed.
        (Provider::ClaudePlan, _, Some(exe)) => Ok(Backend::Cli { exe, model }),
        (_, None, Some(exe)) => Ok(Backend::Cli { exe, model: Provider::ClaudePlan.models()[0].0.to_string() }),
        (Provider::ClaudePlan, _, None) => Err(
            "To use your Claude plan I need Claude Code installed and signed in. Or pick an API in my settings.".into(),
        ),
        (Provider::Anthropic, None, None) => {
            Err("I need an Anthropic API key to think! Right-click me, then Paste key (it starts with sk-ant-).".into())
        }
        (Provider::DeepSeek, None, None) => {
            Err("I need a DeepSeek API key to think! Right-click me, then Paste key (it starts with sk-).".into())
        }
    }
}

/// How the Claude Code session is doing, for the settings panel.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionState {
    Closed,
    Starting,
    Ready,
    Failed(String),
}

/// A picture sent along with a question (base64, as the APIs take it).
#[derive(Clone)]
pub struct Image {
    pub media_type: &'static str,
    pub base64: String,
}

impl Image {
    /// Anthropic-style image block (the Messages API and Claude Code).
    fn block(&self) -> Value {
        json!({"type": "image", "source": {"type": "base64", "media_type": self.media_type, "data": self.base64}})
    }
}

/// Content for the newest user turn: plain text, or the image then the text.
fn user_content(text: &str, image: Option<&Image>) -> Value {
    match image {
        None => json!(text),
        Some(img) => json!([img.block(), {"type": "text", "text": text}]),
    }
}

/// What the main loop gets back from the worker.
pub enum Reply {
    Text(String),
    Error(String),
    /// Still working: what he's looking up (the answer comes later).
    Searching(String),
}

enum Job {
    Ask { backend: Backend, history: Vec<(String, String)>, image: Option<Image>, web: bool },
    /// Start Claude Code now so the first answer comes quicker.
    Warm { exe: PathBuf, model: String, web: bool },
    /// Close the Claude Code session (settings changed).
    Close,
}

pub struct Brain {
    history: Vec<(String, String)>,
    tx: Sender<Job>,
    rx: Receiver<Reply>,
    pub waiting: bool,
    pub session: Arc<Mutex<SessionState>>,
    /// May he search the web?
    pub web: bool,
}

impl Brain {
    pub fn new() -> Brain {
        let (tx, worker_rx) = channel::<Job>();
        let (worker_tx, rx) = channel();
        let session = Arc::new(Mutex::new(SessionState::Closed));
        let state = session.clone();
        std::thread::spawn(move || worker(worker_rx, worker_tx, state));
        Brain { history: Vec::new(), tx, rx, waiting: false, session, web: true }
    }

    /// Sends a question, with a picture if given; the answer arrives later through `poll`.
    pub fn ask(&mut self, question: &str, backend: Backend, image: Option<Image>) -> Result<(), String> {
        self.history.push(("user".into(), question.to_string()));
        let excess = self.history.len().saturating_sub(MAX_HISTORY);
        self.history.drain(..excess);
        // History must start with a user turn.
        while self.history.first().is_some_and(|m| m.0 != "user") {
            self.history.remove(0);
        }
        let job = Job::Ask { backend, history: self.history.clone(), image, web: self.web };
        self.tx.send(job).map_err(|e| e.to_string())?;
        self.waiting = true;
        Ok(())
    }

    pub fn warm(&self, backend: &Backend) {
        if let Backend::Cli { exe, model } = backend {
            let _ = self.tx.send(Job::Warm { exe: exe.clone(), model: model.clone(), web: self.web });
        }
    }

    pub fn close_session(&self) {
        let _ = self.tx.send(Job::Close);
    }

    pub fn session_state(&self) -> SessionState {
        self.session.lock().map(|s| s.clone()).unwrap_or(SessionState::Closed)
    }

    pub fn poll(&mut self) -> Option<Reply> {
        let reply = self.rx.try_recv().ok()?;
        if matches!(reply, Reply::Searching(_)) {
            return Some(reply);
        }
        self.waiting = false;
        match &reply {
            Reply::Text(t) => self.history.push(("assistant".into(), t.clone())),
            // Drop the unanswered question so the history keeps alternating.
            Reply::Searching(_) => {}
            Reply::Error(_) => {
                if self.history.last().is_some_and(|m| m.0 == "user") {
                    self.history.pop();
                }
            }
        }
        Some(reply)
    }
}

fn worker(jobs: Receiver<Job>, replies: Sender<Reply>, state: Arc<Mutex<SessionState>>) {
    let set = |s: SessionState| {
        if let Ok(mut g) = state.lock() {
            *g = s;
        }
    };
    let mut session: Option<Session> = None;
    loop {
        let job = match jobs.recv_timeout(SESSION_IDLE) {
            Ok(job) => job,
            Err(RecvTimeoutError::Timeout) => {
                if session.take().is_some() {
                    set(SessionState::Closed);
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        match job {
            Job::Close => {
                session = None;
                set(SessionState::Closed);
            }
            Job::Warm { exe, model, web } => {
                if session.as_ref().map_or(true, |s| s.model != model || s.web != web) {
                    session = None;
                    set(SessionState::Starting);
                    match Session::start(&exe, &model, web) {
                        Ok(s) => {
                            session = Some(s);
                            set(SessionState::Ready);
                        }
                        Err(e) => set(SessionState::Failed(e)),
                    }
                }
            }
            Job::Ask { backend, history, image, web } => {
                let image = image.as_ref();
                let progress = |what: String| {
                    let _ = replies.send(Reply::Searching(what));
                };
                let reply = match backend {
                    Backend::Anthropic { key, model } => ask_anthropic(&history, &key, &model, image, web),
                    Backend::DeepSeek { key, model } => ask_deepseek(&history, &key, &model, image, web, &progress),
                    Backend::Cli { exe, model } => {
                        let fresh = session.as_mut().map_or(true, |s| s.model != model || s.web != web || !s.alive());
                        if fresh {
                            session = None;
                            set(SessionState::Starting);
                            match Session::start(&exe, &model, web) {
                                Ok(s) => session = Some(s),
                                Err(e) => set(SessionState::Failed(e)),
                            }
                        }
                        match session.as_mut() {
                            None => Reply::Error(
                                "I couldn't start Claude Code. Is it installed? Try running claude in a terminal.".into(),
                            ),
                            Some(s) => {
                                let first = s.turns == 0;
                                let text = cli_prompt(&history, first);
                                match s.ask(&text, image, &progress) {
                                    Ok(t) => {
                                        set(SessionState::Ready);
                                        Reply::Text(t)
                                    }
                                    Err(e) => {
                                        session = None;
                                        set(SessionState::Failed(e.clone()));
                                        Reply::Error(e)
                                    }
                                }
                            }
                        }
                    }
                };
                if replies.send(reply).is_err() {
                    return;
                }
            }
        }
    }
}

/// The text sent to Claude Code for the newest question. A session keeps its own memory,
/// so only a fresh one (new model, or restarted) is told about the chat so far.
fn cli_prompt(history: &[(String, String)], fresh: bool) -> String {
    let Some((_, question)) = history.last() else { return String::new() };
    if !fresh || history.len() < 2 {
        return question.clone();
    }
    let mut s = String::from("(Our chat so far:\n");
    for (role, text) in &history[..history.len() - 1] {
        s.push_str(if role == "user" { "Me: " } else { "You: " });
        s.push_str(text);
        s.push('\n');
    }
    s.push_str(")\n\n");
    s.push_str(question);
    s
}

/// A Claude Code process in streaming-JSON mode: questions go in on stdin one JSON line
/// each, and every answer ends with a `result` line on stdout. It stays open between
/// questions, so only the first one waits for startup.
struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    stderr: Arc<Mutex<String>>,
    model: String,
    web: bool,
    turns: usize,
}

impl Session {
    fn start(exe: &std::path::Path, model: &str, web: bool) -> Result<Session, String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // A neutral working directory, so no project's CLAUDE.md or settings join in.
        let cwd = data_dir().unwrap_or_else(std::env::temp_dir);
        let _ = std::fs::create_dir_all(&cwd);
        // A chat buddy, not an agent: at most web search and page reading (no files, shell,
        // MCP servers, skills or saved transcripts).
        let tools = if web { "WebSearch,WebFetch" } else { "" };
        let mut child = Command::new(exe)
            .args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
            .args(["--model", model, "--system-prompt", &system(web)])
            .args(["--tools", tools, "--allowedTools", tools])
            .args(["--strict-mcp-config", "--disable-slash-commands", "--no-session-persistence"])
            .args(["--setting-sources", ""])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("couldn't start Claude Code: {e}"))?;
        close_with_us(&child);
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let err_pipe = child.stderr.take().ok_or("no stderr")?;
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = stderr.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(err_pipe).lines().map_while(Result::ok) {
                if let Ok(mut s) = sink.lock() {
                    if s.len() < 4000 {
                        s.push_str(&line);
                        s.push('\n');
                    }
                }
            }
        });
        Ok(Session { child, stdin, lines, stderr, model: model.to_string(), web, turns: 0 })
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn ask(&mut self, text: &str, image: Option<&Image>, progress: &dyn Fn(String)) -> Result<String, String> {
        // Anything left over from an earlier, abandoned turn.
        while self.lines.try_recv().is_ok() {}
        let msg = json!({"type": "user", "message": {"role": "user", "content": user_content(text, image)}});
        writeln!(self.stdin, "{msg}")
            .and_then(|_| self.stdin.flush())
            .map_err(|_| self.died())?;
        loop {
            match self.lines.recv_timeout(ANSWER_TIMEOUT) {
                Ok(line) => {
                    if let Some(result) = parse_cli_line(&line) {
                        self.turns += 1;
                        return result;
                    }
                    if let Some(what) = cli_tool_use(&line) {
                        progress(what);
                    }
                }
                Err(RecvTimeoutError::Timeout) => return Err("I thought too long and lost my place. Ask me again?".into()),
                Err(RecvTimeoutError::Disconnected) => return Err(self.died()),
            }
        }
    }

    /// A friendly reason for the process ending early.
    fn died(&mut self) -> String {
        let _ = self.child.wait();
        let err = self.stderr.lock().map(|s| s.clone()).unwrap_or_default();
        explain_cli(&err)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Puts `child` in a job that Windows kills when this process ends, so a Claude Code
/// session never outlives the slime (even if it crashes).
fn close_with_us(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::*;
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        job as usize
    });
    unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) };
}

/// One question (with a picture) through a fresh Claude Code session, for tests.
#[cfg(test)]
pub fn ask_cli_once(text: &str, image: &Image) -> Result<String, String> {
    let exe = claude_exe().ok_or("Claude Code not installed")?;
    Session::start(&exe, "haiku", false)?.ask(text, Some(image), &|_| {})
}

/// One question through a fresh Claude Code session that may search, for tests: the
/// answer and what it looked up.
#[cfg(test)]
pub fn ask_cli_web(text: &str) -> (Result<String, String>, Vec<String>) {
    let Some(exe) = claude_exe() else { return (Err("Claude Code not installed".into()), vec![]) };
    let seen = std::cell::RefCell::new(Vec::new());
    let answer = Session::start(&exe, "haiku", true).and_then(|mut s| s.ask(text, None, &|w| seen.borrow_mut().push(w)));
    (answer, seen.into_inner())
}

/// What Claude Code is looking up, if this stdout line starts a web search or page read.
fn cli_tool_use(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v["type"] != "assistant" {
        return None;
    }
    v["message"]["content"].as_array()?.iter().filter(|b| b["type"] == "tool_use").find_map(|b| {
        match b["name"].as_str()? {
            "WebSearch" => Some(searching(b["input"]["query"].as_str().unwrap_or(""))),
            "WebFetch" => Some(reading(b["input"]["url"].as_str().unwrap_or(""))),
            _ => None,
        }
    })
}

/// Progress notes for the bubble.
fn searching(query: &str) -> String {
    let q: String = query.chars().take(40).collect();
    if q.is_empty() { "Searching the web...".into() } else { format!("Searching for \"{q}\"...") }
}

fn reading(url: &str) -> String {
    let host = url.split("//").nth(1).unwrap_or(url).split('/').next().unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host.is_empty() { "Reading a page...".into() } else { format!("Reading {host}...") }
}

/// The answer, if this stdout line ends a turn.
fn parse_cli_line(line: &str) -> Option<Result<String, String>> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v["type"] != "result" {
        return None;
    }
    let text = v["result"].as_str().unwrap_or("").trim().to_string();
    if v["is_error"] == true || v["subtype"] != "success" {
        let detail = if text.is_empty() { v["subtype"].as_str().unwrap_or("error").to_string() } else { text };
        return Some(Err(explain_cli(&detail)));
    }
    Some(Ok(if text.is_empty() { "...blorp? I lost my train of thought.".into() } else { text }))
}

fn explain_cli(detail: &str) -> String {
    let d = detail.to_lowercase();
    if d.contains("login") || d.contains("log in") || d.contains("logged in") || d.contains("authenticat") {
        "Claude Code isn't signed in. Run claude in a terminal and use /login, then ask me again.".into()
    } else if d.contains("limit") {
        "I've hit your Claude plan's usage limit for now. Try again later, or switch to an API key in my settings.".into()
    } else if d.contains("overloaded") || d.contains("529") {
        "My brain is a bit overloaded. Try again in a moment.".into()
    } else if d.trim().is_empty() {
        "Claude Code stopped unexpectedly. Ask me again?".into()
    } else {
        let line = detail.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        format!("Claude Code said: {}", line.chars().take(160).collect::<String>())
    }
}

/// One request with a single retry for rate limits, overload and network hiccups.
fn post_with_retry(host: &str, path: &str, headers: &str, body: &str) -> Result<(u32, String), String> {
    let mut attempt = 0;
    loop {
        let result = post(host, path, headers, body);
        let retry = match &result {
            Err(_) => true,
            Ok((status, _)) => *status == 429 || *status >= 500,
        };
        if retry && attempt == 0 {
            attempt += 1;
            std::thread::sleep(Duration::from_millis(1500));
            continue;
        }
        return result;
    }
}

/// History as Messages API turns, the picture (if any) going with the newest question.
fn turns(history: &[(String, String)], image: Option<&Image>, content: impl Fn(&str, Option<&Image>) -> Value) -> Vec<Value> {
    let last = history.len().saturating_sub(1);
    history
        .iter()
        .enumerate()
        .map(|(i, (r, t))| json!({"role": r, "content": if i == last { content(t, image) } else { json!(t) }}))
        .collect()
}

/// Anthropic's server-side web tools: search and page fetch with dynamic filtering where
/// the model has it, plain search on Haiku.
fn anthropic_web_tools(model: &str) -> Value {
    if model.contains("haiku") {
        json!([{"type": "web_search_20250305", "name": "web_search", "max_uses": 3}])
    } else {
        json!([
            {"type": "web_search_20260209", "name": "web_search", "max_uses": 3},
            {"type": "web_fetch_20260209", "name": "web_fetch", "max_uses": 2},
        ])
    }
}

fn ask_anthropic(history: &[(String, String)], key: &str, model: &str, image: Option<&Image>, web: bool) -> Reply {
    let mut messages = turns(history, image, user_content);
    let headers = format!("Content-Type: application/json\r\nx-api-key: {key}\r\nanthropic-version: 2023-06-01\r\n");
    let mut web = web;
    // The server runs the searches; a long one pauses and is resumed by sending it back.
    for _ in 0..4 {
        let mut body = json!({
            "model": model,
            "max_tokens": if web { 2048 } else { 1024 },
            "output_config": {"effort": "low"},
            "system": system(web),
            "messages": messages,
        });
        if web {
            body["tools"] = anthropic_web_tools(model);
        }
        match post_with_retry("api.anthropic.com", "/v1/messages", &headers, &body.to_string()) {
            Err(e) => return Reply::Error(format!("I can't reach my brain right now ({e}). Are you online?")),
            Ok((200, text)) => {
                let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                if v["stop_reason"] == "pause_turn" {
                    messages.push(json!({"role": "assistant", "content": v["content"]}));
                    continue;
                }
                return parse_reply(&text);
            }
            // Web tools turned off for this organisation, or not on this model: answer without.
            Ok((400, _)) if web => web = false,
            Ok((status, text)) => return Reply::Error(explain_error(status, &text)),
        }
    }
    Reply::Error("I got lost searching. Ask me again?".into())
}

fn parse_reply(text: &str) -> Reply {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Reply::Error("I got a garbled answer. Try again?".into());
    };
    if v["stop_reason"] == "refusal" {
        return Reply::Text("Hmm, that's not something I can help with. Ask me something else?".into());
    }
    // The answer is the text after the last search or fetch ("Let me look that up" comes
    // before it). Cited passages arrive as separate pieces of one sentence.
    let answer: String = v["content"]
        .as_array()
        .map(|blocks| {
            let start = blocks.iter().rposition(|b| b["type"] != "text").map_or(0, |i| i + 1);
            let after = &blocks[start..];
            let text = if after.is_empty() { &blocks[..] } else { after };
            text.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<String>()
        })
        .unwrap_or_default();
    let answer = answer.trim();
    if answer.is_empty() {
        Reply::Text("...blorp? I lost my train of thought.".into())
    } else {
        Reply::Text(answer.to_string())
    }
}

fn explain_error(status: u32, text: &str) -> String {
    let detail = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_default();
    match status {
        401 => "That API key didn't work. Right-click me and paste a new one.".into(),
        402 => "That account is out of credit. Top it up, or pick another brain in my settings.".into(),
        403 => "My key isn't allowed to do that. Check its permissions?".into(),
        429 => "Too many questions at once! Give me a moment.".into(),
        529 | 500..=599 => "My brain is a bit overloaded. Try again in a moment.".into(),
        _ => format!("Something went wrong ({status}): {detail}"),
    }
}

/// DeepSeek's OpenAI-style chat completions, with thinking off for snappy replies.
/// DeepSeek takes pictures as data URLs, and only V4.1 Flash can see them. With `web`, it
/// gets search and page-reading functions, which run here against DeepSeek's Search and
/// Fetch APIs, for a few rounds before it has to answer.
fn ask_deepseek(
    history: &[(String, String)],
    key: &str,
    model: &str,
    image: Option<&Image>,
    web: bool,
    progress: &dyn Fn(String),
) -> Reply {
    let model = if image.is_some() { "deepseek-flash" } else { model };
    let mut messages = vec![json!({"role": "system", "content": system(web)})];
    messages.extend(turns(history, image, |t, img| match img {
        None => json!(t),
        Some(img) => json!([
            {"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", img.media_type, img.base64)}},
            {"type": "text", "text": t},
        ]),
    }));
    let question = history.last().map_or("", |m| m.1.as_str());
    let headers = format!("Content-Type: application/json\r\nAuthorization: Bearer {key}\r\n");
    for round in 0..=MAX_TOOL_ROUNDS {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "max_tokens": 1024,
            "thinking": {"type": "disabled"},
            "stream": false,
        });
        if web {
            body["tools"] = deepseek_tools();
            // Out of rounds: answer with what it found.
            body["tool_choice"] = json!(if round == MAX_TOOL_ROUNDS { "none" } else { "auto" });
        }
        let text = match post_with_retry("api.deepseek.com", "/chat/completions", &headers, &body.to_string()) {
            Err(e) => return Reply::Error(format!("I can't reach my brain right now ({e}). Are you online?")),
            Ok((200, text)) => text,
            Ok((status, text)) => return Reply::Error(explain_error(status, &text)),
        };
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let message = &v["choices"][0]["message"];
        let calls = message["tool_calls"].as_array().filter(|c| !c.is_empty());
        let Some(calls) = calls else { return parse_openai_reply(&text) };
        messages.push(message.clone());
        for call in calls {
            let args: Value = call["function"]["arguments"].as_str().and_then(|a| serde_json::from_str(a).ok()).unwrap_or(Value::Null);
            let result = match call["function"]["name"].as_str() {
                Some("web_search") => {
                    let query = args["query"].as_str().unwrap_or("");
                    progress(searching(query));
                    deepseek_search(&headers, query, question)
                }
                Some("fetch_page") => {
                    let url = args["url"].as_str().unwrap_or("");
                    progress(reading(url));
                    deepseek_fetch(&headers, url)
                }
                _ => "Unknown tool.".to_string(),
            };
            messages.push(json!({"role": "tool", "tool_call_id": call["id"], "content": result}));
        }
    }
    Reply::Error("I got lost searching. Ask me again?".into())
}

/// The functions DeepSeek may call when he can search.
fn deepseek_tools() -> Value {
    json!([
        {"type": "function", "function": {
            "name": "web_search",
            "description": "Search the web. Use it for current events, news, weather, prices, sports, recent \
releases, or facts you are unsure of. Returns titles, URLs, dates and summaries.",
            "parameters": {"type": "object", "properties": {"query": {"type": "string",
                "description": "What to search for, at most 128 characters. Operators like site:example.com, \
\"exact phrase\", lang:en and after:2026 work."}}, "required": ["query"]},
        }},
        {"type": "function", "function": {
            "name": "fetch_page",
            "description": "Read a web page as text, for example a search result worth a closer look.",
            "parameters": {"type": "object", "properties": {"url": {"type": "string",
                "description": "The http(s) URL to read."}}, "required": ["url"]},
        }},
    ])
}

/// Text cut to at most `n` characters.
fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// DeepSeek Search: the top results, trimmed, as JSON for the model.
fn deepseek_search(headers: &str, query: &str, question: &str) -> String {
    let query = clip(query.trim(), 128);
    if query.is_empty() {
        return "Search failed: the query was empty.".into();
    }
    let mut body = json!({"query": query, "count": 6});
    let question = clip(question.trim(), 128);
    if !question.is_empty() {
        body["ori_question"] = json!(question);
    }
    match post_with_retry("api.deepseek.com", "/v1/tools/search", headers, &body.to_string()) {
        Ok((200, text)) => search_results(&text),
        Ok((status, text)) => format!("Search failed: {}", tool_error(status, &text)),
        Err(e) => format!("Search failed: {e}"),
    }
}

fn search_results(text: &str) -> String {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let results: Vec<Value> = v["results"]
        .as_array()
        .map(|r| {
            r.iter()
                .map(|r| {
                    let about = r["summary"].as_str().filter(|s| !s.is_empty()).or(r["snippet"].as_str()).unwrap_or("");
                    json!({"title": r["title"], "url": r["url"], "date": r["date"], "summary": clip(about, 700)})
                })
                .collect()
        })
        .unwrap_or_default();
    if results.is_empty() { "No results.".into() } else { Value::Array(results).to_string() }
}

/// DeepSeek Fetch: the page as Markdown, trimmed.
fn deepseek_fetch(headers: &str, url: &str) -> String {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return "Fetch failed: that isn't an http(s) URL.".into();
    }
    let body = json!({"url": url}).to_string();
    match post_with_retry("api.deepseek.com", "/v1/tools/fetch", headers, &body) {
        Ok((200, text)) => {
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let content = v["content"].as_str().unwrap_or("");
            if content.is_empty() {
                "The page had no readable content.".into()
            } else {
                format!("# {}\n\n{}", v["title"].as_str().unwrap_or(""), clip(content, 8000))
            }
        }
        Ok((status, text)) => format!("Fetch failed: {}", tool_error(status, &text)),
        Err(e) => format!("Fetch failed: {e}"),
    }
}

/// Why a Search or Fetch call failed, for the model to work around.
fn tool_error(status: u32, text: &str) -> String {
    let detail = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_default();
    let why = match status {
        402 => "the account is out of credit",
        403 => "that page is blocked",
        404 => "no usable content",
        429 => "too many requests",
        _ => "",
    };
    [status.to_string(), why.to_string(), clip(&detail, 200)].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ")
}

fn parse_openai_reply(text: &str) -> Reply {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Reply::Error("I got a garbled answer. Try again?".into());
    };
    let choice = &v["choices"][0];
    if choice["finish_reason"] == "content_filter" {
        return Reply::Text("Hmm, that's not something I can help with. Ask me something else?".into());
    }
    match choice["message"]["content"].as_str().map(str::trim) {
        Some(t) if !t.is_empty() => Reply::Text(t.to_string()),
        _ => Reply::Text("...blorp? I lost my train of thought.".into()),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A blocking HTTPS POST via WinHTTP. Returns the status code and response body.
fn post(host: &str, path: &str, headers: &str, body: &str) -> Result<(u32, String), String> {
    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }
    let fail = |what: &str| format!("{what} failed: {}", unsafe { GetLastError() });
    unsafe {
        let agent = wide("SlimeCompanion/1.0");
        let session = Handle(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(fail("WinHttpOpen"));
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 60_000);
        let host_w = wide(host);
        let connect = Handle(WinHttpConnect(session.0, host_w.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT as u16, 0));
        if connect.0.is_null() {
            return Err(fail("connect"));
        }
        let verb = wide("POST");
        let path_w = wide(path);
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            verb.as_ptr(),
            path_w.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(fail("open request"));
        }
        let headers_w: Vec<u16> = headers.encode_utf16().collect();
        let body_bytes = body.as_bytes();
        if WinHttpSendRequest(
            request.0,
            headers_w.as_ptr(),
            headers_w.len() as u32,
            body_bytes.as_ptr() as *const c_void,
            body_bytes.len() as u32,
            body_bytes.len() as u32,
            0,
        ) == 0
        {
            return Err(fail("send"));
        }
        if WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0 {
            return Err(fail("receive"));
        }
        let mut status: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status as *mut u32 as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
        );
        let mut out = Vec::new();
        loop {
            let mut avail: u32 = 0;
            if WinHttpQueryDataAvailable(request.0, &mut avail) == 0 {
                return Err(fail("read"));
            }
            if avail == 0 {
                break;
            }
            let start = out.len();
            out.resize(start + avail as usize, 0u8);
            let mut read: u32 = 0;
            if WinHttpReadData(request.0, out[start..].as_mut_ptr() as *mut c_void, avail, &mut read) == 0 {
                return Err(fail("read"));
            }
            out.truncate(start + read as usize);
        }
        Ok((status, String::from_utf8_lossy(&out).into_owned()))
    }
}

/// What the bubble is showing: a message typed out a page at a time, or a thinking "...".
pub struct Speech {
    pages: Vec<Vec<String>>,
    page: usize,
    shown: f32,
    hold: f32,
    pub thinking: bool,
    think_t: f32,
    /// Shown above the thinking dots (what he's looking up).
    note: Vec<String>,
}

const TYPE_SPEED: f32 = 45.0; // characters per second

impl Speech {
    pub fn new() -> Speech {
        Speech { pages: Vec::new(), page: 0, shown: 0.0, hold: 0.0, thinking: false, think_t: 0.0, note: Vec::new() }
    }

    pub fn say(&mut self, text: &str) {
        let lines = wrap(&to_ascii(text), BUBBLE_COLS);
        self.pages = lines.chunks(BUBBLE_ROWS).map(|c| c.to_vec()).collect();
        self.page = 0;
        self.shown = 0.0;
        self.hold = 0.0;
        self.thinking = false;
    }

    pub fn think(&mut self) {
        self.pages.clear();
        self.thinking = true;
        self.think_t = 0.0;
        self.note.clear();
    }

    /// Still thinking, with a note of what he's doing (like a web search).
    pub fn think_about(&mut self, note: &str) {
        self.think();
        self.note = wrap(&to_ascii(note), BUBBLE_COLS);
        self.note.truncate(BUBBLE_ROWS - 1);
    }

    pub fn busy(&self) -> bool {
        self.thinking || !self.pages.is_empty()
    }

    pub fn update(&mut self, dt: f32) {
        if self.thinking {
            self.think_t += dt;
            return;
        }
        let Some(page) = self.pages.get(self.page) else { return };
        let total: usize = page.iter().map(|l| l.len() + 1).sum();
        if (self.shown as usize) < total {
            self.shown += dt * TYPE_SPEED;
            return;
        }
        // Fully shown: hold long enough to read, then the next page or done.
        self.hold += dt;
        let read_time = 2.5 + total as f32 * 0.05;
        if self.hold > read_time {
            self.page += 1;
            self.shown = 0.0;
            self.hold = 0.0;
            if self.page >= self.pages.len() {
                self.pages.clear();
            }
        }
    }

    /// Lines to draw and how many characters of them are revealed.
    pub fn view(&self) -> Option<(Vec<String>, usize)> {
        if self.thinking {
            let dots = 1 + (self.think_t * 3.0) as usize % 3;
            let mut lines = self.note.clone();
            lines.push(".".repeat(dots) + &" ".repeat(3 - dots));
            let shown = lines.iter().map(|l| l.len() + 1).sum();
            return Some((lines, shown));
        }
        let page = self.pages.get(self.page)?;
        Some((page.clone(), self.shown as usize))
    }
}


/// Typed or pasted text, reduced to what the pixel font can show (keys stay intact).
pub fn clean_input(s: &str) -> String {
    to_ascii(s).chars().filter(|c| (' '..='~').contains(c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_and_refusals() {
        let ok = r#"{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"Blorp! Hello."}],"stop_reason":"end_turn"}"#;
        assert!(matches!(parse_reply(ok), Reply::Text(t) if t == "Blorp! Hello."));
        let refused = r#"{"content":[],"stop_reason":"refusal"}"#;
        assert!(matches!(parse_reply(refused), Reply::Text(t) if t.contains("not something I can help")));
        assert!(explain_error(401, "{}").contains("API key"));
        assert!(explain_error(529, "{}").contains("overloaded"));
        let ds = r#"{"choices":[{"index":0,"message":{"role":"assistant","content":" Blorp! "},"finish_reason":"stop"}]}"#;
        assert!(matches!(parse_openai_reply(ds), Reply::Text(t) if t == "Blorp!"));
    }

    #[test]
    fn reads_claude_code_results() {
        assert!(parse_cli_line(r#"{"type":"assistant","message":{}}"#).is_none());
        assert!(parse_cli_line("not json").is_none());
        let ok = r#"{"type":"result","subtype":"success","is_error":false,"result":"*squish* Hi, friend!"}"#;
        assert_eq!(parse_cli_line(ok), Some(Ok("*squish* Hi, friend!".to_string())));
        let login = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#;
        assert!(matches!(parse_cli_line(login), Some(Err(e)) if e.contains("signed in")));
    }

    #[test]
    fn picks_a_backend_and_falls_back_to_the_plan() {
        let exe = Some(PathBuf::from("claude.exe"));
        let key = Some("sk-test".to_string());
        assert!(matches!(resolve(Provider::DeepSeek, "deepseek-flash", key.clone(), exe.clone()),
            Ok(Backend::DeepSeek { model, .. }) if model == "deepseek-flash"));
        assert!(matches!(resolve(Provider::Anthropic, "claude-opus-5-5", None, exe.clone()),
            Ok(Backend::Cli { model, .. }) if model == "haiku"));
        assert!(matches!(resolve(Provider::ClaudePlan, "sonnet", None, exe), Ok(Backend::Cli { model, .. }) if model == "sonnet"));
        assert!(resolve(Provider::DeepSeek, "deepseek-flash", None, None).unwrap_err().contains("DeepSeek"));
        assert!(Provider::DeepSeek.looks_like_key("sk-0123456789abcdef0123456789abcdef"));
        assert!(!Provider::DeepSeek.looks_like_key("sk-ant-api03-0123456789abcdef"));
        assert!(Provider::Anthropic.looks_like_key("sk-ant-api03-0123456789abcdef"));
        assert_eq!(mask_key("sk-ant-api03-abcdefgh9Qa1"), "sk-...9Qa1");
    }

    #[test]
    fn fresh_sessions_hear_the_chat_so_far() {
        let h = vec![("user".to_string(), "hi".to_string()), ("assistant".into(), "Blorp!".into()), ("user".into(), "and?".into())];
        assert_eq!(cli_prompt(&h, false), "and?");
        let p = cli_prompt(&h, true);
        assert!(p.contains("Me: hi") && p.contains("You: Blorp!") && p.ends_with("and?"));
    }

    /// Real network round trips with fake keys: proves TLS, headers and error handling.
    /// Run with `cargo test --release -- --ignored`.
    #[test]
    #[ignore]
    fn reaches_the_apis() {
        let q = [("user".to_string(), "hi".to_string())];
        let replies = [
            ask_anthropic(&q, "sk-ant-not-a-real-key", "claude-haiku-5-5", None, true),
            ask_anthropic(&q, "sk-ant-not-a-real-key", "claude-sonnet-5-5", None, true),
            ask_deepseek(&q, "sk-notarealkey", "deepseek-flash", None, true, &|_| {}),
        ];
        for reply in replies {
            match reply {
                Reply::Error(e) => {
                    eprintln!("API said: {e}");
                    assert!(e.contains("API key"), "unexpected: {e}");
                }
                Reply::Text(t) | Reply::Searching(t) => panic!("fake key answered: {t}"),
            }
        }
        // DeepSeek's Search API with a fake key fails politely, for the model to read.
        let headers = "Content-Type: application/json\r\nAuthorization: Bearer sk-notarealkey\r\n";
        let r = deepseek_search(headers, "slime molds", "what are slime molds");
        eprintln!("search said: {r}");
        assert!(r.starts_with("Search failed: 401"), "unexpected: {r}");
    }

    /// DeepSeek searches with its Search API and answers. (Uses the saved DeepSeek key.)
    #[test]
    #[ignore]
    fn deepseek_searches_the_web() {
        let (key, _) = api_key(Provider::DeepSeek).expect("no DeepSeek key saved");
        let q = [("user".to_string(), "Search the web: what is the latest stable version of the Rust programming language?".to_string())];
        let seen = std::cell::RefCell::new(Vec::new());
        let reply = ask_deepseek(&q, &key, "deepseek-flash", None, true, &|w| seen.borrow_mut().push(w));
        let seen = seen.into_inner();
        match reply {
            Reply::Text(t) => {
                eprintln!("looked up: {seen:?}
answer: {t}");
                assert!(!seen.is_empty(), "it didn't search");
            }
            Reply::Error(e) | Reply::Searching(e) => panic!("{e}"),
        }
    }

    /// Claude Code on the plan looks something up and answers. (Uses the Claude plan.)
    #[test]
    #[ignore]
    fn claude_code_searches_the_web() {
        let (answer, seen) = ask_cli_web("Search the web: what is the latest stable version of the Rust programming language?");
        eprintln!("looked up: {seen:?}\nanswer: {answer:?}");
        assert!(!seen.is_empty(), "it didn't search");
        assert!(answer.unwrap().contains("1."));
    }

    #[test]
    fn web_helpers() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_743), (2026, 10, 17));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert!(system(true).contains("search the web") && !system(false).contains("search the web"));
        // Claude Code's tool calls become bubble notes.
        let search = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"WebSearch","input":{"query":"rust release"}}]}}"#;
        assert_eq!(cli_tool_use(search).as_deref(), Some("Searching for \"rust release\"..."));
        let fetch = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"WebFetch","input":{"url":"https://www.rust-lang.org/learn"}}]}}"#;
        assert_eq!(cli_tool_use(fetch).as_deref(), Some("Reading rust-lang.org..."));
        assert_eq!(cli_tool_use(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#), None);
        // After a search, only the answer (in cited pieces) is kept, not "let me look".
        let searched = r#"{"stop_reason":"end_turn","content":[
            {"type":"text","text":"Let me look that up."},
            {"type":"server_tool_use","id":"s1","name":"web_search","input":{"query":"x"}},
            {"type":"web_search_tool_result","tool_use_id":"s1","content":[]},
            {"type":"text","text":"It's "},
            {"type":"text","text":"sunny in Leeds","citations":[{}]},
            {"type":"text","text":" today."}]}"#;
        assert!(matches!(parse_reply(searched), Reply::Text(t) if t == "It's sunny in Leeds today."));
        // Search results are trimmed for the model.
        let results = r#"{"results":[{"title":"A","url":"https://a.example","snippet":"short","summary":"","date":null,"rerank_score":2}]}"#;
        let r: Value = serde_json::from_str(&search_results(results)).unwrap();
        assert_eq!(r[0]["summary"], "short");
        assert_eq!(r[0]["url"], "https://a.example");
        assert_eq!(search_results(r#"{"results":[]}"#), "No results.");
        assert!(deepseek_fetch("", "file:///c:/secret").starts_with("Fetch failed"));
    }

    /// Two questions through one Claude Code session (uses the signed-in Claude plan).
    #[test]
    #[ignore]
    fn claude_code_session_remembers() {
        let exe = claude_exe().expect("Claude Code not installed");
        let mut s = Session::start(&exe, "haiku", false).unwrap();
        // It dies with us: it's inside the kill-on-close job.
        let mut in_job = 0;
        unsafe {
            use std::os::windows::io::AsRawHandle;
            windows_sys::Win32::System::JobObjects::IsProcessInJob(s.child.as_raw_handle() as _, std::ptr::null_mut(), &mut in_job);
        }
        assert_ne!(in_job, 0, "Claude Code isn't in the job");
        let t0 = std::time::Instant::now();
        let a = s.ask("My favourite colour is teal. Just say ok.", None, &|_| {}).unwrap();
        let t1 = std::time::Instant::now();
        let b = s.ask("What is my favourite colour? One word.", None, &|_| {}).unwrap();
        eprintln!("first {:?}: {a}\nsecond {:?}: {b}", t1 - t0, t1.elapsed());
        assert!(b.to_lowercase().contains("teal"), "{b}");
    }

    #[test]
    fn long_answers_page_and_finish() {
        let mut s = Speech::new();
        s.say(&"word ".repeat(80));
        let (lines, _) = s.view().unwrap();
        assert!(lines.len() <= BUBBLE_ROWS && lines.iter().all(|l| l.len() <= BUBBLE_COLS));
        for _ in 0..4000 {
            s.update(0.02);
        }
        assert!(!s.busy(), "never finished talking");
    }
}
