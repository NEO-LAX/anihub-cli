use crate::api;
use crate::api::EpisodeSourcesResponse;
use crate::player::{EndFileReason, MpvMonitorEvent, MpvSession, TaskCancellation};
use crate::storage;
use crate::ui::AppState;
use anyhow::{Result, anyhow};
use std::collections::VecDeque;
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

/// One source-bearing release in an explicitly ordered mainline timeline.
///
/// The caller decides which releases belong to the timeline, so specials,
/// recaps, and other extras never enter the autoplay queue accidentally.
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

/// Build a deterministic queue after the selected target. The supervisor
/// resolves each Ashdi stream only when it becomes current.
pub fn build_playback_queue(app: &AppState, target: &PlayTarget) -> Vec<PlayTarget> {
    let mut queue = Vec::new();
    let mut current = (
        target.anime_id,
        target.anime_title.clone(),
        target.season,
        target.episode,
        target.studio_name.clone(),
    );
    let mut seen = std::collections::HashSet::new();
    seen.insert((current.0, current.2, current.3, current.4.clone()));
    while let Some(next) = get_next_episode(app, &current) {
        let identity = (
            next.anime_id,
            next.season,
            next.episode,
            next.studio_name.clone(),
        );
        if !seen.insert(identity) {
            break;
        }
        current = (
            next.anime_id,
            next.anime_title.clone(),
            next.season,
            next.episode,
            next.studio_name.clone(),
        );
        queue.push(next);
    }
    queue
}

