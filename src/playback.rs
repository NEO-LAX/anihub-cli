use crate::api;
use crate::api::EpisodeSourcesResponse;
use crate::player::{
    EndFileReason, MpvMonitorEvent, MpvPlaylistEntry, MpvSession, TaskCancellation,
};
use crate::storage;
use crate::ui::AppState;
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::{Mutex, Semaphore, mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Duration, sleep, timeout};

#[derive(Clone, Debug, PartialEq)]
pub struct PlayTarget {
    pub anime_id: u32,
    pub anime_title: String,
    pub player_title: String,
    pub season: u32,
    pub episode: u32,
    pub episode_title: String,
    pub stream_page_url: String,
    pub start_time: Option<f64>,
    pub studio_name: String,
    /// HTTP Referer for the Ashdi player page.
    pub referrer: String,
}

/// A complete, ordered playback timeline with one selected entry.
///
/// The full timeline is logical (progress, autoplay, franchise order). mpv
/// starts with a small window (~25 streams: 12 behind + current + 12 ahead),
/// then the actor grows its native playlist in place near either edge. Huge
/// seasons therefore avoid one enormous argv without sacrificing navigation.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackTimeline {
    entries: Vec<PlayTarget>,
    current_index: usize,
}

/// Episodes kept behind / ahead of the current title in the native mpv playlist.
/// Total window size is at most `behind + 1 + ahead` (25 with ±12).
const PLAYLIST_WINDOW_BEHIND: usize = 12;
const PLAYLIST_WINDOW_AHEAD: usize = 12;
/// Re-center the window once the playhead sits this close to either edge.
const PLAYLIST_REWINDOW_EDGE: usize = 3;
/// Avoid a tight resolution loop when an unavailable neighbor truncates a
/// resolved window right beside the current episode.
const PLAYLIST_REWINDOW_RETRY_DELAY: Duration = Duration::from_secs(30);
/// Direct Ashdi URLs can expire. Keep them only long enough to make adjacent
/// window shifts cheap, then scrape the source page again.
const STREAM_URL_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

/// Inclusive-exclusive timeline range loaded into mpv around `current`.
fn playlist_window(current: usize, len: usize) -> (usize, usize) {
    if len == 0 {
        return (0, 0);
    }
    let current = current.min(len - 1);
    let start = current.saturating_sub(PLAYLIST_WINDOW_BEHIND);
    let end = (current + PLAYLIST_WINDOW_AHEAD + 1).min(len);
    (start, end)
}

fn should_rewindow(
    window_start: usize,
    window_len: usize,
    current: usize,
    timeline_len: usize,
) -> bool {
    if window_len == 0 || timeline_len <= window_len {
        return false;
    }
    let local = current.saturating_sub(window_start);
    let near_start = local <= PLAYLIST_REWINDOW_EDGE && window_start > 0;
    let near_end = local + 1 + PLAYLIST_REWINDOW_EDGE >= window_len
        && window_start + window_len < timeline_len;
    near_start || near_end
}

fn timeline_in_window(window_start: usize, window_len: usize, timeline_index: usize) -> bool {
    timeline_index >= window_start && timeline_index < window_start + window_len
}

fn live_timeline_index(current_index: usize, pending_index: Option<usize>) -> usize {
    pending_index.unwrap_or(current_index)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlaylistExtensionPlan {
    /// Indexes inside the newly resolved window which belong before the
    /// currently loaded native playlist.
    before: Range<usize>,
    /// Indexes inside the newly resolved window which belong after it.
    after: Range<usize>,
}

fn playlist_extension_plan(
    loaded_start: usize,
    loaded_len: usize,
    resolved_start: usize,
    resolved_len: usize,
) -> Option<PlaylistExtensionPlan> {
    let loaded_end = loaded_start.checked_add(loaded_len)?;
    let resolved_end = resolved_start.checked_add(resolved_len)?;
    if loaded_len == 0
        || resolved_len == 0
        || loaded_start > resolved_end
        || resolved_start > loaded_end
    {
        return None;
    }
    let before_end = loaded_start.clamp(resolved_start, resolved_end) - resolved_start;
    let after_start = loaded_end.clamp(resolved_start, resolved_end) - resolved_start;
    Some(PlaylistExtensionPlan {
        before: 0..before_end,
        after: after_start..resolved_len,
    })
}

fn extended_playlist_range(
    loaded_start: usize,
    loaded_len: usize,
    prepended: usize,
    appended: usize,
) -> (usize, usize) {
    (
        loaded_start.saturating_sub(prepended),
        loaded_len + prepended + appended,
    )
}

fn contiguous_resolved_range<T>(entries: &[Option<T>], selected: usize) -> Option<(usize, usize)> {
    entries.get(selected)?.as_ref()?;
    let mut start = selected;
    while start > 0 && entries[start - 1].is_some() {
        start -= 1;
    }
    let mut end = selected + 1;
    while end < entries.len() && entries[end].is_some() {
        end += 1;
    }
    Some((start, end))
}

fn play_target_cache_key(target: &PlayTarget) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        target.anime_id, target.season, target.episode, target.studio_name, target.stream_page_url
    )
}

#[derive(Clone, Debug)]
struct CachedStreamUrl {
    url: String,
    inserted_at: Instant,
}

fn fresh_cached_url(
    cache: &mut HashMap<String, CachedStreamUrl>,
    key: &str,
    now: Instant,
) -> Option<String> {
    let entry = cache.get(key)?;
    if now.duration_since(entry.inserted_at) < STREAM_URL_CACHE_TTL {
        return Some(entry.url.clone());
    }
    cache.remove(key);
    None
}

impl PlaybackTimeline {
    pub fn single(target: PlayTarget) -> Self {
        Self {
            entries: vec![target],
            current_index: 0,
        }
    }

    fn from_entries(entries: Vec<PlayTarget>, selected: &PlayTarget) -> Self {
        let current_index = entries
            .iter()
            .position(|target| same_episode(target, selected))
            .unwrap_or(0);
        if entries.is_empty() {
            return Self::single(selected.clone());
        }
        Self {
            entries,
            current_index,
        }
    }

    pub fn current(&self) -> &PlayTarget {
        &self.entries[self.current_index]
    }

    pub fn clear_resume_positions(&mut self) {
        for target in &mut self.entries {
            target.start_time = None;
        }
    }

    fn has_next(&self) -> bool {
        self.current_index + 1 < self.entries.len()
    }

    fn select(&mut self, index: usize) -> Option<PlayTarget> {
        let target = self.entries.get(index)?.clone();
        self.current_index = index;
        Some(target)
    }
}

fn same_episode(left: &PlayTarget, right: &PlayTarget) -> bool {
    left.anime_id == right.anime_id
        && left.season == right.season
        && left.episode == right.episode
        && left.studio_name == right.studio_name
}

/// One source-bearing release in an explicitly ordered mainline timeline.
///
/// The caller decides which releases belong to the timeline, so specials,
/// recaps, and other extras never enter the playback timeline accidentally.
/// Source season and episode numbers are copied verbatim into [`PlayTarget`].
#[derive(Clone, Copy, Debug)]
pub struct PlaybackRelease<'a> {
    pub anime_id: u32,
    pub anime_title: &'a str,
    pub player_title: &'a str,
    pub sources: &'a EpisodeSourcesResponse,
}

