use crate::platform;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(windows)]
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::process::{Child, Command};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

const IPC_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const MONITOR_READY_TIMEOUT: Duration = Duration::from_secs(10);
const MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(50);
// Playlist inspect JSON for a ±5 window stays well under this; the old 64 KiB
// cap broke multi-hundred episode seasons when the full list was loaded.
const MAX_IPC_LINE_BYTES: usize = 256 * 1024;
static NEXT_ENDPOINT_ID: AtomicU64 = AtomicU64::new(1);

/// Cancellation shared by every task owned by a playback session.
#[derive(Clone, Debug)]
pub(crate) struct TaskCancellation {
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl TaskCancellation {
    pub(crate) fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.notify.notified().await;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IpcEndpoint {
    path: PathBuf,
}

impl IpcEndpoint {
    pub fn for_session(session_id: u64) -> Self {
        let unique = NEXT_ENDPOINT_ID.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();

        #[cfg(unix)]
        let path =
            std::env::temp_dir().join(format!("anihub-mpv-{pid}-{session_id}-{unique}.sock"));
        #[cfg(windows)]
        let path = PathBuf::from(format!(r"\\.\pipe\anihub-mpv-{pid}-{session_id}-{unique}"));
        #[cfg(not(any(unix, windows)))]
        let path = std::env::temp_dir().join(format!("anihub-mpv-{pid}-{session_id}-{unique}.ipc"));

        Self { path }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn display(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    pub fn cleanup(&self) {
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug, Clone)]
pub struct MpvCommandResponse {
    pub data: Option<Value>,
}

#[cfg(unix)]
type IpcStream = UnixStream;
#[cfg(windows)]
type IpcStream = NamedPipeClient;

#[derive(Clone)]
pub struct MpvIpc {
    endpoint: IpcEndpoint,
    request_ids: Arc<AtomicU64>,
}

impl MpvIpc {
    fn with_request_ids(endpoint: IpcEndpoint, request_ids: Arc<AtomicU64>) -> Self {
        Self {
            endpoint,
            request_ids,
        }
    }

    pub async fn send_command(&self, command: Value) -> Result<MpvCommandResponse> {
        self.send_command_with_timeout(command, IPC_COMMAND_TIMEOUT)
            .await
    }

    pub async fn send_command_with_timeout(
        &self,
        command: Value,
        command_timeout: Duration,
    ) -> Result<MpvCommandResponse> {
        timeout(command_timeout, self.send_command_inner(command))
            .await
            .map_err(|_| anyhow!("mpv IPC command timed out after {command_timeout:?}"))?
    }

    async fn send_command_inner(&self, command: Value) -> Result<MpvCommandResponse> {
        let request_id = self.request_ids.fetch_add(1, Ordering::Relaxed);
        let mut stream = connect_with_retry(&self.endpoint, IPC_COMMAND_TIMEOUT).await?;
        let request = serde_json::to_vec(&json!({
            "command": command,
            "request_id": request_id,
        }))?;
        stream.write_all(&request).await?;
        stream.write_all(b"\n").await?;
        stream.flush().await?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = reader.read_line(&mut line).await?;
            if bytes == 0 {
                bail!("mpv IPC closed before response to request {request_id}");
            }
            if line.len() > MAX_IPC_LINE_BYTES {
                bail!("mpv IPC response exceeds {MAX_IPC_LINE_BYTES} bytes");
            }

            let value: Value = serde_json::from_str(line.trim_end())
                .with_context(|| format!("invalid mpv IPC response for request {request_id}"))?;
            if value.get("request_id").and_then(Value::as_u64) != Some(request_id) {
                // Notifications and responses for another request can share a
                // connection. They are not the response we are waiting for.
                continue;
            }

            let error = value
                .get("error")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("mpv response {request_id} has no error field"))?;
            if error != "success" {
                bail!("mpv command {request_id} failed: {error}");
            }

            return Ok(MpvCommandResponse {
                data: value.get("data").cloned(),
            });
        }
    }

    async fn send_observe_commands(
        &self,
        reader: &mut BufReader<IpcStream>,
        properties: &[&str],
        cancel: &TaskCancellation,
    ) -> Result<()> {
        let mut pending = HashSet::new();
        for (observer_id, property) in properties.iter().enumerate() {
            let request_id = self.request_ids.fetch_add(1, Ordering::Relaxed);
            pending.insert(request_id);
            let request = serde_json::to_vec(&json!({
                "command": ["observe_property", observer_id + 1, property],
                "request_id": request_id,
            }))?;
            reader.get_mut().write_all(&request).await?;
            reader.get_mut().write_all(b"\n").await?;
        }
        reader.get_mut().flush().await?;

        let mut line = String::new();
        while !pending.is_empty() {
            line.clear();
            let result = tokio::select! {
                _ = cancel.cancelled() => return Err(anyhow!("mpv monitor cancelled")),
                result = timeout(Duration::from_secs(1), reader.read_line(&mut line)) => result,
            };
            let bytes = match result {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(error)) => return Err(error.into()),
                Err(_) => continue,
            };
            if bytes == 0 {
                bail!("mpv IPC closed while observing properties");
            }
            if line.len() > MAX_IPC_LINE_BYTES {
                bail!("mpv monitor response exceeds {MAX_IPC_LINE_BYTES} bytes");
            }
            let value: Value = serde_json::from_str(line.trim_end())?;
            if let Some(request_id) = value.get("request_id").and_then(Value::as_u64) {
                if !pending.remove(&request_id) {
                    continue;
                }
                let error = value
                    .get("error")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("mpv observe response has no error field"))?;
                if error != "success" {
                    bail!("mpv observe_property failed: {error}");
                }
            }
        }
        Ok(())
    }
}