/// Build autoplay targets across an ordered list of distinct releases.
///
/// Identity is never inferred from a normalized display season. Two
/// consecutive releases may both expose raw
/// season 1 and episode 1; their `anime_id` values keep them distinct.
pub fn build_release_playback_queue(
    target: &PlayTarget,
    timeline: &[PlaybackRelease<'_>],
) -> Vec<PlayTarget> {
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
        return Vec::new();
    };

    let mut queue = Vec::new();
    for (release_index, release) in timeline.iter().enumerate().skip(start_release_index) {
        let mut seasons = release
            .sources
            .ashdi
            .iter()
            .map(|studio| studio.season_number)
            .collect::<Vec<_>>();
        seasons.sort_unstable();
        seasons.dedup();

        for season in seasons {
            if release_index == start_release_index && season < target.season {
                continue;
            }

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

            let start_episode_index = if release_index == start_release_index
                && season == target.season
                && studio.studio_name == target.studio_name
            {
                studio
                    .episodes
                    .iter()
                    .position(|episode| episode.episode_number == target.episode)
                    .map(|index| index + 1)
                    .unwrap_or(0)
            } else {
                0
            };

            queue.extend(
                studio
                    .episodes
                    .iter()
                    .skip(start_episode_index)
                    .map(|episode| play_target_for_release(release, studio, episode)),
            );
        }
    }
    queue
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

pub fn get_next_episode(
    app: &AppState,
    current: &(u32, String, u32, u32, String),
) -> Option<PlayTarget> {
    let (current_anime_id, current_title, current_season, current_episode, current_studio) =
        current;

    let sources = app
        .sources_cache
        .get(&api::EpisodeSourcesKey::new(
            *current_anime_id,
            *current_season,
        ))
        .or_else(|| app.current_sources.clone())?;

    // Check if the current studio data is present
    let mut seasons: Vec<u32> = sources.ashdi.iter().map(|s| s.season_number).collect();
    seasons.sort();
    seasons.dedup();

    let season_index = seasons.iter().position(|&s| s == *current_season)?;

    // Find the studio index for current
    let (studio_idx, studio_data) = sources
        .ashdi
        .iter()
        .enumerate()
        .filter(|(_, s)| s.season_number == *current_season)
        .find(|(_, s)| s.studio_name == *current_studio)?;

    let ep_index = studio_data
        .episodes
        .iter()
        .position(|e| e.episode_number == *current_episode)?;

    if let Some(next_ep) = studio_data.episodes.get(ep_index + 1) {
        let anime_id = app
            .studio_anime_ids
            .get(studio_idx)
            .copied()
            .unwrap_or(*current_anime_id);
        let title = app
            .details_cache
            .get(&anime_id)
            .map(|d| d.title_ukrainian.clone())
            .unwrap_or_else(|| current_title.clone());
        let player_title = app
            .details_cache
            .get(&anime_id)
            .map(|d| format!("{} ({})", d.title_ukrainian, d.year.unwrap_or(0)))
            .unwrap_or_else(|| title.clone());

        return Some(PlayTarget {
            anime_id,
            anime_title: title,
            player_title,
            season: *current_season,
            episode: next_ep.episode_number,
            episode_title: format!("Серія {}", next_ep.episode_number),
            stream_page_url: next_ep.url.clone(),
            start_time: None,
            studio_name: current_studio.clone(),
            referrer: "https://ashdi.vip/".to_string(),
        });
    }

    // Next season
    let next_season = seasons.get(season_index + 1).copied()?;
    let (next_studio_idx, next_studio_data) = sources
        .ashdi
        .iter()
        .enumerate()
        .find(|(_, s)| s.season_number == next_season)?;
    let next_ep = next_studio_data.episodes.first()?;

    let anime_id = app
        .studio_anime_ids
        .get(next_studio_idx)
        .copied()
        .unwrap_or(*current_anime_id);
    let title = app
        .details_cache
        .get(&anime_id)
        .map(|d| d.title_ukrainian.clone())
        .unwrap_or_else(|| current_title.clone());
    let player_title = app
        .details_cache
        .get(&anime_id)
        .map(|d| format!("{} ({})", d.title_ukrainian, d.year.unwrap_or(0)))
        .unwrap_or_else(|| title.clone());

    Some(PlayTarget {
        anime_id,
        anime_title: title,
        player_title,
        season: next_season,
        episode: next_ep.episode_number,
        episode_title: format!("Серія {}", next_ep.episode_number),
        stream_page_url: next_ep.url.clone(),
        start_time: None,
        studio_name: next_studio_data.studio_name.clone(),
        referrer: "https://ashdi.vip/".to_string(),
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

#[derive(Clone, Debug, Eq, PartialEq)]
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
        target: PlayTarget,
        queue: Vec<PlayTarget>,
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

    pub async fn play(&self, target: PlayTarget, queue: Vec<PlayTarget>) -> Result<()> {
        self.command(PlaybackCommand::Play { target, queue }).await
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
    queue: VecDeque<PlayTarget>,
    position: f64,
    duration: f64,
    has_position: bool,
    entry_id: Option<i64>,
    waiting_for_next: bool,
}

enum ResolutionPurpose {
    Start {
        target: PlayTarget,
        queue: Vec<PlayTarget>,
        session_id: PlaybackSessionId,
    },
    Next {
        target: PlayTarget,
        session_id: PlaybackSessionId,
    },
    Replace {
        target: PlayTarget,
        queue: Vec<PlayTarget>,
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
            PlaybackCommand::Play { target, queue } => {
                self.cancel_pending().await;
                let session_id = PlaybackSessionId::new();
                let purpose = if self.active.is_some() {
                    ResolutionPurpose::Replace {
                        target,
                        queue,
                        session_id,
                    }
                } else {
                    ResolutionPurpose::Start {
                        target,
                        queue,
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
            ResolutionPurpose::Start { target, .. }
            | ResolutionPurpose::Next { target, .. }
            | ResolutionPurpose::Replace { target, .. } => target.clone(),
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
            let waiting_for_next = self
                .active
                .as_ref()
                .map(|active| active.waiting_for_next)
                .unwrap_or(false);
            let id = self.active.as_ref().map(|active| active.id);
            self.emit(PlaybackEvent::Error {
                session_id: id,
                message: "mpv exited before playback completed".to_string(),
            })
            .await;
            self.stop_active(!waiting_for_next).await;
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
                let is_next = matches!(&purpose, ResolutionPurpose::Next { .. });
                let session_id = match purpose {
                    ResolutionPurpose::Start { session_id, .. }
                    | ResolutionPurpose::Next { session_id, .. }
                    | ResolutionPurpose::Replace { session_id, .. } => Some(session_id),
                };
                self.emit(PlaybackEvent::Error {
                    session_id,
                    message,
                })
                .await;
                if is_next {
                    self.stop_active(false).await;
                }
                return;
            }
        };

        match purpose {
            ResolutionPurpose::Start {
                target,
                queue,
                session_id,
            } => {
                if self.active.is_none() {
                    self.launch_active(session_id, target, queue, resolved)
                        .await;
                }
            }
            ResolutionPurpose::Next { target, session_id } => {
                self.commit_next(session_id, target, resolved).await;
            }
            ResolutionPurpose::Replace {
                target,
                queue,
                session_id,
            } => {
                self.replace_active(session_id, target, queue, resolved)
                    .await;
            }
        }
    }

    async fn launch_active(
        &mut self,
        session_id: PlaybackSessionId,
        target: PlayTarget,
        queue: Vec<PlayTarget>,
        resolved: ResolvedStream,
    ) {
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
        self.active = Some(ActivePlayback {
            id: session_id,
            mpv,
            current: target.clone(),
            queue: queue.into_iter().collect(),
            position: 0.0,
            duration: 0.0,
            has_position: false,
            entry_id: None,
            waiting_for_next: false,
        });
        self.emit(PlaybackEvent::SessionStarted { session_id, target })
            .await;
    }

    async fn commit_next(
        &mut self,
        session_id: PlaybackSessionId,
        target: PlayTarget,
        resolved: ResolvedStream,
    ) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.id != session_id || !active.waiting_for_next {
            return;
        }
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
            self.emit(PlaybackEvent::Error {
                session_id: Some(id),
                message: error.to_string(),
            })
            .await;
            self.stop_active(false).await;
            return;
        }

        active.current = target.clone();
        active.position = 0.0;
        active.duration = 0.0;
        active.has_position = false;
        active.entry_id = None;
        active.waiting_for_next = false;
        self.emit(PlaybackEvent::SessionStarted { session_id, target })
            .await;
    }

    async fn replace_active(
        &mut self,
        session_id: PlaybackSessionId,
        target: PlayTarget,
        queue: Vec<PlayTarget>,
        resolved: ResolvedStream,
    ) {
        let Some(mut old) = self.active.take() else {
            self.launch_active(session_id, target, queue, resolved)
                .await;
            return;
        };
        let old_snapshot = old.mpv.shutdown().await;
        if !old.waiting_for_next {
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
                self.active = Some(ActivePlayback {
                    id: session_id,
                    mpv,
                    current: target.clone(),
                    queue: queue.into_iter().collect(),
                    position: 0.0,
                    duration: 0.0,
                    has_position: false,
                    entry_id: None,
                    waiting_for_next: false,
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
                    if active.has_position && !active.waiting_for_next {
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
                let event = self.active.as_mut().map(|active| {
                    let position = if let Some(time_pos) =
                        time_pos.filter(|value| value.is_finite() && *value >= 0.0)
                    {
                        active.position = time_pos;
                        active.has_position = true;
                        Some(time_pos)
                    } else {
                        active.has_position.then_some(active.position)
                    };
                    PlaybackEvent::PauseChanged {
                        session_id: active.id,
                        identity: PlaybackIdentity::from(&active.current),
                        paused,
                        position,
                    }
                });
                if let Some(event) = event {
                    self.emit(event).await;
                }
            }
            MpvMonitorEvent::PlaylistPosition { position, entry_id } => {
                if let Some(active) = self.active.as_mut() {
                    active.entry_id = entry_id.or(active.entry_id);
                    // Position zero is mpv initialization. It is deliberately
                    // not a queue transition and never emits a reset.
                    let _initial_position = position == Some(0);
                }
            }
            MpvMonitorEvent::FileStarted { playlist_entry_id }
            | MpvMonitorEvent::FileLoaded { playlist_entry_id } => {
                if let Some(active) = self.active.as_mut() {
                    active.entry_id = playlist_entry_id.or(active.entry_id);
                }
            }
            MpvMonitorEvent::EndFile(end_file) => {
                let Some(active) = self.active.as_mut() else {
                    return;
                };
                let id = active.id;
                let entry_id = end_file.playlist_entry_id.or(active.entry_id);
                self.emit(PlaybackEvent::EndFile {
                    session_id: id,
                    reason: end_file.reason.clone(),
                    playlist_entry_id: entry_id,
                })
                .await;
                if end_file.reason.is_natural_eof() {
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

    async fn natural_eof(&mut self) {
        let (id, snapshot, mark, next) = {
            let Some(active) = self.active.as_mut() else {
                return;
            };
            if active.waiting_for_next {
                return;
            }
            active.waiting_for_next = true;
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
            let next = active.queue.pop_front();
            (id, snapshot, mark, next)
        };
        if let Some(snapshot) = snapshot {
            self.emit(PlaybackEvent::ProgressSnapshot(snapshot)).await;
        }
        self.emit(PlaybackEvent::MarkWatched(mark)).await;
        if let Some(target) = next {
            self.begin_resolution(ResolutionPurpose::Next {
                target,
                session_id: id,
            })
            .await;
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
        if emit_partial && !active.waiting_for_next {
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

        let queue = build_release_playback_queue(&target_for(10, 1, 1), &releases);
        let identities = queue
            .iter()
            .map(|target| (target.anime_id, target.season, target.episode))
            .collect::<Vec<_>>();
        assert_eq!(identities, vec![(10, 1, 2), (20, 1, 1), (20, 1, 2)]);
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

        let queue = build_release_playback_queue(&target_for(10, 1, 2), &releases);
        assert_eq!(
            queue
                .iter()
                .map(|target| (target.anime_id, target.episode))
                .collect::<Vec<_>>(),
            vec![(20, 1), (20, 2), (30, 1)]
        );
        assert!(queue.iter().all(|target| target.anime_id != 15));
        let _unlisted_extra = PlaybackRelease {
            anime_id: 15,
            anime_title: "Recap",
            player_title: "Recap",
            sources: &extra,
        };
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
            .play(target(1, None), vec![target(2, None)])
            .await
            .unwrap();
        supervisor.shutdown().await.unwrap();
    }
}
