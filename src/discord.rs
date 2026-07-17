use discord_presence::{Client, models::rich_presence::ActivityType};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const APPLICATION_ID: u64 = 1_527_419_150_761_328_810;
const IMAGE_PROXY: &str = "https://wsrv.nl/";
const MAX_ACTIVITY_ASSET_BYTES: usize = 256;
/// How often the TUI should re-push watching progress so Discord stays aligned
/// after seeks and duration discovery. While playing, Discord animates the bar
/// between start/end without per-second updates.
pub const PRESENCE_SYNC_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresenceMedia {
    /// Playback cursor rounded down to whole seconds.
    pub position_secs: u64,
    /// Total length when known; enables the Spotify-style progress bar.
    pub duration_secs: Option<u64>,
    pub paused: bool,
}

impl PresenceMedia {
    pub fn from_playback(position: f64, duration: f64, paused: bool) -> Self {
        let position_secs = if position.is_finite() && position > 0.0 {
            position.floor() as u64
        } else {
            0
        };
        let duration_secs = if duration.is_finite() && duration > 0.0 {
            let duration_secs = duration.ceil() as u64;
            Some(duration_secs.max(position_secs.saturating_add(1)))
        } else {
            None
        };
        Self {
            position_secs,
            duration_secs,
            paused,
        }
    }