async fn connect_once(endpoint: &IpcEndpoint) -> Result<IpcStream> {
    #[cfg(unix)]
    {
        return Ok(UnixStream::connect(endpoint.path()).await?);
    }
    #[cfg(windows)]
    {
        return Ok(ClientOptions::new().open(endpoint.path())?);
    }
    #[allow(unreachable_code)]
    Err(anyhow!("mpv IPC is not supported on this platform"))
}

async fn connect_with_retry(endpoint: &IpcEndpoint, max_wait: Duration) -> Result<IpcStream> {
    let started = Instant::now();
    loop {
        match connect_once(endpoint).await {
            Ok(stream) => return Ok(stream),
            Err(error) if started.elapsed() >= max_wait => {
                return Err(error.context("mpv IPC endpoint did not become ready"));
            }
            Err(_) => sleep(MONITOR_POLL_INTERVAL).await,
        }
    }
}

async fn connect_with_retry_cancelled(
    endpoint: &IpcEndpoint,
    max_wait: Duration,
    cancel: &TaskCancellation,
) -> Result<IpcStream> {
    let started = Instant::now();
    loop {
        if cancel.is_cancelled() {
            bail!("mpv monitor cancelled while waiting for IPC");
        }
        match connect_once(endpoint).await {
            Ok(stream) => return Ok(stream),
            Err(error) if started.elapsed() >= max_wait => {
                return Err(error.context("mpv IPC endpoint did not become ready"));
            }
            Err(_) => {
                tokio::select! {
                    _ = cancel.cancelled() => bail!("mpv monitor cancelled while waiting for IPC"),
                    _ = sleep(MONITOR_POLL_INTERVAL) => {},
                }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndFileReason {
    Eof,
    Stop,
    Quit,
    Error,
    Abort,
    Redirect,
    Unknown(String),
}

impl EndFileReason {
    fn parse(value: Option<&str>) -> Self {
        match value.unwrap_or_default().to_ascii_lowercase().as_str() {
            "eof" => Self::Eof,
            "stop" => Self::Stop,
            "quit" => Self::Quit,
            "error" => Self::Error,
            "abort" => Self::Abort,
            "redirect" => Self::Redirect,
            "" => Self::Unknown("missing".to_string()),
            other => Self::Unknown(other.to_string()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndFileEvent {
    pub reason: EndFileReason,
    pub playlist_entry_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MpvMonitorEvent {
    Progress {
        time_pos: Option<f64>,
        duration: Option<f64>,
    },
    PauseChanged {
        paused: bool,
        time_pos: Option<f64>,
    },
    PlaylistPosition {
        position: Option<usize>,
        entry_id: Option<i64>,
    },
    FileStarted {
        playlist_entry_id: Option<i64>,
    },
    FileLoaded {
        playlist_entry_id: Option<i64>,
    },
    EofReached(bool),
    EndFile(EndFileEvent),
    MonitorFailed(String),
    Closed,
}

#[derive(Default)]
struct MonitorState {
    time_pos: Option<f64>,
    duration: Option<f64>,
    playlist_entry_id: Option<i64>,
}

fn value_as_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|v| i64::try_from(v).ok()))
    })
}

fn event_entry_id(value: &Value) -> Option<i64> {
    value_as_i64(
        value
            .get("playlist_entry_id")
            .or_else(|| value.get("playlist-entry-id")),
    )
}

fn parse_playlist_entry_ids(data: Option<&Value>) -> HashMap<i64, usize> {
    data.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, entry)| {
            value_as_i64(entry.get("id")).map(|entry_id| (entry_id, index))
        })
        .collect()
}

/// Parse one mpv JSON notification. Kept independent of sockets so fake IPC
/// tests can exercise reason/entry-id handling without launching mpv.
pub(crate) fn parse_monitor_line(
    line: &str,
    state: &mut impl MonitorStateAccess,
) -> Option<MpvMonitorEvent> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("event").and_then(Value::as_str) == Some("property-change") {
        let name = value.get("name").and_then(Value::as_str)?;
        match name {
            "time-pos" => {
                state.set_time(value.get("data").and_then(Value::as_f64));
                Some(MpvMonitorEvent::Progress {
                    time_pos: state.time(),
                    duration: state.duration(),
                })
            }
            "duration" => {
                state.set_duration(value.get("data").and_then(Value::as_f64));
                Some(MpvMonitorEvent::Progress {
                    time_pos: state.time(),
                    duration: state.duration(),
                })
            }
            "pause" => {
                let paused = value.get("data").and_then(Value::as_bool)?;
                Some(MpvMonitorEvent::PauseChanged {
                    paused,
                    time_pos: state.time(),
                })
            }
            "playlist-playing-pos" => {
                let position = value_as_i64(value.get("data"))
                    .and_then(|position| usize::try_from(position).ok());
                Some(MpvMonitorEvent::PlaylistPosition {
                    position,
                    entry_id: state.entry_id(),
                })
            }
            "eof-reached" => value
                .get("data")
                .and_then(Value::as_bool)
                .map(MpvMonitorEvent::EofReached),
            _ => None,
        }
    } else {
        match value.get("event").and_then(Value::as_str)? {
            "start-file" => {
                let playlist_entry_id = event_entry_id(&value);
                // Observed property values belong to the previous file until
                // mpv publishes fresh values for this playlist entry.
                state.set_time(None);
                state.set_duration(None);
                state.set_entry_id(playlist_entry_id);
                Some(MpvMonitorEvent::FileStarted { playlist_entry_id })
            }
            "file-loaded" => Some(MpvMonitorEvent::FileLoaded {
                // mpv's file-loaded event normally omits the id; start-file
                // already stored the matching playlist entry in monitor state.
                playlist_entry_id: event_entry_id(&value).or_else(|| state.entry_id()),
            }),
            "end-file" => Some(MpvMonitorEvent::EndFile(EndFileEvent {
                reason: EndFileReason::parse(value.get("reason").and_then(Value::as_str)),
                playlist_entry_id: event_entry_id(&value),
            })),
            _ => None,
        }
    }
}

pub(crate) trait MonitorStateAccess {
    fn time(&self) -> Option<f64>;
    fn duration(&self) -> Option<f64>;
    fn entry_id(&self) -> Option<i64>;
    fn set_time(&mut self, value: Option<f64>);
    fn set_duration(&mut self, value: Option<f64>);
    fn set_entry_id(&mut self, value: Option<i64>);
}

impl MonitorStateAccess for MonitorState {
    fn time(&self) -> Option<f64> {
        self.time_pos
    }
    fn duration(&self) -> Option<f64> {
        self.duration
    }
    fn entry_id(&self) -> Option<i64> {
        self.playlist_entry_id
    }
    fn set_time(&mut self, value: Option<f64>) {
        self.time_pos = value;
    }
    fn set_duration(&mut self, value: Option<f64>) {
        self.duration = value;
    }
    fn set_entry_id(&mut self, value: Option<i64>) {
        self.playlist_entry_id = value;
    }
}

async fn monitor_ipc(
    endpoint: IpcEndpoint,
    ipc: MpvIpc,
    cancel: TaskCancellation,
    tx: UnboundedSender<MpvMonitorEvent>,
) {
    let stream = match connect_with_retry_cancelled(&endpoint, MONITOR_READY_TIMEOUT, &cancel).await
    {
        Ok(stream) => stream,
        Err(error) => {
            let _ = tx.send(MpvMonitorEvent::MonitorFailed(error.to_string()));
            return;
        }
    };
    let mut reader = BufReader::new(stream);
    if let Err(error) = ipc
        .send_observe_commands(
            &mut reader,
            &[
                "time-pos",
                "duration",
                "pause",
                "playlist-playing-pos",
                "eof-reached",
            ],
            &cancel,
        )
        .await
    {
        if !cancel.is_cancelled() {
            let _ = tx.send(MpvMonitorEvent::MonitorFailed(error.to_string()));
        }
        return;
    }

    let mut state = MonitorState::default();
    let mut line = String::new();
    loop {
        line.clear();
        let result = tokio::select! {
            _ = cancel.cancelled() => break,
            result = timeout(Duration::from_secs(1), reader.read_line(&mut line)) => result,
        };
        let bytes = match result {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(error)) => {
                if !cancel.is_cancelled() {
                    let _ = tx.send(MpvMonitorEvent::MonitorFailed(error.to_string()));
                }
                break;
            }
            Err(_) => continue,
        };
        if bytes == 0 {
            if !cancel.is_cancelled() {
                let _ = tx.send(MpvMonitorEvent::Closed);
            }
            break;
        }
        if line.len() > MAX_IPC_LINE_BYTES {
            let _ = tx.send(MpvMonitorEvent::MonitorFailed(format!(
                "mpv monitor line exceeds {MAX_IPC_LINE_BYTES} bytes"
            )));
            break;
        }
        if let Some(event) = parse_monitor_line(&line, &mut state) {
            let _ = tx.send(event);
        }
    }
}