pub fn selected_play_target(app: &AppState) -> Option<PlayTarget> {
    let e_idx = app.content.selected_episode_index?;

    // Витягуємо дані з обраної студії до будь-яких мутацій
    let studio_info = app.selected_studio().and_then(|s| {
        s.episodes.get(e_idx).map(|ep| {
            (
                ep.url.clone(),
                ep.episode_number,
                s.season_number,
                s.studio_name.clone(),
            )
        })
    });
    let (target_url, episode_num, actual_season, studio_name) = studio_info?;

    // Знаходимо anime_id для прогресу через studio_anime_ids
    let season_num = app.selected_season_num()?;
    let dub_idx = app.content.selected_dubbing_index?;
    let studio_idx = app.content.current_sources.clone().and_then(|sources| {
        sources
            .ashdi
            .iter()
            .enumerate()
            .filter(|(_, s)| s.season_number == season_num)
            .nth(dub_idx)
            .map(|(i, _)| i)
    });
    let progress_anime_id = studio_idx
        .and_then(|i| app.content.studio_anime_ids.get(i).copied())
        .or_else(|| {
            app.content
                .current_details
                .as_ref()
                .map(|details| details.id)
        })
        .or_else(|| {
            app.search
                .selected_result_index
                .and_then(|idx| app.search.results.get(idx).map(|a| a.id))
        })
        .unwrap_or(0);
    let progress_title = app
        .content
        .current_details
        .as_ref()
        .filter(|details| details.id == progress_anime_id)
        .map(|details| details.title_ukrainian.clone())
        .or_else(|| {
            app.search
                .results
                .iter()
                .find(|a| a.id == progress_anime_id)
                .map(|a| a.title_ukrainian.clone())
        })
        .or_else(|| {
            app.library_selected_anime()
                .map(|anime| anime.anime_title.clone())
        })
        .unwrap_or_default();

    let player_title =
        app.content
            .current_details
            .as_ref()
            .map(|details| {
                format!(
                    "{} ({})",
                    details.title_ukrainian,
                    details.year.unwrap_or(0)
                )
            })
            .or_else(|| {
                app.search.selected_result_index.and_then(|result_idx| {
                    app.search.results.get(result_idx).map(|anime| {
                        format!("{} ({})", anime.title_ukrainian, anime.year.unwrap_or(0))
                    })
                })
            })
            .unwrap_or_else(|| progress_title.clone());

    let episode_title = format!("Серія {}", episode_num);
    Some(PlayTarget {
        anime_id: progress_anime_id,
        anime_title: progress_title,
        player_title,
        season: actual_season,
        episode: episode_num,
        episode_title,
        stream_page_url: target_url,
        start_time: None,
        studio_name,
        referrer: "https://ashdi.vip/".to_string(),
    })
}

/// Build a deterministic timeline around the selected target. The supervisor
/// resolves the complete timeline before launching mpv.
pub fn build_playback_timeline(app: &AppState, target: &PlayTarget) -> PlaybackTimeline {
    let Some(sources) = app.content.current_sources.as_ref() else {
        return PlaybackTimeline::single(target.clone());
    };
    let release = PlaybackRelease {
        anime_id: target.anime_id,
        anime_title: &target.anime_title,
        player_title: &target.player_title,
        sources,
    };
    build_release_playback_timeline(target, &[release])
}

