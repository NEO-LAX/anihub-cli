use discord_presence::{Client, models::rich_presence::ActivityType};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RETRY_DELAY: Duration = Duration::from_secs(5);
const APPLICATION_ID: u64 = 1_527_419_150_761_328_810;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresenceActivity {
    title: String,
    state: String,
    started_at: u64,
    poster_url: Option<String>,
}

impl PresenceActivity {
    pub fn watching(
        title: &str,
        season: u32,
        episode: u32,
        studio: &str,
        position: f64,
        poster_url: Option<String>,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let elapsed = position.max(0.0) as u64;
        Self {
            title: truncate(title, 120),
            state: truncate(&format!("Сезон {season} · Серія {episode} · {studio}"), 120),
            started_at: now.saturating_sub(elapsed),
            poster_url,
        }
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
        }
        was_ready = ready;
        if !ready || sent == desired || Instant::now() < next_attempt {
            continue;
        }

        let result = match (&mut client, &desired) {
            (Some(client), Some(activity)) => set_activity(client, activity),
            (Some(client), None) => client.clear_activity().map(|_| ()),
            (None, _) => continue,
        };
        if result.is_ok() {
            sent.clone_from(&desired);
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
                .state(activity.state.clone())
                .timestamps(|timestamps| timestamps.start(activity.started_at));
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
            None,
        );
        assert!(activity.title.chars().count() <= 120);
        assert!(activity.state.chars().count() <= 120);
    }
}