#[derive(Debug, Default)]
pub struct MpvShutdownSnapshot {
    pub time_pos: f64,
    pub duration: f64,
}

pub struct MpvSession {
    pub(crate) endpoint: IpcEndpoint,
    pub(crate) ipc: MpvIpc,
    pub(crate) child: Option<Child>,
    pub(crate) monitor_rx: Option<UnboundedReceiver<MpvMonitorEvent>>,
    pub(crate) monitor: Option<JoinHandle<()>>,
    pub(crate) monitor_cancel: TaskCancellation,
    playlist_entry_ids: HashMap<i64, usize>,
}

#[derive(Clone, Debug)]
struct MpvLaunchSettings {
    path: String,
    extra_args: Vec<String>,
}

impl Default for MpvLaunchSettings {
    fn default() -> Self {
        Self {
            path: "mpv".to_string(),
            extra_args: Vec::new(),
        }
    }
}

static MPV_LAUNCH_SETTINGS: OnceLock<RwLock<MpvLaunchSettings>> = OnceLock::new();

pub fn configure_mpv(path: &str, extra_args: &str) -> Result<()> {
    let parsed = shell_words::split(extra_args).context("Invalid additional mpv arguments")?;
    let settings = MpvLaunchSettings {
        path: if path.trim().is_empty() {
            "mpv".to_string()
        } else {
            path.trim().to_string()
        },
        extra_args: parsed,
    };
    *MPV_LAUNCH_SETTINGS
        .get_or_init(|| RwLock::new(MpvLaunchSettings::default()))
        .write()
        .map_err(|_| anyhow!("mpv settings lock is poisoned"))? = settings;
    Ok(())
}