/// Build bidirectional targets across an ordered list of distinct releases.
///
/// Identity is never inferred from a normalized display season. Two
/// consecutive releases may both expose raw
/// season 1 and episode 1; their `anime_id` values keep them distinct.
pub fn build_release_playback_timeline(
    target: &PlayTarget,
    timeline: &[PlaybackRelease<'_>],
) -> PlaybackTimeline {
    let Some(start_release_index) = timeline.iter().position(|release| {
        release.anime_id == target.anime_id
            && release.sources.ashdi.iter().any(|studio| {
                studio.season_number == target.season
                    && studio.studio_name == target.studio_name
                    && studio
                        .episodes
                        .iter()
                        .any(|episode| episode.episode_number == target.episode)
            })
    }) else {
        return PlaybackTimeline::single(target.clone());
    };

    let mut entries = Vec::new();
    for (release_index, release) in timeline.iter().enumerate() {
        let mut seasons = release
            .sources
            .ashdi
            .iter()
            .map(|studio| studio.season_number)
            .collect::<Vec<_>>();
        seasons.sort_unstable();
        seasons.dedup();

        for season in seasons {
            let studios_for_season = release
                .sources
                .ashdi
                .iter()
                .filter(|studio| studio.season_number == season)
                .collect::<Vec<_>>();
            let studio = if release_index == start_release_index && season == target.season {
                studios_for_season.iter().copied().find(|studio| {
                    studio.studio_name == target.studio_name
                        && studio
                            .episodes
                            .iter()
                            .any(|episode| episode.episode_number == target.episode)
                })
            } else {
                studios_for_season
                    .iter()
                    .copied()
                    .find(|studio| studio.studio_name == target.studio_name)
                    .or_else(|| studios_for_season.first().copied())
            };
            let Some(studio) = studio else {
                continue;
            };

            let mut episodes = studio.episodes.iter().collect::<Vec<_>>();
            episodes.sort_by_key(|episode| episode.episode_number);
            entries.extend(episodes.into_iter().map(|episode| {
                let generated = play_target_for_release(release, studio, episode);
                if same_episode(&generated, target) {
                    target.clone()
                } else {
                    generated
                }
            }));
        }
    }
    PlaybackTimeline::from_entries(entries, target)
}

fn play_target_for_release(
    release: &PlaybackRelease<'_>,
    studio: &api::AshdiStudio,
    episode: &api::AshdiEpisode,
) -> PlayTarget {
    PlayTarget {
        anime_id: release.anime_id,
        anime_title: release.anime_title.to_string(),
        player_title: release.player_title.to_string(),
        season: studio.season_number,
        episode: episode.episode_number,
        episode_title: format!("Серія {}", episode.episode_number),
        stream_page_url: episode.url.clone(),
        start_time: None,
        studio_name: studio.studio_name.clone(),
        referrer: "https://ashdi.vip/".to_string(),
    }
}

#[derive(Clone)]
pub struct ContinueResolvedEpisode {
    pub season: u32,
    pub episode: u32,
    pub season_index: usize,
    pub dubbing_index: usize,
    pub episode_index: usize,
    pub url: String,
    pub start_time: Option<f64>,
    pub studio_name: String,
}

pub fn resolve_continue_target(
    progress: &storage::WatchProgress,
    sources: &EpisodeSourcesResponse,
) -> Option<ContinueResolvedEpisode> {
    let mut seasons: Vec<u32> = sources
        .ashdi
        .iter()
        .map(|studio| studio.season_number)
        .collect();
    seasons.sort();
    seasons.dedup();

    let exact_studio_data = sources
        .ashdi
        .iter()
        .filter(|studio| studio.season_number == progress.season)
        .enumerate()
        .find(|(_, studio)| studio.studio_name == progress.studio_name)
        .or_else(|| {
            sources
                .ashdi
                .iter()
                .filter(|studio| studio.season_number == progress.season)
                .enumerate()
                .next()
        });

    let current_studio_data = exact_studio_data?;
    let canonical_season = current_studio_data.1.season_number;
    let season_index = seasons
        .iter()
        .position(|season| *season == canonical_season)?;

    let current_dubbing_index = current_studio_data.0;
    let current_studio = current_studio_data.1;

    let current_episode_index = current_studio
        .episodes
        .iter()
        .position(|episode| episode.episode_number == progress.episode)?;

    if !progress.watched {
        let episode = current_studio.episodes.get(current_episode_index)?;
        return Some(ContinueResolvedEpisode {
            season: canonical_season,
            episode: progress.episode,
            season_index,
            dubbing_index: current_dubbing_index,
            episode_index: current_episode_index,
            url: episode.url.clone(),
            start_time: Some(progress.timestamp),
            studio_name: current_studio.studio_name.clone(),
        });
    }

    if let Some(next_episode) = current_studio.episodes.get(current_episode_index + 1) {
        return Some(ContinueResolvedEpisode {
            season: canonical_season,
            episode: next_episode.episode_number,
            season_index,
            dubbing_index: current_dubbing_index,
            episode_index: current_episode_index + 1,
            url: next_episode.url.clone(),
            start_time: Some(0.0),
            studio_name: current_studio.studio_name.clone(),
        });
    }

    let next_season = seasons.get(season_index + 1).copied()?;
    let next_studio_data = sources
        .ashdi
        .iter()
        .filter(|studio| studio.season_number == next_season)
        .enumerate()
        .next()?;
    let episode = next_studio_data.1.episodes.first()?;
    Some(ContinueResolvedEpisode {
        season: next_season,
        episode: episode.episode_number,
        season_index: season_index + 1,
        dubbing_index: next_studio_data.0,
        episode_index: 0,
        url: episode.url.clone(),
        start_time: Some(0.0),
        studio_name: next_studio_data.1.studio_name.clone(),
    })
}

// ---------------------------------------------------------------------------
// Supervisor API
// ---------------------------------------------------------------------------

static NEXT_PLAYBACK_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PlaybackSessionId(u64);

impl PlaybackSessionId {
    pub fn new() -> Self {
        Self(NEXT_PLAYBACK_SESSION_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PlaybackIdentity {
    pub anime_id: u32,
    pub anime_title: String,
    pub season: u32,
    pub episode: u32,
    pub studio_name: String,
}

impl From<&PlayTarget> for PlaybackIdentity {
    fn from(target: &PlayTarget) -> Self {
        Self {
            anime_id: target.anime_id,
            anime_title: target.anime_title.clone(),
            season: target.season,
            episode: target.episode,
            studio_name: target.studio_name.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProgressSnapshot {
    pub session_id: PlaybackSessionId,
    pub identity: PlaybackIdentity,
    pub position: f64,
    pub duration: f64,
    pub watched: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarkWatchedEvent {
    pub session_id: PlaybackSessionId,
    pub identity: PlaybackIdentity,
    pub position: f64,
    pub duration: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackEvent {
    SessionStarted {
        session_id: PlaybackSessionId,
        target: PlayTarget,
    },
    ProgressSnapshot(ProgressSnapshot),
    PauseChanged {
        session_id: PlaybackSessionId,
        identity: PlaybackIdentity,
        paused: bool,
        position: Option<f64>,
    },
    MarkWatched(MarkWatchedEvent),
    EndFile {
        session_id: PlaybackSessionId,
        reason: EndFileReason,
        playlist_entry_id: Option<i64>,
    },
    SessionStopped {
        session_id: PlaybackSessionId,
    },
    Error {
        session_id: Option<PlaybackSessionId>,
        message: String,
    },
}

#[derive(Clone, Debug)]
pub enum PlaybackCommand {
    Play {
        timeline: PlaybackTimeline,
        autoplay_next: bool,
    },
    Shutdown,
}

struct CommandEnvelope {
    command: PlaybackCommand,
    reply: oneshot::Sender<std::result::Result<(), String>>,
}

/// Bounded command/event handle. All mpv, monitor, stream-resolution, and
/// cancellation resources live behind its actor; callers only exchange
/// commands and typed events.
pub struct PlaybackSupervisor {
    commands: mpsc::Sender<CommandEnvelope>,
    events: mpsc::Receiver<PlaybackEvent>,
    actor: Option<JoinHandle<()>>,
}

impl PlaybackSupervisor {
    pub fn new() -> Self {
        let (commands, command_rx) = mpsc::channel(32);
        let (events, event_rx) = mpsc::channel(128);
        let actor = tokio::spawn(PlaybackActor::new(command_rx, events).run());
        Self {
            commands,
            events: event_rx,
            actor: Some(actor),
        }
    }

    pub async fn command(&self, command: PlaybackCommand) -> Result<()> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandEnvelope { command, reply })
            .await
            .map_err(|_| anyhow!("playback supervisor is shut down"))?;
        result
            .await
            .map_err(|_| anyhow!("playback supervisor command was cancelled"))?
            .map_err(|message| anyhow!(message))
    }

    pub async fn play(&self, timeline: PlaybackTimeline, autoplay_next: bool) -> Result<()> {
        self.command(PlaybackCommand::Play {
            timeline,
            autoplay_next,
        })
        .await
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        if self.actor.is_none() {
            return Ok(());
        }
        self.command(PlaybackCommand::Shutdown).await?;
        if let Some(actor) = self.actor.take() {
            actor
                .await
                .map_err(|error| anyhow!("playback supervisor task failed: {error}"))?;
        }
        Ok(())
    }

    pub fn try_recv_event(&mut self) -> Option<PlaybackEvent> {
        self.events.try_recv().ok()
    }

    pub fn drain_events(&mut self) -> Vec<PlaybackEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.try_recv_event() {
            events.push(event);
        }
        events
    }
}

impl Drop for PlaybackSupervisor {
    fn drop(&mut self) {
        // Explicit shutdown is still the normal path because it can query
        // final position and wait for graceful mpv exit.
        if let Some(actor) = self.actor.take() {
            actor.abort();
        }
    }
}

struct ResolvedPlaylist {
    /// Streams for `timeline[window_start..window_start + entries.len()]`.
    entries: Vec<MpvPlaylistEntry>,
    window_start: usize,
}

struct ActivePlayback {
    id: PlaybackSessionId,
    mpv: MpvSession,
    current: PlayTarget,
    timeline: PlaybackTimeline,
    /// First timeline index currently loaded into the native mpv playlist.
    window_start: usize,
    /// Number of entries currently in the mpv playlist window.
    window_len: usize,
    autoplay_next: bool,
    position: f64,
    duration: f64,
    has_position: bool,
    entry_id: Option<i64>,
    /// Pending **timeline** index after native playlist navigation.
    pending_index: Option<usize>,
    at_eof: bool,
}

enum ResolutionPurpose {
    Start {
        timeline: PlaybackTimeline,
        autoplay_next: bool,
        session_id: PlaybackSessionId,
    },
    Replace {
        timeline: PlaybackTimeline,
        autoplay_next: bool,
        session_id: PlaybackSessionId,
    },
    /// Internal window shift for the currently running logical session. The
    /// resolved snapshot is reconciled with live mpv state before replacement.
    Rewindow {
        timeline: PlaybackTimeline,
        session_id: PlaybackSessionId,
    },
    /// Autoplay reached the edge before background extension completed. Resolve
    /// around the next logical episode, append it, then use native playlist-next.
    Advance {
        timeline: PlaybackTimeline,
        session_id: PlaybackSessionId,
        next_index: usize,
    },
}

struct PendingResolution {
    cancellation: TaskCancellation,
    purpose: Option<ResolutionPurpose>,
    task: Option<JoinHandle<std::result::Result<ResolvedPlaylist, String>>>,
}

impl Drop for PendingResolution {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

struct PlaybackActor {
    commands: mpsc::Receiver<CommandEnvelope>,
    events: mpsc::Sender<PlaybackEvent>,
    active: Option<ActivePlayback>,
    pending: Option<PendingResolution>,
    /// Cached Ashdi m3u8 URLs so window shifts do not re-scrape every page.
    url_cache: Arc<Mutex<HashMap<String, CachedStreamUrl>>>,
    last_rewindow_attempt: Option<(PlaybackSessionId, Instant)>,
}

impl PlaybackActor {
    fn new(commands: mpsc::Receiver<CommandEnvelope>, events: mpsc::Sender<PlaybackEvent>) -> Self {
        Self {
            commands,
            events,
            active: None,
            pending: None,
            url_cache: Arc::new(Mutex::new(HashMap::new())),
            last_rewindow_attempt: None,
        }
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                envelope = self.commands.recv() => {
                    let Some(envelope) = envelope else {
                        self.cancel_pending().await;
                        self.stop_active(true).await;
                        return;
                    };
                    let is_shutdown = matches!(envelope.command, PlaybackCommand::Shutdown);
                    let result = self.handle_command(envelope.command).await;
                    let _ = envelope.reply.send(result);
                    if is_shutdown {
                        return;
                    }
                }
                _ = sleep(Duration::from_millis(40)) => {
                    self.tick().await;
                }
            }
        }
    }

    async fn emit(&self, event: PlaybackEvent) {
        let _ = self.events.send(event).await;
    }

    async fn handle_command(
        &mut self,
        command: PlaybackCommand,
    ) -> std::result::Result<(), String> {
        match command {
            PlaybackCommand::Play {
                timeline,
                autoplay_next,
            } => {
                self.cancel_pending().await;
                self.last_rewindow_attempt = None;
                let session_id = PlaybackSessionId::new();
                let purpose = if self.active.is_some() {
                    ResolutionPurpose::Replace {
                        timeline,
                        autoplay_next,
                        session_id,
                    }
                } else {
                    ResolutionPurpose::Start {
                        timeline,
                        autoplay_next,
                        session_id,
                    }
                };
                self.begin_resolution(purpose).await;
                Ok(())
            }
            PlaybackCommand::Shutdown => {
                self.cancel_pending().await;
                self.stop_active(true).await;
                Ok(())
            }
        }
    }

    async fn begin_resolution(&mut self, purpose: ResolutionPurpose) {
        let timeline = match &purpose {
            ResolutionPurpose::Start { timeline, .. }
            | ResolutionPurpose::Replace { timeline, .. }
            | ResolutionPurpose::Rewindow { timeline, .. }
            | ResolutionPurpose::Advance { timeline, .. } => timeline.clone(),
        };
        let cancellation = TaskCancellation::new();
        let task_cancellation = cancellation.clone();
        let url_cache = self.url_cache.clone();
        let task = tokio::spawn(async move {
            resolve_playlist(&timeline, &task_cancellation, url_cache)
                .await
                .map_err(|error| error.to_string())
        });
        self.pending = Some(PendingResolution {
            cancellation,
            purpose: Some(purpose),
            task: Some(task),
        });
    }

    /// Resolve another window near the native playlist edge and merge it in.
    async fn rewindow_active_if_needed(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if self
            .last_rewindow_attempt
            .is_some_and(|(session_id, attempted_at)| {
                session_id == active.id && attempted_at.elapsed() < PLAYLIST_REWINDOW_RETRY_DELAY
            })
        {
            return;
        }
        if !should_rewindow(
            active.window_start,
            active.window_len,
            active.timeline.current_index,
            active.timeline.entries.len(),
        ) {
            return;
        }
        let timeline = active.timeline.clone();
        let session_id = active.id;
        self.last_rewindow_attempt = Some((session_id, Instant::now()));
        self.begin_resolution(ResolutionPurpose::Rewindow {
            timeline,
            session_id,
        })
        .await;
    }

    async fn cancel_pending(&mut self) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        pending.cancellation.cancel();
        if let Some(task) = pending.task.take() {
            let _ = timeout(Duration::from_secs(2), task).await;
        }
    }

    async fn tick(&mut self) {
        if self
            .pending
            .as_ref()
            .and_then(|pending| pending.task.as_ref())
            .map(|task| task.is_finished())
            .unwrap_or(false)
        {
            if let Some(mut pending) = self.pending.take() {
                let purpose = pending
                    .purpose
                    .take()
                    .expect("pending resolution has purpose");
                let task = pending.task.take().expect("pending resolution has task");
                let result = match task.await {
                    Ok(result) => result,
                    Err(error) => Err(format!("stream resolution task failed: {error}")),
                };
                self.handle_resolution(purpose, result).await;
                return;
            }
        }

        let event = self
            .active
            .as_mut()
            .and_then(|active| active.mpv.try_recv_event().ok().flatten());
        if let Some(event) = event {
            self.handle_mpv_event(event).await;
            return;
        }

        let exited = self
            .active
            .as_mut()
            .and_then(|active| active.mpv.child_exited().ok().flatten())
            .is_some();
        if exited {
            let id = self.active.as_ref().map(|active| active.id);
            self.emit(PlaybackEvent::Error {
                session_id: id,
                message: "mpv exited before playback completed".to_string(),
            })
            .await;
            self.stop_active(true).await;
        }
    }

    async fn handle_resolution(
        &mut self,
        purpose: ResolutionPurpose,
        result: std::result::Result<ResolvedPlaylist, String>,
    ) {
        let resolved = match result {
            Ok(resolved) => resolved,
            Err(message) => {
                let session_id = match &purpose {
                    ResolutionPurpose::Start { session_id, .. }
                    | ResolutionPurpose::Replace { session_id, .. }
                    | ResolutionPurpose::Rewindow { session_id, .. }
                    | ResolutionPurpose::Advance { session_id, .. } => Some(*session_id),
                };
                self.emit(PlaybackEvent::Error {
                    session_id,
                    message,
                })
                .await;
                match purpose {
                    ResolutionPurpose::Start { session_id, .. } => {
                        self.emit(PlaybackEvent::SessionStopped { session_id })
                            .await;
                    }
                    ResolutionPurpose::Replace { .. } => {
                        // Resolution failed before the old player was touched.
                        // Re-announce it because the UI may have optimistically
                        // prepared a newly selected episode.
                        if let Some(active) = self.active.as_ref() {
                            self.emit(PlaybackEvent::SessionStarted {
                                session_id: active.id,
                                target: active.current.clone(),
                            })
                            .await;
                        }
                    }
                    ResolutionPurpose::Rewindow { .. } | ResolutionPurpose::Advance { .. } => {}
                }
                return;
            }
        };

        match purpose {
            ResolutionPurpose::Start {
                timeline,
                autoplay_next,
                session_id,
            } => {
                if self.active.is_none() {
                    self.launch_active(session_id, timeline, autoplay_next, resolved)
                        .await;
                }
            }
            ResolutionPurpose::Replace {
                timeline,
                autoplay_next,
                session_id,
            } => {
                self.replace_active(session_id, timeline, autoplay_next, resolved)
                    .await;
            }
            ResolutionPurpose::Rewindow { session_id, .. } => {
                self.extend_active_playlist(session_id, resolved, true, true)
                    .await;
            }
            ResolutionPurpose::Advance {
                session_id,
                next_index,
                ..
            } => {
                self.extend_active_playlist(session_id, resolved, false, false)
                    .await;
                let result = if let Some(active) = self.active.as_ref()
                    && timeline_in_window(active.window_start, active.window_len, next_index)
                {
                    match active.mpv.playlist_next().await {
                        Ok(()) => active.mpv.set_paused(false).await,
                        Err(error) => Err(error),
                    }
                } else {
                    Err(anyhow!("наступна серія не потрапила до mpv-плейлиста"))
                };
                if let Err(error) = result {
                    self.emit(PlaybackEvent::Error {
                        session_id: Some(session_id),
                        message: error.to_string(),
                    })
                    .await;
                }
            }
        }
    }

    async fn launch_active(
        &mut self,
        session_id: PlaybackSessionId,
        timeline: PlaybackTimeline,
        autoplay_next: bool,
        resolved: ResolvedPlaylist,
    ) {
        let target = timeline.current().clone();
        let playlist_start = timeline
            .current_index
            .saturating_sub(resolved.window_start)
            .min(resolved.entries.len().saturating_sub(1));
        let window_len = resolved.entries.len();
        let window_start = resolved.window_start;
        let mpv = match MpvSession::spawn(session_id.raw(), &resolved.entries, playlist_start).await
        {
            Ok(mpv) => mpv,
            Err(error) => {
                self.emit(PlaybackEvent::Error {
                    session_id: Some(session_id),
                    message: error.to_string(),
                })
                .await;
                self.emit(PlaybackEvent::SessionStopped { session_id })
                    .await;
                return;
            }
        };
        self.active = Some(ActivePlayback {
            id: session_id,
            mpv,
            current: target.clone(),
            timeline,
            window_start,
            window_len,
            autoplay_next,
            position: 0.0,
            duration: 0.0,
            has_position: false,
            entry_id: None,
            pending_index: None,
            at_eof: false,
        });
        self.emit(PlaybackEvent::SessionStarted { session_id, target })
            .await;
    }

    async fn replace_active(
        &mut self,
        session_id: PlaybackSessionId,
        timeline: PlaybackTimeline,
        autoplay_next: bool,
        resolved: ResolvedPlaylist,
    ) {
        let target = timeline.current().clone();
        let Some(mut old) = self.active.take() else {
            self.launch_active(session_id, timeline, autoplay_next, resolved)
                .await;
            return;
        };
        let old_snapshot = old.mpv.shutdown().await;
        self.emit_partial(&old, old_snapshot.time_pos, old_snapshot.duration)
            .await;
        self.emit(PlaybackEvent::SessionStopped { session_id: old.id })
            .await;
        let playlist_start = timeline
            .current_index
            .saturating_sub(resolved.window_start)
            .min(resolved.entries.len().saturating_sub(1));
        let window_len = resolved.entries.len();
        let window_start = resolved.window_start;
        let launch_result =
            MpvSession::spawn(session_id.raw(), &resolved.entries, playlist_start).await;
        match launch_result {
            Ok(mpv) => {
                self.active = Some(ActivePlayback {
                    id: session_id,
                    mpv,
                    current: target.clone(),
                    timeline,
                    window_start,
                    window_len,
                    autoplay_next,
                    position: 0.0,
                    duration: 0.0,
                    has_position: false,
                    entry_id: None,
                    pending_index: None,
                    at_eof: false,
                });
                self.emit(PlaybackEvent::SessionStarted { session_id, target })
                    .await;
            }
            Err(error) => {
                self.emit(PlaybackEvent::Error {
                    session_id: Some(session_id),
                    message: error.to_string(),
                })
                .await;
                if session_id != old.id {
                    self.emit(PlaybackEvent::SessionStopped { session_id })
                        .await;
                }
            }
        }
    }

    /// Grow the native mpv playlist in place. Resolution may have taken long
    /// enough for the user to navigate again, so discard a window which no
    /// longer overlaps the live playhead and resolve around the latest entry.
    async fn extend_active_playlist(
        &mut self,
        session_id: PlaybackSessionId,
        resolved: ResolvedPlaylist,
        require_live_playhead: bool,
        allow_prepend: bool,
    ) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.id != session_id {
            return;
        }
        let current_index =
            live_timeline_index(active.timeline.current_index, active.pending_index);
        if require_live_playhead
            && !timeline_in_window(resolved.window_start, resolved.entries.len(), current_index)
        {
            // The playhead outran this request. Discard it and immediately
            // resolve a window around the latest entry without touching mpv.
            let mut timeline = active.timeline.clone();
            timeline.current_index = current_index;
            self.begin_resolution(ResolutionPurpose::Rewindow {
                timeline,
                session_id,
            })
            .await;
            return;
        }

        let Some(plan) = playlist_extension_plan(
            active.window_start,
            active.window_len,
            resolved.window_start,
            resolved.entries.len(),
        ) else {
            self.last_rewindow_attempt = Some((session_id, Instant::now()));
            return;
        };

        let before = if allow_prepend {
            &resolved.entries[plan.before.clone()]
        } else {
            &resolved.entries[0..0]
        };
        let after = &resolved.entries[plan.after.clone()];
        if before.is_empty() && after.is_empty() {
            self.last_rewindow_attempt = Some((session_id, Instant::now()));
            return;
        }

        let outcome = {
            let Some(active) = self.active.as_mut() else {
                return;
            };
            active.mpv.extend_playlist(before, after).await
        };
        if let Some(active) = self.active.as_mut() {
            (active.window_start, active.window_len) = extended_playlist_range(
                active.window_start,
                active.window_len,
                outcome.prepended,
                outcome.appended,
            );
        }

        if let Some(message) = outcome.error {
            self.last_rewindow_attempt = Some((session_id, Instant::now()));
            self.emit(PlaybackEvent::Error {
                session_id: Some(session_id),
                message,
            })
            .await;
        } else if outcome.prepended + outcome.appended > 0 {
            self.last_rewindow_attempt = None;
        } else {
            self.last_rewindow_attempt = Some((session_id, Instant::now()));
        }
    }

    async fn handle_mpv_event(&mut self, event: MpvMonitorEvent) {
        match event {
            MpvMonitorEvent::Progress { time_pos, duration } => {
                let snapshot = if let Some(active) = self.active.as_mut() {
                    if active.pending_index.is_some() {
                        return;
                    }
                    if let Some(time_pos) =
                        time_pos.filter(|value| value.is_finite() && *value >= 0.0)
                    {
                        active.position = time_pos;
                        active.has_position = true;
                    }
                    if let Some(duration) =
                        duration.filter(|value| value.is_finite() && *value >= 0.0)
                    {
                        active.duration = duration;
                    }
                    if active.has_position && !active.at_eof {
                        Some(Self::progress_snapshot(active, false))
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(snapshot) = snapshot {
                    self.emit(PlaybackEvent::ProgressSnapshot(snapshot)).await;
                }
            }
            MpvMonitorEvent::PauseChanged { paused, time_pos } => {
                let event = self.active.as_mut().and_then(|active| {
                    if active.pending_index.is_some() {
                        return None;
                    }
                    let position = if let Some(time_pos) =
                        time_pos.filter(|value| value.is_finite() && *value >= 0.0)
                    {
                        active.position = time_pos;
                        active.has_position = true;
                        Some(time_pos)
                    } else {
                        active.has_position.then_some(active.position)
                    };
                    Some(PlaybackEvent::PauseChanged {
                        session_id: active.id,
                        identity: PlaybackIdentity::from(&active.current),
                        paused,
                        position,
                    })
                });
                if let Some(event) = event {
                    self.emit(event).await;
                }
            }
            MpvMonitorEvent::PlaylistPosition { position, entry_id } => {
                if let Some(active) = self.active.as_mut() {
                    active.entry_id = entry_id.or(active.entry_id);
                    if let Some(local) = position.filter(|index| *index < active.window_len) {
                        let timeline_index = active.window_start + local;
                        if timeline_index != active.timeline.current_index {
                            active.pending_index = Some(timeline_index);
                        }
                    }
                }
            }
            MpvMonitorEvent::FileStarted { playlist_entry_id } => {
                if let Some(active) = self.active.as_mut() {
                    let local =
                        playlist_entry_id.and_then(|entry_id| active.mpv.playlist_index(entry_id));
                    if let Some(local) = local {
                        let timeline_index = active.window_start + local;
                        if timeline_index != active.timeline.current_index {
                            active.pending_index = Some(timeline_index);
                        }
                    }
                    active.entry_id = playlist_entry_id.or(active.entry_id);
                }
            }
            MpvMonitorEvent::FileLoaded { playlist_entry_id } => {
                let (started, resume_from_eof) = if let Some(active) = self.active.as_mut() {
                    let local =
                        playlist_entry_id.and_then(|entry_id| active.mpv.playlist_index(entry_id));
                    let timeline_index = local
                        .map(|index| active.window_start + index)
                        .or(active.pending_index)
                        .unwrap_or(active.timeline.current_index);
                    active.pending_index = None;
                    active.entry_id = playlist_entry_id.or(active.entry_id);
                    let resume_from_eof = active.at_eof;
                    if timeline_index != active.timeline.current_index {
                        let selected = active
                            .timeline
                            .select(timeline_index)
                            .unwrap_or_else(|| active.current.clone());
                        active.current = selected.clone();
                        active.position = 0.0;
                        active.duration = 0.0;
                        active.has_position = false;
                        active.at_eof = false;
                        (Some((active.id, selected)), resume_from_eof)
                    } else {
                        active.at_eof = false;
                        (None, resume_from_eof)
                    }
                } else {
                    (None, false)
                };
                if resume_from_eof && let Some(active) = self.active.as_ref() {
                    let _ = active.mpv.set_paused(false).await;
                }
                if let Some((session_id, target)) = started {
                    self.emit(PlaybackEvent::SessionStarted { session_id, target })
                        .await;
                }
                // After settling on a new episode near the window edge, shift the
                // sliding window so prev/next keep working for long seasons.
                self.rewindow_active_if_needed().await;
            }
            MpvMonitorEvent::EofReached(true) => self.natural_eof().await,
            MpvMonitorEvent::EofReached(false) => {}
            MpvMonitorEvent::EndFile(end_file) => {
                let (id, entry_id, partial) = {
                    let Some(active) = self.active.as_mut() else {
                        return;
                    };
                    let partial = if matches!(end_file.reason, EndFileReason::Stop)
                        && active.has_position
                        && !active.at_eof
                    {
                        let resume = (active.position > 0.0).then_some(active.position);
                        active.timeline.entries[active.timeline.current_index].start_time = resume;
                        active.current.start_time = resume;
                        Some(Self::progress_snapshot(active, false))
                    } else {
                        None
                    };
                    (
                        active.id,
                        end_file.playlist_entry_id.or(active.entry_id),
                        partial,
                    )
                };
                if let Some(partial) = partial {
                    self.emit(PlaybackEvent::ProgressSnapshot(partial)).await;
                }
                self.emit(PlaybackEvent::EndFile {
                    session_id: id,
                    reason: end_file.reason.clone(),
                    playlist_entry_id: entry_id,
                })
                .await;
                match end_file.reason {
                    EndFileReason::Stop => {
                        // Native playlist-prev/next unloads the old entry with
                        // reason=stop. The following start/file-loaded events
                        // commit the new logical episode.
                    }
                    EndFileReason::Eof => self.natural_eof().await,
                    EndFileReason::Quit => self.stop_active(false).await,
                    EndFileReason::Error
                    | EndFileReason::Abort
                    | EndFileReason::Redirect
                    | EndFileReason::Unknown(_) => self.stop_active(true).await,
                }
            }
            MpvMonitorEvent::MonitorFailed(message) => {
                let id = self.active.as_ref().map(|active| active.id);
                self.emit(PlaybackEvent::Error {
                    session_id: id,
                    message,
                })
                .await;
                self.stop_active(true).await;
            }
            MpvMonitorEvent::Closed => {
                let id = self.active.as_ref().map(|active| active.id);
                self.emit(PlaybackEvent::Error {
                    session_id: id,
                    message: "mpv IPC monitor closed".to_string(),
                })
                .await;
                self.stop_active(true).await;
            }
        }
    }

    async fn natural_eof(&mut self) {
        let (snapshot, mark, should_advance, next_in_window) = {
            let Some(active) = self.active.as_mut() else {
                return;
            };
            if active.at_eof {
                return;
            }
            active.at_eof = true;
            // Going back to a fully completed episode should replay it from
            // the beginning, not from the timestamp used to enter this
            // session or the final frame.
            active.timeline.entries[active.timeline.current_index].start_time = None;
            active.current.start_time = None;
            let id = active.id;
            let position = active.position.max(active.duration);
            let duration = active.duration;
            let snapshot = if position > 0.0 {
                Some(Self::progress_snapshot(active, true))
            } else {
                None
            };
            let mark = MarkWatchedEvent {
                session_id: id,
                identity: PlaybackIdentity::from(&active.current),
                position,
                duration,
            };
            let next_index = active
                .timeline
                .has_next()
                .then_some(active.timeline.current_index + 1);
            let next_in_window = next_index.is_some_and(|index| {
                timeline_in_window(active.window_start, active.window_len, index)
            });
            let should_advance = active.autoplay_next && next_index.is_some();
            (snapshot, mark, should_advance, next_in_window)
        };
        if let Some(snapshot) = snapshot {
            self.emit(PlaybackEvent::ProgressSnapshot(snapshot)).await;
        }
        self.emit(PlaybackEvent::MarkWatched(mark)).await;
        if !should_advance {
            return;
        }
        if next_in_window {
            let result = if let Some(active) = self.active.as_ref() {
                match active.mpv.playlist_next().await {
                    Ok(()) => active.mpv.set_paused(false).await,
                    Err(error) => Err(error),
                }
            } else {
                Ok(())
            };
            if let Err(error) = result {
                let id = self.active.as_ref().map(|active| active.id);
                self.emit(PlaybackEvent::Error {
                    session_id: id,
                    message: error.to_string(),
                })
                .await;
            }
            return;
        }
        // Next episode is outside the loaded window. Resolve it and append to
        // the live native playlist; the mpv process and current EOF state stay.
        if let Some(active) = self.active.as_ref() {
            let next = active.timeline.current_index + 1;
            if next < active.timeline.entries.len() {
                let mut timeline = active.timeline.clone();
                timeline.current_index = next;
                let session_id = active.id;
                self.begin_resolution(ResolutionPurpose::Advance {
                    timeline,
                    session_id,
                    next_index: next,
                })
                .await;
            }
        }
    }

    fn progress_snapshot(active: &ActivePlayback, watched: bool) -> ProgressSnapshot {
        ProgressSnapshot {
            session_id: active.id,
            identity: PlaybackIdentity::from(&active.current),
            position: active.position,
            duration: active.duration,
            watched,
        }
    }

    async fn emit_partial(&self, active: &ActivePlayback, time_pos: f64, duration: f64) {
        let position = if time_pos.is_finite() && time_pos > 0.0 {
            time_pos
        } else {
            active.position
        };
        if position <= 0.0 {
            return;
        }
        self.emit(PlaybackEvent::ProgressSnapshot(ProgressSnapshot {
            session_id: active.id,
            identity: PlaybackIdentity::from(&active.current),
            position,
            duration: if duration > 0.0 {
                duration
            } else {
                active.duration
            },
            watched: false,
        }))
        .await;
    }

    async fn stop_active(&mut self, emit_partial: bool) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        let snapshot = active.mpv.shutdown().await;
        if emit_partial {
            self.emit_partial(&active, snapshot.time_pos, snapshot.duration)
                .await;
        }
        self.emit(PlaybackEvent::SessionStopped {
            session_id: active.id,
        })
        .await;
    }
}

const PLAYLIST_RESOLUTION_CONCURRENCY: usize = 6;
/// Once the selected stream is known to work, do not make startup wait for a
/// slow or broken unrelated neighbor. Fast neighbors still become native mpv
/// playlist entries during this short grace period.
const PLAYLIST_NEIGHBOR_GRACE: Duration = Duration::from_secs(2);

async fn resolve_playlist_entry(
    target: PlayTarget,
    parser: Arc<api::AshdiParser>,
    cancellation: TaskCancellation,
    url_cache: Arc<Mutex<HashMap<String, CachedStreamUrl>>>,
) -> Result<MpvPlaylistEntry> {
    let cache_key = play_target_cache_key(&target);
    let cached_url = {
        let mut cache = url_cache.lock().await;
        fresh_cached_url(&mut cache, &cache_key, Instant::now())
    };
    if let Some(url) = cached_url {
        return Ok(MpvPlaylistEntry {
            media_url: url,
            title: format!("{} - {}", target.player_title, target.episode_title),
            start_time: target.start_time,
            referrer: target.referrer,
        });
    }

    let url = tokio::select! {
        _ = cancellation.cancelled() => return Err(anyhow!("playlist resolution cancelled")),
        result = parser.extract_m3u8(&target.stream_page_url) => result
            .map_err(|error| anyhow!(
                "не вдалося отримати S{}E{}: {error}",
                target.season,
                target.episode
            ))?,
    };
    let now = Instant::now();
    let mut cache = url_cache.lock().await;
    cache.retain(|_, entry| now.duration_since(entry.inserted_at) < STREAM_URL_CACHE_TTL);
    cache.insert(
        cache_key,
        CachedStreamUrl {
            url: url.clone(),
            inserted_at: now,
        },
    );
    drop(cache);
    Ok(MpvPlaylistEntry {
        media_url: url,
        title: format!("{} - {}", target.player_title, target.episode_title),
        start_time: target.start_time,
        referrer: target.referrer,
    })
}

async fn resolve_playlist(
    timeline: &PlaybackTimeline,
    cancellation: &TaskCancellation,
    url_cache: Arc<Mutex<HashMap<String, CachedStreamUrl>>>,
) -> Result<ResolvedPlaylist> {
    if cancellation.is_cancelled() {
        return Err(anyhow!("playlist resolution cancelled"));
    }
    if timeline.entries.is_empty() {
        return Err(anyhow!("playback timeline is empty"));
    }

    let (window_start, window_end) =
        playlist_window(timeline.current_index, timeline.entries.len());
    let window_targets = timeline.entries[window_start..window_end].to_vec();
    if window_targets.is_empty() {
        return Err(anyhow!("playback window is empty"));
    }

    let parser = Arc::new(api::AshdiParser::new()?);
    let selected_offset = timeline.current_index - window_start;
    let mut entries = vec![None; window_end - window_start];
    entries[selected_offset] = Some(
        resolve_playlist_entry(
            window_targets[selected_offset].clone(),
            parser.clone(),
            cancellation.clone(),
            url_cache.clone(),
        )
        .await?,
    );

    let concurrency = Arc::new(Semaphore::new(PLAYLIST_RESOLUTION_CONCURRENCY));
    let mut neighbor_targets = window_targets
        .into_iter()
        .enumerate()
        .filter(|(offset, _)| *offset != selected_offset)
        .collect::<Vec<_>>();
    neighbor_targets.sort_by_key(|(offset, _)| offset.abs_diff(selected_offset));
    let mut tasks = JoinSet::new();
    for (offset, target) in neighbor_targets {
        let parser = parser.clone();
        let concurrency = concurrency.clone();
        let cancellation = cancellation.clone();
        let url_cache = url_cache.clone();
        tasks.spawn(async move {
            let permit = tokio::select! {
                _ = cancellation.cancelled() => return (offset, Err(anyhow!("playlist resolution cancelled"))),
                permit = concurrency.acquire_owned() => permit,
            };
            let result = match permit {
                Ok(permit) => {
                    let result =
                        resolve_playlist_entry(target, parser, cancellation, url_cache).await;
                    drop(permit);
                    result
                }
                Err(_) => Err(anyhow!("playlist resolver was closed")),
            };
            (offset, result)
        });
    }

    let collect_neighbors = async {
        while let Some(outcome) = tasks.join_next().await {
            if let Ok((offset, Ok(entry))) = outcome {
                entries[offset] = Some(entry);
            }
        }
    };
    tokio::select! {
        _ = cancellation.cancelled() => return Err(anyhow!("playlist resolution cancelled")),
        _ = timeout(PLAYLIST_NEIGHBOR_GRACE, collect_neighbors) => {}
    }
    tasks.abort_all();

    // Neighbor failures must not block an otherwise playable selected episode.
    // Keep the largest contiguous native playlist containing the selection so
    // timeline indexes remain exact and mpv prev/next never skip silently.
    let (contiguous_start, contiguous_end) = contiguous_resolved_range(&entries, selected_offset)
        .expect("selected playlist entry was checked above");
    let entries = entries[contiguous_start..contiguous_end]
        .iter_mut()
        .map(|entry| entry.take().expect("contiguous entry was resolved"))
        .collect();
    Ok(ResolvedPlaylist {
        entries,
        window_start: window_start + contiguous_start,
    })
}

#[cfg(test)]
mod supervisor_tests {
    use super::*;

    fn sources(season: u32, episodes: &[u32]) -> EpisodeSourcesResponse {
        EpisodeSourcesResponse {
            ashdi: vec![api::AshdiStudio {
                id: season,
                studio_name: "dub".to_string(),
                season_number: season,
                episodes: episodes
                    .iter()
                    .map(|episode| api::AshdiEpisode {
                        episode_number: *episode,
                        display_episode_number: None,
                        title: format!("Episode {episode}"),
                        url: format!("https://ashdi.vip/s{season}/e{episode}"),
                        ashdi_episode_id: format!("{season}-{episode}"),
                    })
                    .collect(),
                episodes_count: episodes.len() as u32,
            }],
            moonanime: Vec::new(),
        }
    }

    fn target(episode: u32, start_time: Option<f64>) -> PlayTarget {
        PlayTarget {
            anime_id: 1,
            anime_title: "Test".to_string(),
            player_title: "Test (2026)".to_string(),
            season: 1,
            episode,
            episode_title: format!("Episode {episode}"),
            stream_page_url: format!("https://media.test/{episode}.m3u8"),
            start_time,
            studio_name: "dub".to_string(),
            referrer: "https://ashdi.vip/".to_string(),
        }
    }

    #[test]
    fn playlist_window_keeps_twelve_behind_and_ahead() {
        // 12 behind + current + 12 ahead = 25 when not clipped.
        assert_eq!(playlist_window(0, 270), (0, 13));
        assert_eq!(playlist_window(12, 270), (0, 25));
        assert_eq!(playlist_window(257, 270), (245, 270));
        assert_eq!(playlist_window(269, 270), (257, 270));
        assert_eq!(
            playlist_window(100, 270).1 - playlist_window(100, 270).0,
            25
        );
    }

    #[test]
    fn rewindow_triggers_near_loaded_edges_only() {
        // Window [88, 113) while sitting on 100 — middle, no rewindow.
        assert!(!should_rewindow(88, 25, 100, 270));
        // Near end of window with more timeline ahead.
        assert!(should_rewindow(88, 25, 109, 270));
        // Near start with more timeline behind.
        assert!(should_rewindow(88, 25, 91, 270));
        // Whole season fits — never rewindow.
        assert!(!should_rewindow(0, 80, 40, 80));
    }

    #[test]
    fn unresolved_neighbors_are_trimmed_without_losing_selected_episode() {
        let entries = [Some(1), Some(2), None, Some(4), Some(5), None, Some(7)];
        assert_eq!(contiguous_resolved_range(&entries, 4), Some((3, 5)));
        assert_eq!(contiguous_resolved_range(&entries, 1), Some((0, 2)));
        assert_eq!(contiguous_resolved_range(&entries, 2), None);
    }

    #[test]
    fn extension_plan_adds_only_entries_outside_the_loaded_native_playlist() {
        assert_eq!(
            playlist_extension_plan(10, 10, 5, 20),
            Some(PlaylistExtensionPlan {
                before: 0..5,
                after: 15..20,
            })
        );
        assert_eq!(
            playlist_extension_plan(10, 10, 5, 13),
            Some(PlaylistExtensionPlan {
                before: 0..5,
                after: 13..13,
            })
        );
        assert_eq!(
            playlist_extension_plan(10, 10, 15, 10),
            Some(PlaylistExtensionPlan {
                before: 0..0,
                after: 5..10,
            })
        );
        assert_eq!(
            playlist_extension_plan(10, 10, 20, 5),
            Some(PlaylistExtensionPlan {
                before: 0..0,
                after: 0..5,
            })
        );
        assert_eq!(
            playlist_extension_plan(10, 10, 5, 5),
            Some(PlaylistExtensionPlan {
                before: 0..5,
                after: 5..5,
            })
        );
        assert_eq!(playlist_extension_plan(10, 10, 21, 5), None);
    }

    #[test]
    fn partial_ipc_extension_keeps_timeline_indexes_contiguous() {
        assert_eq!(extended_playlist_range(10, 10, 3, 2), (7, 15));
        assert!(timeline_in_window(7, 15, 7));
        assert!(timeline_in_window(7, 15, 21));
        assert!(!timeline_in_window(7, 15, 22));
    }

    #[test]
    fn pending_native_navigation_wins_over_stale_rewindow_snapshot() {
        assert_eq!(live_timeline_index(5, Some(6)), 6);
        assert_eq!(live_timeline_index(5, None), 5);
    }

    #[test]
    fn direct_stream_cache_expires_and_keys_include_source_page() {
        let now = Instant::now();
        let mut cache = HashMap::from([
            (
                "fresh".to_string(),
                CachedStreamUrl {
                    url: "https://cdn.test/fresh.m3u8".to_string(),
                    inserted_at: now - Duration::from_secs(30),
                },
            ),
            (
                "stale".to_string(),
                CachedStreamUrl {
                    url: "https://cdn.test/stale.m3u8".to_string(),
                    inserted_at: now - STREAM_URL_CACHE_TTL - Duration::from_secs(1),
                },
            ),
        ]);
        assert_eq!(
            fresh_cached_url(&mut cache, "fresh", now).as_deref(),
            Some("https://cdn.test/fresh.m3u8")
        );
        assert_eq!(fresh_cached_url(&mut cache, "stale", now), None);
        assert!(!cache.contains_key("stale"));

        let mut first = target(1, None);
        let first_key = play_target_cache_key(&first);
        first.stream_page_url = "https://media.test/replaced".to_string();
        assert_ne!(first_key, play_target_cache_key(&first));
    }

    #[test]
    fn session_ids_are_unique_and_ordered() {
        let first = PlaybackSessionId::new();
        let second = PlaybackSessionId::new();
        assert!(second > first);
    }

    #[test]
    fn identity_does_not_include_resume_time() {
        let identity = PlaybackIdentity::from(&target(4, Some(123.0)));
        assert_eq!(identity.episode, 4);
        assert_eq!(identity.studio_name, "dub");
    }

    #[test]
    fn duplicate_raw_season_one_remains_distinct_across_release_ids() {
        let part_one = sources(1, &[1, 2]);
        let part_two = sources(1, &[1, 2]);
        let releases = [
            PlaybackRelease {
                anime_id: 10,
                anime_title: "Part 1",
                player_title: "Part 1 (2024)",
                sources: &part_one,
            },
            PlaybackRelease {
                anime_id: 20,
                anime_title: "Part 2",
                player_title: "Part 2 (2024)",
                sources: &part_two,
            },
        ];

        let timeline = build_release_playback_timeline(&target_for(10, 1, 1), &releases);
        let identities = timeline
            .entries
            .iter()
            .map(|target| (target.anime_id, target.season, target.episode))
            .collect::<Vec<_>>();
        assert_eq!(
            identities,
            vec![(10, 1, 1), (10, 1, 2), (20, 1, 1), (20, 1, 2)]
        );
        assert_eq!(timeline.current_index, 0);
    }

    #[test]
    fn ordered_timeline_crosses_cours_and_skips_unlisted_extra() {
        let part_one = sources(1, &[1, 2]);
        let extra = sources(1, &[1]);
        let part_two = sources(1, &[1, 2]);
        let next_release = sources(1, &[1]);
        let releases = [
            PlaybackRelease {
                anime_id: 10,
                anime_title: "Part 1",
                player_title: "Part 1",
                sources: &part_one,
            },
            PlaybackRelease {
                anime_id: 20,
                anime_title: "Part 2",
                player_title: "Part 2",
                sources: &part_two,
            },
            PlaybackRelease {
                anime_id: 30,
                anime_title: "Next",
                player_title: "Next",
                sources: &next_release,
            },
        ];

        let timeline = build_release_playback_timeline(&target_for(10, 1, 2), &releases);
        assert_eq!(
            timeline
                .entries
                .iter()
                .map(|target| (target.anime_id, target.episode))
                .collect::<Vec<_>>(),
            vec![(10, 1), (10, 2), (20, 1), (20, 2), (30, 1)]
        );
        assert_eq!(timeline.current_index, 1);
        assert!(timeline.entries.iter().all(|target| target.anime_id != 15));
        let _unlisted_extra = PlaybackRelease {
            anime_id: 15,
            anime_title: "Recap",
            player_title: "Recap",
            sources: &extra,
        };
    }

    #[test]
    fn timeline_opened_at_episode_six_can_move_to_both_bounds() {
        let release_sources = sources(1, &(1..=12).collect::<Vec<_>>());
        let releases = [PlaybackRelease {
            anime_id: 10,
            anime_title: "Season 1",
            player_title: "Season 1",
            sources: &release_sources,
        }];
        let mut selected = target_for(10, 1, 6);
        selected.start_time = Some(123.0);

        let mut timeline = build_release_playback_timeline(&selected, &releases);
        assert_eq!(timeline.entries.len(), 12);
        assert_eq!(timeline.current_index, 5);
        assert_eq!(timeline.current().episode, 6);
        assert_eq!(timeline.current().start_time, Some(123.0));
        assert!(timeline.current_index > 0);
        assert!(timeline.has_next());

        timeline.select(0).unwrap();
        assert_eq!(timeline.current().episode, 1);
        assert_eq!(timeline.current_index, 0);
        timeline.select(11).unwrap();
        assert_eq!(timeline.current().episode, 12);
        assert!(!timeline.has_next());
    }

    #[test]
    fn timeline_sorts_episodes_even_when_the_source_does_not() {
        let release_sources = sources(1, &[3, 1, 2]);
        let releases = [PlaybackRelease {
            anime_id: 10,
            anime_title: "Season 1",
            player_title: "Season 1",
            sources: &release_sources,
        }];

        let timeline = build_release_playback_timeline(&target_for(10, 1, 2), &releases);
        assert_eq!(
            timeline
                .entries
                .iter()
                .map(|target| target.episode)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(timeline.current_index, 1);
    }

    #[test]
    fn continue_requires_the_exact_stored_season() {
        let sources = sources(1, &[1, 2]);
        let progress = storage::WatchProgress {
            anime_id: 10,
            anime_title: "Part 1".to_string(),
            season: 3,
            episode: 2,
            studio_name: "dub".to_string(),
            timestamp: 42.0,
            duration: 1200.0,
            watched: false,
            updated_at: 1,
        };

        assert!(resolve_continue_target(&progress, &sources).is_none());
    }

    fn target_for(anime_id: u32, season: u32, episode: u32) -> PlayTarget {
        PlayTarget {
            anime_id,
            anime_title: format!("Release {anime_id}"),
            player_title: format!("Release {anime_id}"),
            season,
            episode,
            episode_title: format!("Episode {episode}"),
            stream_page_url: format!("https://ashdi.vip/s{season}/e{episode}"),
            start_time: None,
            studio_name: "dub".to_string(),
            referrer: "https://ashdi.vip/".to_string(),
        }
    }

    #[tokio::test]
    async fn command_channel_is_bounded_and_shutdown_is_safe_without_mpv() {
        let mut supervisor = PlaybackSupervisor::new();
        // A non-existent host URL is only queued; the actor owns/cancels its
        // resolver. This verifies the API can be shut down without leaking the
        // task even when no mpv process is available.
        supervisor
            .play(
                PlaybackTimeline::from_entries(
                    vec![target(1, None), target(2, None)],
                    &target(1, None),
                ),
                true,
            )
            .await
            .unwrap();
        supervisor.shutdown().await.unwrap();
    }
}
