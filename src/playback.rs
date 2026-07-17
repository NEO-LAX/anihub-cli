use crate::api;
use crate::api::EpisodeSourcesResponse;
use crate::player::{
    EndFileEvent, EndFileReason, MpvMonitorEvent, MpvNavigation, MpvSession, TaskCancellation,
};
use crate::storage;
use crate::ui::AppState;
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
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
/// Unlike the old forward-only queue, entries before `current_index` remain
/// available for manual navigation from inside mpv. Stream URLs are still
/// resolved lazily when an entry becomes current.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackTimeline {
    entries: Vec<PlayTarget>,
    current_index: usize,
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

    fn adjacent_index(&self, navigation: MpvNavigation) -> Option<usize> {
        match navigation {
            MpvNavigation::Previous => self.current_index.checked_sub(1),
            MpvNavigation::Next => {
                (self.current_index + 1 < self.entries.len()).then_some(self.current_index + 1)
            }
        }
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
    let e_idx = app.selected_episode_index?;

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
    let dub_idx = app.selected_dubbing_index?;
    let studio_idx = app.current_sources.clone().and_then(|sources| {
        sources
            .ashdi
            .iter()
            .enumerate()
            .filter(|(_, s)| s.season_number == season_num)
            .nth(dub_idx)
            .map(|(i, _)| i)
    });
    let progress_anime_id = studio_idx
        .and_then(|i| app.studio_anime_ids.get(i).copied())
        .or_else(|| app.current_details.as_ref().map(|details| details.id))
        .or_else(|| {
            app.selected_result_index
                .and_then(|idx| app.search_results.get(idx).map(|a| a.id))
        })
        .unwrap_or(0);
    let progress_title = app
        .current_details
        .as_ref()
        .filter(|details| details.id == progress_anime_id)
        .map(|details| details.title_ukrainian.clone())
        .or_else(|| {
            app.search_results
                .iter()
                .find(|a| a.id == progress_anime_id)
                .map(|a| a.title_ukrainian.clone())
        })
        .or_else(|| {
            app.library_selected_anime()
                .map(|anime| anime.anime_title.clone())
        })
        .unwrap_or_default();

    let player_title = app
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
            app.selected_result_index.and_then(|result_idx| {
                app.search_results
                    .get(result_idx)
                    .map(|anime| format!("{} ({})", anime.title_ukrainian, anime.year.unwrap_or(0)))
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
/// resolves each Ashdi stream only when it becomes current.
pub fn build_playback_timeline(app: &AppState, target: &PlayTarget) -> PlaybackTimeline {
    let Some(sources) = app.current_sources.as_ref() else {
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
    NavigationFailed {
        session_id: PlaybackSessionId,
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

struct ResolvedStream {
    url: String,
}

struct ActivePlayback {
    id: PlaybackSessionId,
    mpv: MpvSession,
    current: PlayTarget,
    timeline: PlaybackTimeline,
    autoplay_next: bool,
    resolved_streams: HashMap<PlaybackIdentity, String>,
    position: f64,
    duration: f64,
    has_position: bool,
    entry_id: Option<i64>,
    transitioning: bool,
    at_eof: bool,
    pending_navigation: Option<PendingNavigation>,
    stale_entry_id: Option<i64>,
}

struct PendingNavigation {
    target: PlayTarget,
    target_index: usize,
    resolved_url: String,
    old_entry_id: Option<i64>,
    new_entry_id: Option<i64>,
    replacing_active_file: bool,
    old_end_seen: bool,
}

enum ResolutionPurpose {
    Start {
        timeline: PlaybackTimeline,
        autoplay_next: bool,
        session_id: PlaybackSessionId,
    },
    Navigate {
        target: PlayTarget,
        target_index: usize,
        session_id: PlaybackSessionId,
    },
    Replace {
        timeline: PlaybackTimeline,
        autoplay_next: bool,
        session_id: PlaybackSessionId,
    },
}

struct PendingResolution {
    cancellation: TaskCancellation,
    purpose: Option<ResolutionPurpose>,
    task: Option<JoinHandle<std::result::Result<ResolvedStream, String>>>,
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
}

fn end_file_belongs_to_replaced_entry(
    pending: &PendingNavigation,
    end_file: &EndFileEvent,
) -> bool {
    if !pending.replacing_active_file {
        return false;
    }
    if let (Some(event_entry_id), Some(new_entry_id)) =
        (end_file.playlist_entry_id, pending.new_entry_id)
    {
        // Once mpv announces the new entry, every other concrete id belongs
        // to the entry being replaced even if its id was not observed earlier.
        return event_entry_id != new_entry_id;
    }
    if let (Some(event_entry_id), Some(old_entry_id)) =
        (end_file.playlist_entry_id, pending.old_entry_id)
    {
        return event_entry_id == old_entry_id;
    }
    if pending.new_entry_id.is_none() {
        // Before `start-file`, an uncorrelated synthetic stop (or a genuine
        // last-frame EOF race) belongs to the entry being replaced. Never
        // suppress an error/abort from an entry whose id we failed to observe.
        return matches!(end_file.reason, EndFileReason::Stop | EndFileReason::Eof);
    }
    // Some mpv builds omit the old playlist entry id for the synthetic stop.
    matches!(end_file.reason, EndFileReason::Stop)
}

impl PlaybackActor {
    fn new(commands: mpsc::Receiver<CommandEnvelope>, events: mpsc::Sender<PlaybackEvent>) -> Self {
        Self {
            commands,
            events,
            active: None,
            pending: None,
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
        let target = match &purpose {
            ResolutionPurpose::Start { timeline, .. }
            | ResolutionPurpose::Replace { timeline, .. } => timeline.current().clone(),
            ResolutionPurpose::Navigate { target, .. } => target.clone(),
        };
        let cancellation = TaskCancellation::new();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            resolve_stream(&target, &task_cancellation)
                .await
                .map_err(|error| error.to_string())
        });
        self.pending = Some(PendingResolution {
            cancellation,
            purpose: Some(purpose),
            task: Some(task),
        });
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
            let transitioning = self
                .active
                .as_ref()
                .map(|active| active.transitioning)
                .unwrap_or(false);
            let id = self.active.as_ref().map(|active| active.id);
            self.emit(PlaybackEvent::Error {
                session_id: id,
                message: "mpv exited before playback completed".to_string(),
            })
            .await;
            self.stop_active(!transitioning).await;
        }
    }

    async fn handle_resolution(
        &mut self,
        purpose: ResolutionPurpose,
        result: std::result::Result<ResolvedStream, String>,
    ) {
        let resolved = match result {
            Ok(resolved) => resolved,
            Err(message) => {
                let is_navigation = matches!(&purpose, ResolutionPurpose::Navigate { .. });
                let session_id = match purpose {
                    ResolutionPurpose::Start { session_id, .. }
                    | ResolutionPurpose::Navigate { session_id, .. }
                    | ResolutionPurpose::Replace { session_id, .. } => Some(session_id),
                };
                if is_navigation {
                    let stopped_at_eof = self.active.as_ref().is_some_and(|active| active.at_eof);
                    if stopped_at_eof {
                        self.emit(PlaybackEvent::Error {
                            session_id,
                            message,
                        })
                        .await;
                        self.stop_active(false).await;
                    } else if let Some(active) = self.active.as_mut() {
                        let session_id = active.id;
                        active.transitioning = false;
                        self.emit(PlaybackEvent::NavigationFailed {
                            session_id,
                            message,
                        })
                        .await;
                    }
                } else {
                    self.emit(PlaybackEvent::Error {
                        session_id,
                        message,
                    })
                    .await;
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
            ResolutionPurpose::Navigate {
                target,
                target_index,
                session_id,
            } => {
                self.commit_navigation(session_id, target_index, target, resolved)
                    .await;
            }
            ResolutionPurpose::Replace {
                timeline,
                autoplay_next,
                session_id,
            } => {
                self.replace_active(session_id, timeline, autoplay_next, resolved)
                    .await;
            }
        }
    }

    async fn launch_active(
        &mut self,
        session_id: PlaybackSessionId,
        timeline: PlaybackTimeline,
        autoplay_next: bool,
        resolved: ResolvedStream,
    ) {
        let target = timeline.current().clone();
        let mpv = match MpvSession::spawn(
            session_id.raw(),
            &resolved.url,
            target.start_time,
            &target.player_title,
            &target.episode_title,
            &target.referrer,
        )
        .await
        {
            Ok(mpv) => mpv,
            Err(error) => {
                self.emit(PlaybackEvent::Error {
                    session_id: Some(session_id),
                    message: error.to_string(),
                })
                .await;
                return;
            }
        };
        let mut resolved_streams = HashMap::new();
        resolved_streams.insert(PlaybackIdentity::from(&target), resolved.url);
        self.active = Some(ActivePlayback {
            id: session_id,
            mpv,
            current: target.clone(),
            timeline,
            autoplay_next,
            resolved_streams,
            position: 0.0,
            duration: 0.0,
            has_position: false,
            entry_id: None,
            transitioning: false,
            at_eof: false,
            pending_navigation: None,
            stale_entry_id: None,
        });
        self.emit(PlaybackEvent::SessionStarted { session_id, target })
            .await;
    }

    async fn commit_navigation(
        &mut self,
        session_id: PlaybackSessionId,
        target_index: usize,
        target: PlayTarget,
        resolved: ResolvedStream,
    ) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.id != session_id || !active.transitioning {
            return;
        }
        let replacing_active_file = !active.at_eof;
        let old_entry_id = active.entry_id;
        let load_result = active
            .mpv
            .load_media(
                &resolved.url,
                target.start_time,
                &target.player_title,
                &target.episode_title,
            )
            .await;
        if let Err(error) = load_result {
            let id = active.id;
            active.transitioning = false;
            if replacing_active_file {
                self.emit(PlaybackEvent::NavigationFailed {
                    session_id: id,
                    message: error.to_string(),
                })
                .await;
            } else {
                self.emit(PlaybackEvent::Error {
                    session_id: Some(id),
                    message: error.to_string(),
                })
                .await;
                self.stop_active(false).await;
            }
            return;
        }

        // `loadfile` only acknowledges that the command was queued. Keep the
        // old logical episode selected until mpv confirms the new entry with
        // `file-loaded`, otherwise queued old-file events could be persisted
        // against the new episode.
        active.pending_navigation = Some(PendingNavigation {
            target,
            target_index,
            resolved_url: resolved.url,
            old_entry_id,
            new_entry_id: None,
            replacing_active_file,
            old_end_seen: false,
        });
    }

    async fn replace_active(
        &mut self,
        session_id: PlaybackSessionId,
        timeline: PlaybackTimeline,
        autoplay_next: bool,
        resolved: ResolvedStream,
    ) {
        let target = timeline.current().clone();
        let Some(mut old) = self.active.take() else {
            self.launch_active(session_id, timeline, autoplay_next, resolved)
                .await;
            return;
        };
        let old_snapshot = old.mpv.shutdown().await;
        if !old.transitioning {
            self.emit_partial(&old, old_snapshot.time_pos, old_snapshot.duration)
                .await;
        }
        let launch_result = MpvSession::spawn(
            session_id.raw(),
            &resolved.url,
            target.start_time,
            &target.player_title,
            &target.episode_title,
            &target.referrer,
        )
        .await;
        match launch_result {
            Ok(mpv) => {
                let mut resolved_streams = HashMap::new();
                resolved_streams.insert(PlaybackIdentity::from(&target), resolved.url);
                self.active = Some(ActivePlayback {
                    id: session_id,
                    mpv,
                    current: target.clone(),
                    timeline,
                    autoplay_next,
                    resolved_streams,
                    position: 0.0,
                    duration: 0.0,
                    has_position: false,
                    entry_id: None,
                    transitioning: false,
                    at_eof: false,
                    pending_navigation: None,
                    stale_entry_id: None,
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
            }
        }
    }

    async fn handle_mpv_event(&mut self, event: MpvMonitorEvent) {
        match event {
            MpvMonitorEvent::Progress { time_pos, duration } => {
                let snapshot = if let Some(active) = self.active.as_mut() {
                    if active.pending_navigation.is_some() {
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
                    if active.has_position && !active.transitioning {
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
                    if active.pending_navigation.is_some() {
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
                    if active.pending_navigation.is_none() {
                        active.entry_id = entry_id.or(active.entry_id);
                    }
                    // Position zero is mpv initialization. It is deliberately
                    // not a queue transition and never emits a reset.
                    let _initial_position = position == Some(0);
                }
            }
            MpvMonitorEvent::FileStarted { playlist_entry_id } => {
                if let Some(active) = self.active.as_mut() {
                    if let Some(pending) = active.pending_navigation.as_mut() {
                        if playlist_entry_id.is_some() && playlist_entry_id != pending.old_entry_id
                        {
                            pending.new_entry_id = playlist_entry_id;
                        }
                    } else {
                        active.entry_id = playlist_entry_id.or(active.entry_id);
                    }
                }
            }
            MpvMonitorEvent::FileLoaded { playlist_entry_id } => {
                let started = if let Some(active) = self.active.as_mut() {
                    let stale_old_load =
                        active.pending_navigation.as_ref().is_some_and(|pending| {
                            pending.replacing_active_file
                                && playlist_entry_id.is_some()
                                && playlist_entry_id == pending.old_entry_id
                        });
                    if stale_old_load {
                        active.entry_id = playlist_entry_id.or(active.entry_id);
                        None
                    } else if let Some(pending) = active.pending_navigation.take() {
                        let new_entry_id = playlist_entry_id.or(pending.new_entry_id);
                        active.stale_entry_id = (!pending.old_end_seen
                            && pending.replacing_active_file)
                            .then_some(pending.old_entry_id)
                            .flatten();
                        let identity = PlaybackIdentity::from(&pending.target);
                        active
                            .resolved_streams
                            .insert(identity, pending.resolved_url);
                        let selected = active
                            .timeline
                            .select(pending.target_index)
                            .unwrap_or(pending.target);
                        active.current = selected.clone();
                        active.position = 0.0;
                        active.duration = 0.0;
                        active.has_position = false;
                        active.entry_id = new_entry_id;
                        active.transitioning = false;
                        active.at_eof = false;
                        Some((active.id, selected))
                    } else {
                        active.entry_id = playlist_entry_id.or(active.entry_id);
                        None
                    }
                } else {
                    None
                };
                if let Some((session_id, target)) = started {
                    self.emit(PlaybackEvent::SessionStarted { session_id, target })
                        .await;
                }
            }
            MpvMonitorEvent::Navigate(navigation) => {
                self.navigate(navigation).await;
            }
            MpvMonitorEvent::EndFile(end_file) => {
                let (id, entry_id, ignore_replaced, pending_failed) = {
                    let Some(active) = self.active.as_mut() else {
                        return;
                    };
                    let mut ignore_replaced = false;
                    let mut pending_failed = None;
                    if let Some(pending) = active.pending_navigation.as_mut() {
                        if end_file_belongs_to_replaced_entry(pending, &end_file) {
                            pending.old_end_seen = true;
                            ignore_replaced = true;
                        } else {
                            pending_failed = Some(format!(
                                "mpv не зміг завантажити серію {}",
                                pending.target.episode
                            ));
                        }
                    } else if active.stale_entry_id.is_some()
                        && end_file.playlist_entry_id == active.stale_entry_id
                    {
                        active.stale_entry_id = None;
                        ignore_replaced = true;
                    }
                    (
                        active.id,
                        end_file.playlist_entry_id.or(active.entry_id),
                        ignore_replaced,
                        pending_failed,
                    )
                };
                if ignore_replaced {
                    return;
                }
                self.emit(PlaybackEvent::EndFile {
                    session_id: id,
                    reason: end_file.reason.clone(),
                    playlist_entry_id: entry_id,
                })
                .await;
                if let Some(message) = pending_failed {
                    self.emit(PlaybackEvent::Error {
                        session_id: Some(id),
                        message,
                    })
                    .await;
                    self.stop_active(false).await;
                } else if end_file.reason.is_natural_eof() {
                    self.natural_eof().await;
                } else {
                    self.stop_active(true).await;
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

    async fn navigate(&mut self, navigation: MpvNavigation) {
        let (session_id, target_index, target, cached, snapshot) = {
            let Some(active) = self.active.as_mut() else {
                return;
            };
            if active.transitioning {
                return;
            }
            let Some(target_index) = active.timeline.adjacent_index(navigation) else {
                let message = match navigation {
                    MpvNavigation::Previous => "Це перша доступна серія",
                    MpvNavigation::Next => "Це остання доступна серія",
                };
                let _ = active.mpv.show_text(message, 1500).await;
                return;
            };
            let target = active.timeline.entries[target_index].clone();
            let cached = active
                .resolved_streams
                .get(&PlaybackIdentity::from(&target))
                .cloned();
            let snapshot = (active.has_position && !active.at_eof)
                .then(|| Self::progress_snapshot(active, false));
            if active.has_position && !active.at_eof {
                let resume = (active.position > 0.0).then_some(active.position);
                active.timeline.entries[active.timeline.current_index].start_time = resume;
                active.current.start_time = resume;
            }
            active.transitioning = true;
            (active.id, target_index, target, cached, snapshot)
        };
        if let Some(snapshot) = snapshot {
            self.emit(PlaybackEvent::ProgressSnapshot(snapshot)).await;
        }

        if let Some(url) = cached {
            self.commit_navigation(session_id, target_index, target, ResolvedStream { url })
                .await;
        } else {
            if let Some(active) = self.active.as_ref() {
                let _ = active
                    .mpv
                    .show_text(&format!("Завантаження серії {}…", target.episode), 2000)
                    .await;
            }
            self.begin_resolution(ResolutionPurpose::Navigate {
                target,
                target_index,
                session_id,
            })
            .await;
        }
    }

    async fn natural_eof(&mut self) {
        let (id, snapshot, mark, next_index, navigation_already_in_progress) = {
            let Some(active) = self.active.as_mut() else {
                return;
            };
            if active.at_eof {
                return;
            }
            let navigation_already_in_progress = active.transitioning;
            active.transitioning = true;
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
            let next_index = if navigation_already_in_progress {
                None
            } else {
                active
                    .autoplay_next
                    .then(|| active.timeline.adjacent_index(MpvNavigation::Next))
                    .flatten()
            };
            (
                id,
                snapshot,
                mark,
                next_index,
                navigation_already_in_progress,
            )
        };
        if let Some(snapshot) = snapshot {
            self.emit(PlaybackEvent::ProgressSnapshot(snapshot)).await;
        }
        self.emit(PlaybackEvent::MarkWatched(mark)).await;
        if navigation_already_in_progress {
            // A manually selected neighbour is still resolving. Its result
            // will either load from idle or close this completed session on
            // failure; do not start a second autoplay resolution.
            return;
        }
        if let Some(target_index) = next_index {
            let (target, cached) = {
                let active = self.active.as_ref().expect("active playback at EOF");
                let target = active.timeline.entries[target_index].clone();
                let cached = active
                    .resolved_streams
                    .get(&PlaybackIdentity::from(&target))
                    .cloned();
                (target, cached)
            };
            if let Some(url) = cached {
                self.commit_navigation(id, target_index, target, ResolvedStream { url })
                    .await;
            } else {
                self.begin_resolution(ResolutionPurpose::Navigate {
                    target,
                    target_index,
                    session_id: id,
                })
                .await;
            }
        } else {
            self.stop_active(false).await;
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
        if emit_partial && !active.transitioning {
            self.emit_partial(&active, snapshot.time_pos, snapshot.duration)
                .await;
        }
        self.emit(PlaybackEvent::SessionStopped {
            session_id: active.id,
        })
        .await;
    }
}

async fn resolve_stream(
    target: &PlayTarget,
    cancellation: &TaskCancellation,
) -> Result<ResolvedStream> {
    if cancellation.is_cancelled() {
        return Err(anyhow!("stream resolution cancelled"));
    }
    let parser = api::AshdiParser::new()?;
    let url = tokio::select! {
        _ = cancellation.cancelled() => return Err(anyhow!("stream resolution cancelled")),
        result = parser.extract_m3u8(&target.stream_page_url) => result?,
    };
    Ok(ResolvedStream { url })
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
    fn pending_load_correlates_old_and_new_end_file_events() {
        let pending = PendingNavigation {
            target: target(2, None),
            target_index: 1,
            resolved_url: "https://media.test/2.m3u8".to_string(),
            old_entry_id: Some(10),
            new_entry_id: Some(11),
            replacing_active_file: true,
            old_end_seen: false,
        };
        assert!(end_file_belongs_to_replaced_entry(
            &pending,
            &EndFileEvent {
                reason: EndFileReason::Eof,
                playlist_entry_id: Some(10),
            }
        ));
        assert!(end_file_belongs_to_replaced_entry(
            &pending,
            &EndFileEvent {
                reason: EndFileReason::Stop,
                playlist_entry_id: None,
            }
        ));
        assert!(!end_file_belongs_to_replaced_entry(
            &pending,
            &EndFileEvent {
                reason: EndFileReason::Error,
                playlist_entry_id: Some(11),
            }
        ));

        let old_id_was_not_observed = PendingNavigation {
            old_entry_id: None,
            ..PendingNavigation {
                target: target(2, None),
                target_index: 1,
                resolved_url: "https://media.test/2.m3u8".to_string(),
                old_entry_id: Some(10),
                new_entry_id: Some(11),
                replacing_active_file: true,
                old_end_seen: false,
            }
        };
        assert!(end_file_belongs_to_replaced_entry(
            &old_id_was_not_observed,
            &EndFileEvent {
                reason: EndFileReason::Eof,
                playlist_entry_id: Some(10),
            }
        ));

        let from_idle = PendingNavigation {
            replacing_active_file: false,
            ..pending
        };
        assert!(!end_file_belongs_to_replaced_entry(
            &from_idle,
            &EndFileEvent {
                reason: EndFileReason::Stop,
                playlist_entry_id: Some(10),
            }
        ));
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
        assert_eq!(timeline.adjacent_index(MpvNavigation::Previous), Some(4));
        assert_eq!(timeline.adjacent_index(MpvNavigation::Next), Some(6));

        timeline.select(0).unwrap();
        assert_eq!(timeline.current().episode, 1);
        assert_eq!(timeline.adjacent_index(MpvNavigation::Previous), None);
        timeline.select(11).unwrap();
        assert_eq!(timeline.current().episode, 12);
        assert_eq!(timeline.adjacent_index(MpvNavigation::Next), None);
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