fn mpv_launch_settings() -> MpvLaunchSettings {
    MPV_LAUNCH_SETTINGS
        .get_or_init(|| RwLock::new(MpvLaunchSettings::default()))
        .read()
        .map_or_else(
            |_| MpvLaunchSettings::default(),
            |settings| settings.clone(),
        )
}

#[derive(Clone, Debug, PartialEq)]
pub struct MpvPlaylistEntry {
    pub media_url: String,
    pub title: String,
    pub start_time: Option<f64>,
    pub referrer: String,
}

pub fn build_mpv_args(
    endpoint: &IpcEndpoint,
    entries: &[MpvPlaylistEntry],
    current_index: usize,
    extra_args: &[String],
) -> Vec<String> {
    let mut args = vec![
        format!("--input-ipc-server={}", endpoint.display()),
        "--idle=yes".to_string(),
        // Keep every entry open at EOF. The supervisor performs native
        // `playlist-next` only when autoplay is enabled; manual mpv playlist
        // controls remain available either way.
        "--keep-open=always".to_string(),
        format!("--playlist-start={current_index}"),
        "--force-window=yes".to_string(),
        "--no-terminal".to_string(),
        "--vo=gpu-next".to_string(),
    ];
    args.extend(extra_args.iter().cloned());
    for entry in entries {
        args.push("--{".to_string());
        args.push(format!("--force-media-title={}", entry.title));
        args.push(format!("--referrer={}", entry.referrer));
        if let Some(start_time) = entry
            .start_time
            .filter(|time| time.is_finite() && *time > 0.0)
        {
            args.push(format!("--start={start_time}"));
        }
        args.push(entry.media_url.clone());
        args.push("--}".to_string());
    }
    args
}