    /// Spotify-style progress while **playing**: `start = now - position`,
    /// optional `end = start + duration`.
    ///
    /// While **paused**, returns `None` so the activity has no timestamps —
    /// Discord would keep advancing any start/end pair on the wall clock, and
    /// SET_ACTIVITY is rate-limited (~15s), so a frozen bar is not practical.
    pub fn timestamps_at(&self, now_unix: u64) -> Option<(u64, Option<u64>)> {
        if self.paused {
            return None;
        }
        let start = now_unix.saturating_sub(self.position_secs);
        let end = self
            .duration_secs
            .map(|duration| start.saturating_add(duration));
        Some((start, end))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresenceActivity {
    title: String,
    state: String,
    poster_url: Option<String>,
    media: Option<PresenceMedia>,
}

impl PresenceActivity {
    pub fn idle() -> Self {
        Self {
            title: "Нічого не дивиться".to_string(),
            state: "AniHub CLI запущено".to_string(),
            poster_url: None,
            media: None,
        }
    }

    pub fn watching(
        title: &str,
        season: u32,
        episode: u32,
        studio: &str,
        poster_url: Option<String>,
        position: f64,
        duration: f64,
        paused: bool,
    ) -> Self {
        let media = PresenceMedia::from_playback(position, duration, paused);
        let state = if paused {
            format!("Сезон {season} · Серія {episode} · {studio} · Пауза")
        } else {
            format!("Сезон {season} · Серія {episode} · {studio}")
        };
        Self {
            title: truncate(title, 120),
            state: truncate(&state, 120),
            poster_url: poster_url.and_then(|url| square_poster_url(&url)),
            media: Some(media),
        }
    }

    fn without_poster(&self) -> Self {
        Self {
            title: self.title.clone(),
            state: self.state.clone(),
            poster_url: None,
            media: self.media.clone(),
        }
    }
}

enum Command {
    Configure(Option<u64>),
    Update(PresenceActivity),
    Shutdown,
}

/// Non-blocking facade around Discord's local IPC client.
///
/// All socket discovery, reconnects, and RPC calls stay on a dedicated worker
/// thread so a missing or restarting Discord client cannot stall the TUI.
pub struct DiscordPresence {
    commands: Sender<Command>,
    worker: Option<JoinHandle<()>>,
}

impl DiscordPresence {
    pub fn new(enabled: bool) -> Self {
        let (commands, receiver) = mpsc::channel();
        let worker = thread::spawn(move || run_worker(receiver));
        let presence = Self {
            commands,
            worker: Some(worker),
        };
        presence.configure(enabled);
        presence
    }

    pub fn configure(&self, enabled: bool) {
        let _ = self
            .commands
            .send(Command::Configure(enabled.then_some(APPLICATION_ID)));
    }

    pub fn update(&self, activity: PresenceActivity) {
        let _ = self.commands.send(Command::Update(activity));
    }

    pub fn clear(&self) {
        self.configure(false);
    }

    pub fn shutdown(mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for DiscordPresence {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.commands.send(Command::Shutdown);
        }
    }
}

fn run_worker(receiver: Receiver<Command>) {
    let mut configured_id = None;
    let mut client: Option<ManagedClient> = None;
    let mut desired: Option<PresenceActivity> = None;
    let mut queued: Option<PresenceActivity> = None;
    let mut queued_epoch = 0;
    let mut seen_rejection_epoch = 0;
    let mut fallback_without_poster = false;
    let started_at = unix_now();

    loop {
        let command = match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Command::Shutdown),
        };

        if let Some(command) = command {
            match command {
                Command::Configure(next_id) => {
                    if next_id.is_none() {
                        desired = None;
                    }
                    if next_id != configured_id {
                        stop_client(&mut client);
                        configured_id = next_id;
                        queued = None;
                        queued_epoch = 0;
                        seen_rejection_epoch = 0;
                        fallback_without_poster = false;
                        if let Some(id) = next_id {
                            client = Some(start_client(id));
                        }
                    }
                }
                Command::Update(activity) => {
                    if desired.as_ref() != Some(&activity) {
                        fallback_without_poster = false;
                    }
                    desired = Some(activity);
                }
                Command::Shutdown => {
                    stop_client(&mut client);
                    break;
                }
            }
        }

        // Collapse bursts of progress events into their newest state.
        while let Ok(command) = receiver.try_recv() {
            match command {
                Command::Configure(next_id) => {
                    if next_id.is_none() {
                        desired = None;
                    }
                    if next_id != configured_id {
                        stop_client(&mut client);
                        configured_id = next_id;
                        queued = None;
                        queued_epoch = 0;
                        seen_rejection_epoch = 0;
                        fallback_without_poster = false;
                        if let Some(id) = next_id {
                            client = Some(start_client(id));
                        }
                    }
                }
                Command::Update(activity) => {
                    if desired.as_ref() != Some(&activity) {
                        fallback_without_poster = false;
                    }
                    desired = Some(activity);
                }
                Command::Shutdown => {
                    stop_client(&mut client);
                    return;
                }
            }
        }

        if let Some(client) = &mut client {
            let rejection_epoch = client.rejection_epoch.load(Ordering::Relaxed);
            if rejection_epoch != seen_rejection_epoch {
                seen_rejection_epoch = rejection_epoch;
                queued = None;
                fallback_without_poster = true;
            }

            let Some(activity) = &desired else {
                continue;
            };
            let effective_activity = if fallback_without_poster {
                activity.without_poster()
            } else {
                activity.clone()
            };
            let connection_epoch = client.connection_epoch.load(Ordering::Relaxed);
            if should_queue(
                queued.as_ref(),
                &effective_activity,
                client.connected.load(Ordering::Relaxed),
                connection_epoch,
                queued_epoch,
            ) {
                queue_activity(&mut client.client, &effective_activity, started_at);
                queued = Some(effective_activity);
                queued_epoch = connection_epoch;
            }
        }
    }
}

fn should_queue(
    queued: Option<&PresenceActivity>,
    desired: &PresenceActivity,
    connected: bool,
    connection_epoch: u64,
    queued_epoch: u64,
) -> bool {
    queued != Some(desired) || (connected && connection_epoch != queued_epoch)
}

struct ManagedClient {
    client: Client,
    connected: Arc<AtomicBool>,
    connection_epoch: Arc<AtomicU64>,
    rejection_epoch: Arc<AtomicU64>,
}

fn start_client(application_id: u64) -> ManagedClient {
    let connected = Arc::new(AtomicBool::new(false));
    let connection_epoch = Arc::new(AtomicU64::new(0));
    let rejection_epoch = Arc::new(AtomicU64::new(0));
    let mut client = Client::with_error_config(application_id, Duration::from_secs(1), None);

    let connected_on_open = connected.clone();
    let epoch_on_open = connection_epoch.clone();
    client
        .on_connected(move |_| {
            connected_on_open.store(true, Ordering::Relaxed);
            epoch_on_open.fetch_add(1, Ordering::Relaxed);
        })
        .persist();
    let connected_on_close = connected.clone();
    client
        .on_disconnected(move |_| connected_on_close.store(false, Ordering::Relaxed))
        .persist();
    let rejection_on_error = rejection_epoch.clone();
    client
        .on_error(move |_| {
            rejection_on_error.fetch_add(1, Ordering::Relaxed);
        })
        .persist();
    client.start();

    ManagedClient {
        client,
        connected,
        connection_epoch,
        rejection_epoch,
    }
}

