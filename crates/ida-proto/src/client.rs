//! Spawns `ida-host.exe` and multiplexes requests over its stdio.

use crate::{HostEvent, Method, Request, Response};
use futures::channel::{mpsc, oneshot};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub enum ClientError {
    /// The host process is gone (crashed, exited or never started).
    HostGone(String),
    /// IDA reported a failure for this request.
    Ida(String),
    Protocol(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostGone(m) => write!(f, "IDA host is not running: {m}"),
            Self::Ida(m) => f.write_str(m),
            Self::Protocol(m) => write!(f, "protocol error: {m}"),
        }
    }
}

impl std::error::Error for ClientError {}

type Reply = Result<Box<serde_json::value::RawValue>, ClientError>;

struct Shared {
    pending: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    gone: Mutex<Option<String>>,
}

impl Shared {
    fn fail_all(&self, why: &str) {
        *self.gone.lock().unwrap() = Some(why.to_owned());
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(ClientError::HostGone(why.to_owned())));
        }
    }
}

/// Handle to a running host. Cheap to clone; all clones share the process.
#[derive(Clone)]
pub struct Client {
    shared: Arc<Shared>,
    outbox: std::sync::mpsc::Sender<String>,
    next_id: Arc<AtomicU64>,
    child: Arc<Mutex<Child>>,
}

/// Where to find IDA and the host binary.
#[derive(Clone, Debug)]
pub struct HostConfig {
    pub host_exe: PathBuf,
    pub ida_dir: PathBuf,
    /// Command-line arguments for the host, e.g. `--loader-binary`.
    pub args: Vec<String>,
    /// Extra environment variables, e.g. a private `IDAUSR`.
    pub env: Vec<(String, String)>,
}

impl HostConfig {
    /// `ida-host.exe` next to the current executable (or its parent, for
    /// cargo examples), IDA from `IDADIR` or the newest install found under
    /// Program Files (see [`find_ida_install`]).
    pub fn discover() -> Self {
        let exe = std::env::current_exe().unwrap_or_default();
        let mut host_exe = exe.with_file_name("ida-host.exe");
        // Examples run from target\<profile>\examples; the host sits one up.
        if !host_exe.exists()
            && let Some(up) = exe
                .parent()
                .and_then(Path::parent)
                .map(|p| p.join("ida-host.exe"))
                .filter(|p| p.exists())
        {
            host_exe = up;
        }
        let ida_dir = std::env::var_os("IDADIR")
            .map(PathBuf::from)
            .or_else(ida_dir_setting)
            .or_else(find_ida_install)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\IDA Professional"));
        Self {
            host_exe,
            ida_dir,
            args: Vec::new(),
            env: Vec::new(),
        }
    }
}

static IDA_DIR_SETTING: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

/// The IDA folder the user picked ("Locate IDA..."); `IDADIR` still wins.
pub fn set_ida_dir_setting(dir: Option<PathBuf>) {
    *IDA_DIR_SETTING.write().unwrap() = dir;
}

fn ida_dir_setting() -> Option<PathBuf> {
    IDA_DIR_SETTING
        .read()
        .unwrap()
        .clone()
        .filter(|d| is_ida_dir(d))
}

/// Whether `dir` is an IDA install idalib can run from.
pub fn is_ida_dir(dir: &Path) -> bool {
    dir.join("idalib.dll").is_file()
}

/// What stops a host from starting at all, found before trying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupProblem {
    /// No IDA install with `idalib.dll` was found.
    IdaMissing,
    /// `ida-host.exe` is not where it should be.
    HostMissing(PathBuf),
}

impl SetupProblem {
    pub fn message(&self) -> String {
        match self {
            Self::IdaMissing => "IDA Pro with idalib was not found. ida-zui needs your own licensed IDA install (a folder that contains idalib.dll)."
                .to_owned(),
            Self::HostMissing(p) => format!(
                "ida-host.exe is missing ({}). It must sit in the same folder as ida-zui.exe; extract the whole release zip or rebuild with `cargo build --workspace --bins`.",
                p.display()
            ),
        }
    }
}

impl HostConfig {
    /// The first thing that would keep [`Client::spawn`] from working.
    pub fn problem(&self) -> Option<SetupProblem> {
        if !self.host_exe.is_file() {
            Some(SetupProblem::HostMissing(self.host_exe.clone()))
        } else if !is_ida_dir(&self.ida_dir) {
            Some(SetupProblem::IdaMissing)
        } else {
            None
        }
    }
}