impl MpvSession {
    pub async fn spawn(
        id: u64,
        entries: &[MpvPlaylistEntry],
        current_index: usize,
    ) -> Result<Self> {
        if entries.is_empty() {
            bail!("Cannot start mpv with an empty playlist");
        }
        if current_index >= entries.len() {
            bail!("Selected mpv playlist index is out of bounds");
        }
        let endpoint = IpcEndpoint::for_session(id);
        endpoint.cleanup();
        let launch = mpv_launch_settings();
        let args = build_mpv_args(&endpoint, entries, current_index, &launch.extra_args);
        let child = Command::new(&launch.path)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("Failed to start mpv at `{}`", launch.path))?;

        let request_ids = Arc::new(AtomicU64::new(1));
        let ipc = MpvIpc::with_request_ids(endpoint.clone(), request_ids);
        let cancel = TaskCancellation::new();
        let (tx, monitor_rx) = mpsc::unbounded_channel();
        let monitor = tokio::spawn(monitor_ipc(
            endpoint.clone(),
            ipc.clone(),
            cancel.clone(),
            tx,
        ));

        let mut session = Self {
            endpoint,
            ipc,
            child: Some(child),
            monitor_rx: Some(monitor_rx),
            monitor: Some(monitor),
            monitor_cancel: cancel,
            playlist_entry_ids: HashMap::new(),
        };
        if let Err(error) = session.refresh_playlist_entry_ids(entries.len()).await {
            let _ = session.shutdown().await;
            return Err(error.context("Failed to inspect native mpv playlist"));
        }
        Ok(session)
    }

    async fn refresh_playlist_entry_ids(&mut self, expected_count: usize) -> Result<()> {
        let response = self
            .ipc
            .send_command(json!(["get_property", "playlist"]))
            .await?;
        let entry_ids = parse_playlist_entry_ids(response.data.as_ref());
        if entry_ids.len() != expected_count {
            bail!(
                "mpv playlist contains {} mapped entries, expected {expected_count}",
                entry_ids.len()
            );
        }
        self.playlist_entry_ids = entry_ids;
        Ok(())
    }

    pub fn playlist_index(&self, entry_id: i64) -> Option<usize> {
        self.playlist_entry_ids.get(&entry_id).copied()
    }

    pub fn try_recv_event(&mut self) -> Result<Option<MpvMonitorEvent>> {
        let Some(monitor_rx) = self.monitor_rx.as_mut() else {
            return Ok(Some(MpvMonitorEvent::Closed));
        };
        match monitor_rx.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => Ok(Some(MpvMonitorEvent::Closed)),
        }
    }

    pub fn child_exited(&mut self) -> Result<Option<std::process::ExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(Some(std::process::ExitStatus::default()));
        };
        Ok(child.try_wait()?)
    }

    pub async fn playlist_next(&self) -> Result<()> {
        self.ipc
            .send_command(json!(["playlist-next", "weak"]))
            .await?;
        Ok(())
    }

    pub async fn set_paused(&self, paused: bool) -> Result<()> {
        self.ipc
            .send_command(json!(["set_property", "pause", paused]))
            .await?;
        Ok(())
    }

    pub async fn final_position(&self) -> MpvShutdownSnapshot {
        let time_pos = self
            .ipc
            .send_command_with_timeout(
                json!(["get_property", "time-pos"]),
                Duration::from_millis(500),
            )
            .await
            .ok()
            .and_then(|response| response.data)
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0);
        let duration = self
            .ipc
            .send_command_with_timeout(
                json!(["get_property", "duration"]),
                Duration::from_millis(500),
            )
            .await
            .ok()
            .and_then(|response| response.data)
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0);
        MpvShutdownSnapshot { time_pos, duration }
    }

    pub async fn shutdown(&mut self) -> MpvShutdownSnapshot {
        self.monitor_cancel.cancel();
        let mut final_position = self.final_position().await;

        let _ = self
            .ipc
            .send_command_with_timeout(json!(["quit"]), Duration::from_millis(500))
            .await;

        if let Some(mut child) = self.child.take() {
            let mut exited = false;
            if let Ok(Ok(_)) = timeout(Duration::from_secs(2), child.wait()).await {
                exited = true;
            }
            if !exited {
                if let Some(pid) = child.id() {
                    platform::kill_process_tree(pid);
                }
                let _ = child.kill().await;
                let _ = timeout(Duration::from_secs(2), child.wait()).await;
            }
        }

        if let Some(monitor) = self.monitor.take() {
            monitor.abort();
            let _ = monitor.await;
        }
        self.endpoint.cleanup();

        // Query errors are expected when mpv has already crashed. The values
        // obtained before that point remain the best final snapshot.
        if !final_position.time_pos.is_finite() {
            final_position.time_pos = 0.0;
        }
        if !final_position.duration.is_finite() {
            final_position.duration = 0.0;
        }
        final_position
    }
}

