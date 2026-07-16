use discord_presence::{Client, models::rich_presence::ActivityType};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RETRY_DELAY: Duration = Duration::from_secs(5);
const SEEK_DEBOUNCE: Duration = Duration::from_secs(2);
const SEEK_DRIFT_SECONDS: f64 = 4.0;
const APPLICATION_ID: u64 = 1_527_419_150_761_328_810;

#[derive(Clone, Debug, PartialEq)]
pub struct PresenceActivity {
    title: String,
    season: u32,
    episode: u32,
    studio: String,
    position: f64,
    paused: bool,
    poster_url: Option<String>,
}

impl PresenceActivity {
    pub fn watching(
        title: &str,
        season: u32,
        episode: u32,
        studio: &str,
        position: f64,
        paused: bool,
        poster_url: Option<String>,
    ) -> Self {
        Self {
            title: truncate(title, 120),
            season,
            episode,
            studio: truncate(studio, 80),
            position: if position.is_finite() {
                position.max(0.0)
            } else {
                0.0
            },
            paused,
            poster_url,
        }
    }

    fn state(&self) -> String {
        if self.paused {
            truncate(
                &format!(
                    "Пауза · {} · {}",
                    format_position(self.position),
                    self.studio
                ),
                120,
            )
        } else {
            truncate(
                &format!(
                    "Сезон {} · Серія {} · {}",
                    self.season, self.episode, self.studio
                ),
                120,
            )
        }
    }

    fn same_media(&self, other: &Self) -> bool {
        self.title == other.title
            && self.season == other.season
            && self.episode == other.episode
            && self.studio == other.studio
            && self.poster_url == other.poster_url
    }
}

enum Command {
    Configure(Option<u64>),
    Update(PresenceActivity),
    Clear,
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
        let _ = self.commands.send(Command::Clear);
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
    let mut client: Option<Client> = None;
    let mut desired: Option<PresenceActivity> = None;
    let mut sent: Option<PresenceActivity> = None;
    let mut last_sent_at: Option<Instant> = None;
    let mut was_ready = false;
    let mut next_attempt = Instant::now();

    loop {
        let command = match receiver.recv_timeout(Duration::from_millis(500)) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Command::Shutdown),
        };

        if let Some(command) = command {
            match command {
                Command::Configure(next_id) => {
                    if next_id != configured_id {
                        stop_client(&mut client);
                        configured_id = next_id;
                        sent = None;
                        last_sent_at = None;
                        was_ready = false;
                        if let Some(id) = next_id {
                            let mut next =
                                Client::with_error_config(id, Duration::from_secs(1), None);
                            next.start();
                            client = Some(next);
                        }
                    }
                }
                Command::Update(activity) => desired = Some(activity),
                Command::Clear => desired = None,
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
                    if next_id != configured_id {
                        stop_client(&mut client);
                        configured_id = next_id;
                        sent = None;
                        last_sent_at = None;
                        was_ready = false;
                        if let Some(id) = next_id {
                            let mut next =
                                Client::with_error_config(id, Duration::from_secs(1), None);
                            next.start();
                            client = Some(next);
                        }
                    }
                }
                Command::Update(activity) => desired = Some(activity),
                Command::Clear => desired = None,
                Command::Shutdown => {
                    stop_client(&mut client);
                    return;
                }
            }
        }

        let ready = client.is_some() && Client::is_ready();
        if ready && !was_ready {
            // Discord may have restarted while the desired activity stayed
            // unchanged; resend it after each successful reconnect.
            sent = None;
            last_sent_at = None;
        }
        was_ready = ready;
        let now = Instant::now();
        if !ready
            || now < next_attempt
            || !presence_needs_update(sent.as_ref(), desired.as_ref(), last_sent_at, now)
        {
            continue;
        }

        let result = match (&mut client, &desired) {
            (Some(client), Some(activity)) => set_activity(client, activity),
            (Some(client), None) => client.clear_activity().map(|_| ()),
            (None, _) => continue,
        };
        if result.is_ok() {
            sent.clone_from(&desired);
            last_sent_at = Some(now);
        } else {
            next_attempt = Instant::now() + RETRY_DELAY;
        }
    }
}

fn set_activity(client: &mut Client, activity: &PresenceActivity) -> discord_presence::Result<()> {
    client
        .set_activity(|builder| {
            let builder = builder
                .activity_type(ActivityType::Watching)
                .details(activity.title.clone())
                .state(activity.state());
            let builder = if activity.paused {
                builder
            } else {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let started_at = now.saturating_sub(activity.position as u64);
                builder.timestamps(|timestamps| timestamps.start(started_at))
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
        })
        .map(|_| ())
}

fn presence_needs_update(
    sent: Option<&PresenceActivity>,
    desired: Option<&PresenceActivity>,
    last_sent_at: Option<Instant>,
    now: Instant,
) -> bool {
    let (Some(sent), Some(desired)) = (sent, desired) else {
        return sent.is_some() != desired.is_some();
    };
    if !sent.same_media(desired) || sent.paused != desired.paused {
        return true;
    }

    let elapsed = last_sent_at
        .map(|sent_at| now.saturating_duration_since(sent_at))
        .unwrap_or(SEEK_DEBOUNCE);
    if elapsed < SEEK_DEBOUNCE {
        return false;
    }
    let expected_position = if sent.paused {
        sent.position
    } else {
        sent.position + elapsed.as_secs_f64()
    };
    (desired.position - expected_position).abs() >= SEEK_DRIFT_SECONDS
}

fn stop_client(client: &mut Option<Client>) {
    if Client::is_ready()
        && let Some(client) = client.as_mut()
    {
        let _ = client.clear_activity();
    }
    if let Some(client) = client.take() {
        let _ = client.shutdown();
    }
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

fn format_position(position: f64) -> String {
    let seconds = position.max(0.0) as u64;
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
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
            12.0,
            false,
            None,
        );
        assert!(activity.title.chars().count() <= 120);
        assert!(activity.state().chars().count() <= 120);
    }

    #[test]
    fn normal_progress_does_not_resend_but_seeks_do_after_debounce() {
        let sent = PresenceActivity::watching("Каґуя", 2, 4, "Dzuski", 10.0, false, None);
        let normal = PresenceActivity::watching("Каґуя", 2, 4, "Dzuski", 12.0, false, None);
        let seek = PresenceActivity::watching("Каґуя", 2, 4, "Dzuski", 90.0, false, None);
        let started = Instant::now();

        assert!(!presence_needs_update(
            Some(&sent),
            Some(&normal),
            Some(started),
            started + Duration::from_secs(2),
        ));
        assert!(!presence_needs_update(
            Some(&sent),
            Some(&seek),
            Some(started),
            started + Duration::from_secs(1),
        ));
        assert!(presence_needs_update(
            Some(&sent),
            Some(&seek),
            Some(started),
            started + Duration::from_secs(2),
        ));
    }

    #[test]
    fn pause_changes_are_immediate_and_show_a_frozen_position() {
        let playing = PresenceActivity::watching("Каґуя", 2, 4, "Dzuski", 754.0, false, None);
        let paused = PresenceActivity::watching("Каґуя", 2, 4, "Dzuski", 754.0, true, None);
        let now = Instant::now();

        assert!(presence_needs_update(
            Some(&playing),
            Some(&paused),
            Some(now),
            now,
        ));
        assert_eq!(paused.state(), "Пауза · 12:34 · Dzuski");
        assert_eq!(format_position(3723.0), "1:02:03");
    }
}