/// The newest IDA install that ships `idalib.dll`: a folder named
/// `IDA Professional *`, `IDA Pro *` or `IDA *` under `%ProgramFiles%`.
/// "Newest" compares the digit runs in the folder name numerically, so
/// `IDA Professional 10` beats `IDA Professional 9`.
pub fn find_ida_install() -> Option<PathBuf> {
    let root = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let mut found: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            (name.starts_with("IDA ") && path.join("idalib.dll").exists())
                .then(|| (version_key(&name), path))
        })
        .collect();
    found.sort();
    found.pop().map(|(_, path)| path)
}

fn version_key(name: &str) -> Vec<u64> {
    name.split(|c: char| !c.is_ascii_digit())
        .filter(|d| !d.is_empty())
        .filter_map(|d| d.parse().ok())
        .collect()
}

/// The private file extension that maps to `processor` in the IDAUSR config
/// written by [`HostConfig::raw_binary`]. A raw input must be opened under a
/// name ending in it (`blob.bin.izp_arm`) for IDA to pick that processor.
/// IDA ignores a map key longer than nine characters, so a long name keeps
/// three letters and ends in a two-digit checksum (`izp_tri26`).
pub fn processor_extension(processor: &str) -> String {
    let clean: String = processor
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_lowercase();
    if clean.len() <= 5 {
        return format!("izp_{clean}");
    }
    let sum = clean
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)))
        % 100;
    format!("izp_{}{sum:02}", &clean[..3])
}

impl HostConfig {
    /// Loads inputs with IDA's "Binary file" loader and makes the processor
    /// choosable per input: `dir` becomes a private IDAUSR whose `ida.cfg`
    /// maps `processor_extension(p)` to each `p` (one entry per line: with commas
    /// between entries only the first is found). The user's own IDAUSR (or
    /// IDA's default per-user directory) stays on the path so their plugins
    /// still load.
    pub fn raw_binary(mut self, dir: &Path, processors: &[&str]) -> std::io::Result<Self> {
        let entries: Vec<String> = processors
            .iter()
            .map(|p| format!("    \"{}\" : \"{p}\"", processor_extension(p)))
            .collect();
        let text = format!(
            "// Written by ida-zui: raw-binary processor choices.
DEFAULT_PROCESSOR = {{
{}
}}
",
            entries.join(
                "
"
            )
        );
        let file = dir.join("cfg").join("ida.cfg");
        if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
            std::fs::create_dir_all(dir.join("cfg"))?;
            std::fs::write(&file, text)?;
        }
        let user = std::env::var("IDAUSR")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| {
                std::env::var("APPDATA")
                    .map(|a| format!(r"{a}\Hex-Rays\IDA Pro"))
                    .unwrap_or_default()
            });
        let mut idausr = dir.display().to_string();
        if !user.is_empty() {
            idausr.push(';');
            idausr.push_str(&user);
        }
        self.args.push("--loader-binary".into());
        self.env.push(("IDAUSR".into(), idausr));
        Ok(self)
    }
}

impl HostConfig {
    /// Puts `dir` in front of IDAUSR, so its `plugins` folder is scanned too.
    /// The user's own IDAUSR (or IDA's default per-user directory) stays on
    /// the path.
    pub fn with_idausr(mut self, dir: &Path) -> Self {
        let user = std::env::var("IDAUSR")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| {
                std::env::var("APPDATA")
                    .map(|a| format!(r"{a}\Hex-Rays\IDA Pro"))
                    .unwrap_or_default()
            });
        let mut idausr = dir.display().to_string();
        if !user.is_empty() {
            idausr.push(';');
            idausr.push_str(&user);
        }
        self.env.retain(|(k, _)| k != "IDAUSR");
        self.env.push(("IDAUSR".into(), idausr));
        self
    }
}

impl Client {
    /// Starts the host. Log lines (IDA's console output) arrive on the receiver.
    pub fn spawn(config: &HostConfig) -> std::io::Result<(Self, mpsc::UnboundedReceiver<String>)> {
        let (client, logs, _events) = Self::spawn_with_events(config)?;
        Ok((client, logs))
    }

    /// Starts the host. Besides the log lines, the host's events (`log`,
    /// `db_changed`, `analysis`) arrive on the third receiver, in the order
    /// the host wrote them and before the reply of the request that was
    /// running when they were written.
    pub fn spawn_with_events(
        config: &HostConfig,
    ) -> std::io::Result<(
        Self,
        mpsc::UnboundedReceiver<String>,
        mpsc::UnboundedReceiver<HostEvent>,
    )> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![config.ida_dir.clone()];
        paths.extend(std::env::split_paths(&path));
        let path = std::env::join_paths(paths).map_err(std::io::Error::other)?;