impl Drop for MpvSession {
    fn drop(&mut self) {
        self.monitor_cancel.cancel();
        if let Some(monitor) = self.monitor.take() {
            monitor.abort();
        }
        if let Some(mut child) = self.child.take() {
            if let Some(pid) = child.id() {
                platform::kill_process_tree(pid);
            }
            let _ = child.start_kill();
        }
        self.endpoint.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_is_unique_per_session() {
        let first = IpcEndpoint::for_session(7);
        let second = IpcEndpoint::for_session(7);
        assert_ne!(first, second);
        assert!(first.display().contains("anihub-mpv-"));
    }

    #[test]
    fn mpv_args_build_a_native_playlist_with_selected_only_resume() {
        let endpoint = IpcEndpoint::for_session(1);
        let entries = vec![
            MpvPlaylistEntry {
                media_url: "https://media.test/1.m3u8".to_string(),
                title: "Anime - Ep 1".to_string(),
                start_time: None,
                referrer: "https://ref.test/".to_string(),
            },
            MpvPlaylistEntry {
                media_url: "https://media.test/2.m3u8".to_string(),
                title: "Anime - Ep 2".to_string(),
                start_time: Some(42.0),
                referrer: "https://ref.test/".to_string(),
            },
            MpvPlaylistEntry {
                media_url: "https://media.test/3.m3u8".to_string(),
                title: "Anime - Ep 3".to_string(),
                start_time: None,
                referrer: "https://ref.test/".to_string(),
            },
        ];
        let args = build_mpv_args(&endpoint, &entries, 1, &["--hwdec=auto".to_string()]);
        assert!(args.iter().any(|arg| arg == "--idle=yes"));
        assert!(args.iter().any(|arg| arg == "--keep-open=always"));
        assert!(args.iter().any(|arg| arg == "--playlist-start=1"));
        assert!(args.iter().any(|arg| arg == "--hwdec=auto"));
        assert_eq!(args.iter().filter(|arg| *arg == "--{").count(), 3);
        assert_eq!(args.iter().filter(|arg| *arg == "--}").count(), 3);
        assert_eq!(args.iter().filter(|arg| *arg == "--start=42").count(), 1);
        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("https://media.test/"))
                .cloned()
                .collect::<Vec<_>>(),
            entries
                .iter()
                .map(|entry| entry.media_url.clone())
                .collect::<Vec<_>>()
        );
        assert!(!args.iter().any(|arg| arg.contains("anihub-next")));
        assert!(!args.iter().any(|arg| arg.contains("osc-custom_button")));
    }

    #[test]
    fn monitor_parses_typed_end_file_and_entry_id() {
        let mut state = MonitorState::default();
        let event = parse_monitor_line(
            r#"{"event":"end-file","reason":"eof","playlist_entry_id":17}"#,
            &mut state,
        );
        assert_eq!(
            event,
            Some(MpvMonitorEvent::EndFile(EndFileEvent {
                reason: EndFileReason::Eof,
                playlist_entry_id: Some(17),
            }))
        );
    }

    #[test]
    fn monitor_reports_pause_with_the_latest_position() {
        let mut state = MonitorState::default();
        let _ = parse_monitor_line(
            r#"{"event":"property-change","name":"time-pos","data":754.0}"#,
            &mut state,
        );
        let event = parse_monitor_line(
            r#"{"event":"property-change","name":"pause","data":true}"#,
            &mut state,
        );
        assert_eq!(
            event,
            Some(MpvMonitorEvent::PauseChanged {
                paused: true,
                time_pos: Some(754.0),
            })
        );
    }

    #[test]
    fn start_file_clears_properties_from_the_previous_entry() {
        let mut state = MonitorState::default();
        let _ = parse_monitor_line(
            r#"{"event":"property-change","name":"time-pos","data":754.0}"#,
            &mut state,
        );
        let _ = parse_monitor_line(
            r#"{"event":"property-change","name":"duration","data":1200.0}"#,
            &mut state,
        );
        assert_eq!(
            parse_monitor_line(
                r#"{"event":"start-file","playlist_entry_id":22}"#,
                &mut state,
            ),
            Some(MpvMonitorEvent::FileStarted {
                playlist_entry_id: Some(22),
            })
        );
        assert_eq!(state.time_pos, None);
        assert_eq!(state.duration, None);
        assert_eq!(state.playlist_entry_id, Some(22));
        assert_eq!(
            parse_monitor_line(r#"{"event":"file-loaded"}"#, &mut state),
            Some(MpvMonitorEvent::FileLoaded {
                playlist_entry_id: Some(22),
            })
        );
    }

    #[test]
    fn monitor_parses_native_playlist_and_eof_properties() {
        let mut state = MonitorState::default();
        assert_eq!(
            parse_monitor_line(
                r#"{"event":"property-change","name":"playlist-playing-pos","data":5}"#,
                &mut state,
            ),
            Some(MpvMonitorEvent::PlaylistPosition {
                position: Some(5),
                entry_id: None,
            })
        );
        assert_eq!(
            parse_monitor_line(
                r#"{"event":"property-change","name":"eof-reached","data":true}"#,
                &mut state,
            ),
            Some(MpvMonitorEvent::EofReached(true))
        );
    }

    #[test]
    fn playlist_snapshot_maps_stable_entry_ids_to_indexes() {
        let data = json!([
            {"id": 17, "filename": "one"},
            {"id": 42, "filename": "two"}
        ]);
        assert_eq!(
            parse_playlist_entry_ids(Some(&data)),
            HashMap::from([(17, 0), (42, 1)])
        );
    }

    #[test]
    fn unknown_end_file_reason_is_preserved() {
        assert_eq!(
            EndFileReason::parse(Some("future-reason")),
            EndFileReason::Unknown("future-reason".to_string())
        );
    }
}
