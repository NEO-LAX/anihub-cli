#![allow(dead_code, clippy::wrong_self_convention)]

use crate::platform;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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
const MAX_IPC_LINE_BYTES: usize = 64 * 1024;
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
    pub request_id: u64,
    pub error: String,
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
    pub fn new(endpoint: IpcEndpoint) -> Self {
        Self {
            endpoint,
            request_ids: Arc::new(AtomicU64::new(1)),
        }
    }

    fn with_request_ids(endpoint: IpcEndpoint, request_ids: Arc<AtomicU64>) -> Self {
        Self {
            endpoint,
            request_ids,
        }
    }

    pub fn endpoint(&self) -> &IpcEndpoint {
        &self.endpoint
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
                request_id,
                error: error.to_string(),
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

    pub fn is_natural_eof(&self) -> bool {
        matches!(self, Self::Eof)
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
    EndFile(EndFileEvent),
    MonitorFailed(String),
    Closed,
}

#[derive(Default)]
struct MonitorState {
    time_pos: Option<f64>,
    duration: Option<f64>,
    playlist_pos: Option<usize>,
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
            "playlist-pos" => {
                let position = value_as_i64(value.get("data"))
                    .and_then(|position| usize::try_from(position).ok());
                state.set_position(position);
                Some(MpvMonitorEvent::PlaylistPosition {
                    position,
                    entry_id: state.entry_id(),
                })
            }
            "playlist-entry-id" => {
                let entry_id = value_as_i64(value.get("data"));
                state.set_entry_id(entry_id);
                Some(MpvMonitorEvent::PlaylistPosition {
                    position: state.position(),
                    entry_id,
                })
            }
            _ => None,
        }
    } else {
        match value.get("event").and_then(Value::as_str)? {
            "start-file" => Some(MpvMonitorEvent::FileStarted {
                playlist_entry_id: event_entry_id(&value),
            }),
            "file-loaded" => Some(MpvMonitorEvent::FileLoaded {
                playlist_entry_id: event_entry_id(&value),
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
    fn position(&self) -> Option<usize>;
    fn entry_id(&self) -> Option<i64>;
    fn set_time(&mut self, value: Option<f64>);
    fn set_duration(&mut self, value: Option<f64>);
    fn set_position(&mut self, value: Option<usize>);
    fn set_entry_id(&mut self, value: Option<i64>);
}

impl MonitorStateAccess for MonitorState {
    fn time(&self) -> Option<f64> {
        self.time_pos
    }
    fn duration(&self) -> Option<f64> {
        self.duration
    }
    fn position(&self) -> Option<usize> {
        self.playlist_pos
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
    fn set_position(&mut self, value: Option<usize>) {
        self.playlist_pos = value;
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
            &["time-pos", "duration", "playlist-pos", "playlist-entry-id"],
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
    pub(crate) id: u64,
    pub(crate) endpoint: IpcEndpoint,
    pub(crate) ipc: MpvIpc,
    pub(crate) child: Option<Child>,
    pub(crate) monitor_rx: Option<UnboundedReceiver<MpvMonitorEvent>>,
    pub(crate) monitor: Option<JoinHandle<()>>,
    pub(crate) monitor_cancel: TaskCancellation,
    released: bool,
}

pub fn build_mpv_args(
    endpoint: &IpcEndpoint,
    media_url: &str,
    start_time: Option<f64>,
    anime_title: &str,
    episode_title: &str,
    referrer: &str,
) -> Vec<String> {
    let mut args = vec![
        format!("--input-ipc-server={}", endpoint.display()),
        "--idle=yes".to_string(),
        "--keep-open=no".to_string(),
        format!("--force-media-title={} - {}", anime_title, episode_title),
        format!("--referrer={referrer}"),
        "--force-window=yes".to_string(),
        "--no-terminal".to_string(),
        "--vo=gpu-next".to_string(),
    ];
    if let Some(start_time) = start_time.filter(|time| time.is_finite() && *time > 0.0) {
        // This is the only resume input. No playlist/property notification is
        // allowed to manufacture a resume position.
        args.push(format!("--start={start_time}"));
    }
    args.push(media_url.to_string());
    args
}

impl MpvSession {
    pub async fn spawn(
        id: u64,
        media_url: &str,
        start_time: Option<f64>,
        anime_title: &str,
        episode_title: &str,
        referrer: &str,
    ) -> Result<Self> {
        let endpoint = IpcEndpoint::for_session(id);
        endpoint.cleanup();
        let args = build_mpv_args(
            &endpoint,
            media_url,
            start_time,
            anime_title,
            episode_title,
            referrer,
        );
        let child = Command::new("mpv")
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Failed to start mpv. Please make sure mpv is installed.")?;

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

        Ok(Self {
            id,
            endpoint,
            ipc,
            child: Some(child),
            monitor_rx: Some(monitor_rx),
            monitor: Some(monitor),
            monitor_cancel: cancel,
            released: false,
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn endpoint(&self) -> &IpcEndpoint {
        &self.endpoint
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

    pub async fn load_media(
        &self,
        media_url: &str,
        start_time: Option<f64>,
        anime_title: &str,
        episode_title: &str,
    ) -> Result<()> {
        let command =
            if let Some(start_time) = start_time.filter(|time| time.is_finite() && *time > 0.0) {
                json!(["loadfile", media_url, "replace", -1, {"start": start_time}])
            } else {
                json!(["loadfile", media_url, "replace"])
            };
        self.ipc.send_command(command).await?;
        self.ipc
            .send_command(json!([
                "set_property",
                "force-media-title",
                format!("{anime_title} - {episode_title}")
            ]))
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

    /// Compatibility conversion for the untouched UI loop. The authoritative
    /// supervisor uses typed events directly and never uses this bridge.
    pub(crate) fn into_legacy(&mut self) -> (Child, UnboundedReceiver<MpvEvent>, JoinHandle<()>) {
        self.released = true;
        let child = self.child.take().expect("mpv child already consumed");
        let endpoint = self.endpoint.clone();
        let monitor_rx = self
            .monitor_rx
            .take()
            .expect("mpv monitor already consumed");
        let monitor = self.monitor.take();
        let monitor_cancel = self.monitor_cancel.clone();
        let (tx, rx) = mpsc::unbounded_channel();
        let bridge = tokio::spawn(async move {
            let mut typed_rx = monitor_rx;
            while let Some(event) = typed_rx.recv().await {
                if let Some(event) = legacy_event(event) {
                    let _ = tx.send(event);
                }
            }
            monitor_cancel.cancel();
            if let Some(monitor) = monitor {
                monitor.abort();
                let _ = monitor.await;
            }
            endpoint.cleanup();
        });
        (child, rx, bridge)
    }
}

impl Drop for MpvSession {
    fn drop(&mut self) {
        if self.released {
            return;
        }
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

#[derive(Clone, Debug)]
pub enum MpvEvent {
    Progress(f64, f64),
    PlaylistPos(usize),
    FileStarted,
    FileLoaded,
    EndFile,
}

fn legacy_event(event: MpvMonitorEvent) -> Option<MpvEvent> {
    match event {
        MpvMonitorEvent::Progress { time_pos, duration } => Some(MpvEvent::Progress(
            time_pos.unwrap_or(0.0),
            duration.unwrap_or(0.0),
        )),
        // Position zero is an initialization notification, not a progress
        // transition. Suppressing it protects the compatibility UI from
        // persisting/resetting the new episode at time zero.
        MpvMonitorEvent::PlaylistPosition {
            position: Some(position),
            ..
        } if position > 0 => Some(MpvEvent::PlaylistPos(position)),
        MpvMonitorEvent::FileStarted { .. } => Some(MpvEvent::FileStarted),
        // The new supervisor applies PlayTarget.start_time in the load command;
        // do not expose file-loaded as a cue for a detached resume task.
        MpvMonitorEvent::FileLoaded { .. }
        | MpvMonitorEvent::PlaylistPosition { .. }
        | MpvMonitorEvent::MonitorFailed(_)
        | MpvMonitorEvent::Closed => None,
        MpvMonitorEvent::EndFile(_) => Some(MpvEvent::EndFile),
    }
}

pub struct MpvPlayer {
    shared: Arc<Mutex<LegacyEndpoint>>,
    bound_session_id: Option<u64>,
}

struct LegacyEndpoint {
    session_id: u64,
    endpoint: IpcEndpoint,
    request_ids: Arc<AtomicU64>,
}

impl Clone for MpvPlayer {
    fn clone(&self) -> Self {
        let bound_session_id = self
            .shared
            .lock()
            .ok()
            .map(|state| state.session_id)
            .or(self.bound_session_id);
        Self {
            shared: self.shared.clone(),
            bound_session_id,
        }
    }
}

impl MpvPlayer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            shared: Arc::new(Mutex::new(LegacyEndpoint {
                session_id: 0,
                endpoint: IpcEndpoint::for_session(0),
                request_ids: Arc::new(AtomicU64::new(1)),
            })),
            bound_session_id: None,
        })
    }

    pub async fn send_command(&self, command: Value) -> Result<()> {
        let (session_id, endpoint, request_ids) = {
            let state = self
                .shared
                .lock()
                .map_err(|_| anyhow!("mpv endpoint state poisoned"))?;
            if state.session_id == 0 {
                bail!("no active mpv session");
            }
            if let Some(bound) = self.bound_session_id {
                if bound != state.session_id {
                    bail!("stale IPC command for session {bound}");
                }
            }
            (
                state.session_id,
                state.endpoint.clone(),
                state.request_ids.clone(),
            )
        };
        let _ = session_id;
        MpvIpc::with_request_ids(endpoint, request_ids)
            .send_command(command)
            .await
            .map(|_| ())
    }

    pub async fn start(
        &self,
        media_url: &str,
        start_time: Option<f64>,
        anime_title: &str,
        episode_title: &str,
        referrer: &str,
    ) -> Result<(Child, UnboundedReceiver<MpvEvent>, JoinHandle<()>)> {
        let session_id = NEXT_ENDPOINT_ID.fetch_add(1, Ordering::Relaxed);
        let session = MpvSession::spawn(
            session_id,
            media_url,
            start_time,
            anime_title,
            episode_title,
            referrer,
        )
        .await?;
        let endpoint = session.endpoint.clone();
        let request_ids = session.ipc.request_ids.clone();
        let mut session = session;
        let result = session.into_legacy();
        if let Ok(mut state) = self.shared.lock() {
            state.session_id = session_id;
            state.endpoint = endpoint;
            state.request_ids = request_ids;
        }
        Ok(result)
    }

    pub fn cleanup(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.endpoint.cleanup();
            state.session_id = 0;
        }
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
    fn mpv_args_use_idle_and_non_persistent_keep_open() {
        let endpoint = IpcEndpoint::for_session(1);
        let args = build_mpv_args(
            &endpoint,
            "https://media.test/a.m3u8",
            Some(42.0),
            "Anime",
            "Ep 1",
            "https://ref.test/",
        );
        assert!(args.iter().any(|arg| arg == "--idle=yes"));
        assert!(args.iter().any(|arg| arg == "--keep-open=no"));
        assert!(args.iter().any(|arg| arg == "--start=42"));
    }

    #[test]
    fn initial_playlist_position_is_not_a_legacy_transition() {
        let event = legacy_event(MpvMonitorEvent::PlaylistPosition {
            position: Some(0),
            entry_id: Some(12),
        });
        assert!(event.is_none());
    }

    #[test]
    fn monitor_parses_typed_end_file_and_entry_id() {
        let mut state = MonitorState::default();
        let started = parse_monitor_line(r#"{"event":"playlist-pos","data":0}"#, &mut state);
        assert!(started.is_none());
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
    fn unknown_end_file_reason_is_preserved() {
        assert_eq!(
            EndFileReason::parse(Some("future-reason")),
            EndFileReason::Unknown("future-reason".to_string())
        );
    }
}