        let mut cmd = Command::new(&config.host_exe);
        cmd.args(&config.args).envs(config.env.iter().cloned());
        cmd.env("PATH", path)
            .env("IDADIR", &config.ida_dir)
            .env("IDA_ZUI_PARENT", std::process::id().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("cannot start {}: {e}", config.host_exe.display()),
            )
        })?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            gone: Mutex::new(None),
        });

        let (outbox, outbox_rx) = std::sync::mpsc::channel::<String>();
        let writer_shared = shared.clone();
        std::thread::Builder::new()
            .name("ida-host-writer".into())
            .spawn(move || writer(stdin, outbox_rx, writer_shared))?;

        let (log_tx, log_rx) = mpsc::unbounded();
        let (event_tx, event_rx) = mpsc::unbounded();
        let reader_shared = shared.clone();
        let reader_logs = log_tx.clone();
        std::thread::Builder::new()
            .name("ida-host-reader".into())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                // Bytes, not `lines()`: one invalid UTF-8 byte must not end the session.
                for line in reader.split(b'\n') {
                    let Ok(line) = line else { break };
                    let line = String::from_utf8_lossy(&line);
                    // Events have no id; never let one resolve a request.
                    if line.starts_with(r#"{"event":"#) {
                        if let Ok(event) = serde_json::from_str::<HostEvent>(&line) {
                            if let HostEvent::Log { lines } = &event {
                                for l in lines {
                                    let _ = reader_logs.unbounded_send(l.clone());
                                }
                            }
                            let _ = event_tx.unbounded_send(event);
                        }
                        continue;
                    }
                    let Ok(resp) = serde_json::from_str::<Response>(&line) else {
                        continue;
                    };
                    let tx = reader_shared.pending.lock().unwrap().remove(&resp.id);
                    if let Some(tx) = tx {
                        let reply = match (resp.ok, resp.err) {
                            (_, Some(err)) => Err(ClientError::Ida(err)),
                            (Some(ok), None) => Ok(ok),
                            // `"ok": null` (unit replies) deserialises as None.
                            (None, None) => Ok(serde_json::value::RawValue::from_string(
                                "null".into(),
                            )
                            .unwrap()),
                        };
                        let _ = tx.send(reply);
                    }
                }
                reader_shared.fail_all("the host process exited");
            })?;

        std::thread::Builder::new()
            .name("ida-host-log".into())
            .spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.split(b'\n') {
                    let Ok(line) = line else { break };
                    let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                    if log_tx.unbounded_send(text).is_err() {
                        break;
                    }
                }
            })?;

        Ok((
            Self {
                shared,
                outbox,
                next_id: Arc::new(AtomicU64::new(1)),
                child: Arc::new(Mutex::new(child)),
            },
            log_rx,
            event_rx,
        ))
    }

    /// Sends a request; the future resolves when the host answers.
    pub fn call<M: Method>(
        &self,
        method: M,
    ) -> impl std::future::Future<Output = Result<M::Reply, ClientError>> + Send + 'static + use<M>
    {
        let (tx, rx) = oneshot::channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let gone = self.shared.gone.lock().unwrap().clone();
        let sent = match gone {
            Some(why) => Err(ClientError::HostGone(why)),
            None => serde_json::value::to_raw_value(&method)
                .map_err(|e| ClientError::Protocol(e.to_string()))
                .and_then(|params| {
                    let line = serde_json::to_string(&Request {
                        id,
                        method: M::NAME,
                        params,
                    })
                    .map_err(|e| ClientError::Protocol(e.to_string()))?;
                    self.shared.pending.lock().unwrap().insert(id, tx);
                    self.outbox
                        .send(line)
                        .map_err(|_| ClientError::HostGone("writer stopped".into()))
                }),
        };
        async move {
            sent?;
            let raw = rx
                .await
                .map_err(|_| ClientError::HostGone("request dropped".into()))??;
            serde_json::from_str::<M::Reply>(raw.get())
                .map_err(|e| ClientError::Protocol(format!("{}: {e}", M::NAME)))
        }
    }

    /// Blocking convenience for tools and tests.
    pub fn call_blocking<M: Method>(&self, method: M) -> Result<M::Reply, ClientError> {
        futures::executor::block_on(self.call(method))
    }

    pub fn is_alive(&self) -> bool {
        self.shared.gone.lock().unwrap().is_none()
    }

    /// Blocks until the host process exits; true if it did within `timeout`.
    pub fn wait_exit(&self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if !matches!(self.child.lock().unwrap().try_wait(), Ok(None)) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Kills the host without saving.
    pub fn kill(&self) {
        let _ = self.child.lock().unwrap().kill();
    }
}

fn writer(mut stdin: ChildStdin, rx: std::sync::mpsc::Receiver<String>, shared: Arc<Shared>) {
    for line in rx {
        if stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .is_err()
        {
            shared.fail_all("cannot write to the host");
            return;
        }
    }
}