fn queue_activity(client: &mut Client, activity: &PresenceActivity, started_at: u64) {
    client.queue_activity(|builder| {
        let builder = builder
            .activity_type(ActivityType::Watching)
            .details(activity.title.clone())
            .state(activity.state.clone());
        let builder = match &activity.media {
            Some(media) => match media.timestamps_at(unix_now()) {
                Some((start, end)) => builder.timestamps(|timestamps| {
                    let timestamps = timestamps.start(start);
                    if let Some(end) = end {
                        timestamps.end(end)
                    } else {
                        timestamps
                    }
                }),
                // Paused: no timestamps → Discord drops the progress bar.
                None => builder,
            },
            // Idle keeps a simple session timer.
            None => builder.timestamps(|timestamps| timestamps.start(started_at)),
        };
        if let Some(poster_url) = &activity.poster_url {
            builder.assets(|assets| {
                assets
                    .large_image(poster_url.clone())
                    .large_text(activity.title.clone())
            })
        } else {
            builder
        }
    });
}

fn stop_client(client: &mut Option<ManagedClient>) {
    if let Some(client) = client.take() {
        let _ = client.client.shutdown();
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!(
            "{}…",
            prefix
                .chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        prefix
    }
}

fn square_poster_url(source: &str) -> Option<String> {
    let Ok(source_url) = reqwest::Url::parse(source) else {
        return None;
    };
    if !matches!(source_url.scheme(), "http" | "https") {
        return None;
    }

    let mut proxy = reqwest::Url::parse(IMAGE_PROXY).expect("static image proxy URL is valid");
    proxy
        .query_pairs_mut()
        .append_pair("url", source_url.as_str())
        .append_pair("w", "1024")
        .append_pair("h", "1024")
        .append_pair("fit", "cover")
        .append_pair("a", "attention");
    let proxy = String::from(proxy);
    if proxy.len() <= MAX_ACTIVITY_ASSET_BYTES {
        Some(proxy)
    } else if source.len() <= MAX_ACTIVITY_ASSET_BYTES {
        // Preserve the activity even when a very long proxy URL would make
        // Discord reject the complete payload. The uncropped source is still
        // preferable to losing the watching status altogether.
        Some(source.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discord_is_opt_in_and_uses_the_anihub_application() {
        assert_eq!(false.then_some(APPLICATION_ID), None);
        assert_eq!(true.then_some(APPLICATION_ID), Some(APPLICATION_ID));
    }

    #[test]
    fn activity_strings_are_bounded_without_breaking_unicode() {
        let activity = PresenceActivity::watching(
            &"Каґуя".repeat(40),
            2,
            4,
            &"Озвучка".repeat(40),
            None,
            0.0,
            0.0,
            false,
        );
        assert!(activity.title.chars().count() <= 120);
        assert!(activity.state.chars().count() <= 120);
    }

    #[test]
    fn idle_and_watching_states_are_stable() {
        let idle = PresenceActivity::idle();
        assert_eq!(idle.title, "Нічого не дивиться");
        assert_eq!(idle.state, "AniHub CLI запущено");
        assert_eq!(idle.media, None);

        let watching = PresenceActivity::watching("Каґуя", 2, 4, "Dzuski", None, 17.4, 142.0, false);
        assert_eq!(watching.title, "Каґуя");
        assert_eq!(watching.state, "Сезон 2 · Серія 4 · Dzuski");
        assert_eq!(
            watching.media,
            Some(PresenceMedia {
                position_secs: 17,
                duration_secs: Some(142),
                paused: false,
            })
        );

        let paused = PresenceActivity::watching("Каґуя", 2, 4, "Dzuski", None, 17.4, 142.0, true);
        assert_eq!(paused.state, "Сезон 2 · Серія 4 · Dzuski · Пауза");
        assert!(paused.media.as_ref().is_some_and(|media| media.paused));
        assert_eq!(paused.media.as_ref().unwrap().timestamps_at(1_000_000), None);
    }

    #[test]
    fn media_timestamps_anchor_progress_like_spotify_only_while_playing() {
        let media = PresenceMedia::from_playback(17.4, 142.2, false);
        let (start, end) = media.timestamps_at(1_000_000).expect("playing has timestamps");
        assert_eq!(start, 1_000_000 - 17);
        assert_eq!(end, Some(1_000_000 - 17 + 143));

        let unknown_duration = PresenceMedia::from_playback(8.0, 0.0, false);
        let (start, end) = unknown_duration
            .timestamps_at(500)
            .expect("playing has timestamps");
        assert_eq!(start, 492);
        assert_eq!(end, None);

        let paused = PresenceMedia::from_playback(17.4, 142.2, true);
        assert_eq!(paused.timestamps_at(1_000_000), None);
    }

    #[test]
    fn reconnect_requeues_the_last_activity_without_progress_churn() {
        let activity = PresenceActivity::watching("Каґуя", 2, 4, "Dzuski", None, 0.0, 0.0, false);
        assert!(should_queue(None, &activity, false, 0, 0));
        assert!(!should_queue(Some(&activity), &activity, true, 1, 1));
        assert!(should_queue(Some(&activity), &activity, true, 2, 1));

        let advanced =
            PresenceActivity::watching("Каґуя", 2, 4, "Dzuski", None, 30.0, 142.0, false);
        assert!(should_queue(Some(&activity), &advanced, true, 1, 1));
    }

    #[test]
    fn discord_posters_are_square_attention_crops_with_encoded_sources() {
        let source = "https://s4.anilist.co/poster.jpg?large=1&lang=uk";
        let transformed = square_poster_url(source).unwrap();
        let url = reqwest::Url::parse(&transformed).unwrap();
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(url.host_str(), Some("wsrv.nl"));
        assert_eq!(query.get("url").map(|value| value.as_ref()), Some(source));
        assert_eq!(query.get("w").map(|value| value.as_ref()), Some("1024"));
        assert_eq!(query.get("h").map(|value| value.as_ref()), Some("1024"));
        assert_eq!(query.get("fit").map(|value| value.as_ref()), Some("cover"));
        assert_eq!(
            query.get("a").map(|value| value.as_ref()),
            Some("attention")
        );
        assert!(transformed.len() <= MAX_ACTIVITY_ASSET_BYTES);
        assert_eq!(square_poster_url("not a URL"), None);
    }

    #[test]
    fn long_anihub_poster_keeps_the_rpc_asset_within_the_safe_limit() {
        let source = "https://cdn.anihub.in.ua/file/media-bucket-anihub/media/anime/posters/4802/friren-shcho-provodzhaye-v-ostanniu-put-medium_640-cc07f01b539252830d588ee3f394200f.webp";
        let transformed = square_poster_url(source).unwrap();
        assert!(transformed.len() <= MAX_ACTIVITY_ASSET_BYTES);
        assert!(transformed.contains("fit=cover"));

        let proxy_too_long = format!("https://example.com/{}.jpg", "a".repeat(220));
        assert_eq!(
            square_poster_url(&proxy_too_long).as_deref(),
            Some(proxy_too_long.as_str())
        );

        let source_too_long = format!("https://example.com/{}.jpg", "a".repeat(300));
        assert_eq!(square_poster_url(&source_too_long), None);
    }

    #[test]
    fn rejected_poster_can_fall_back_without_losing_watching_state() {
        let activity = PresenceActivity::watching(
            "Фрірен",
            1,
            1,
            "Amanogawa",
            Some("https://example.com/poster.jpg".to_string()),
            12.0,
            1400.0,
            false,
        );
        let fallback = activity.without_poster();
        assert_eq!(fallback.title, activity.title);
        assert_eq!(fallback.state, activity.state);
        assert_eq!(fallback.media, activity.media);
        assert_eq!(fallback.poster_url, None);
        assert!(should_queue(Some(&activity), &fallback, true, 1, 1));
    }
}
