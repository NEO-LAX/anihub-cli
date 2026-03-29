use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(windows)]
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep, timeout};

pub struct MpvPlayer {
    socket_path: PathBuf,
}

impl MpvPlayer {
    pub fn new() -> Result<Self> {
        #[cfg(unix)]
        {
            let proj_dirs = ProjectDirs::from("com", "shadowgarden", "anihub-cli")
                .context("Failed to determine project directories")?;
            let socket_path = proj_dirs.data_dir().join("mpv_ipc.sock");
            Ok(Self { socket_path })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                socket_path: PathBuf::from(r"\\.\pipe\anihub_mpv"),
            })
        }
    }

    pub async fn start(
        &self,
        m3u8_url: &str,
        start_time: Option<f64>,
        anime_title: &str,
        episode_title: &str,
    ) -> Result<(Child, JoinHandle<(f64, f64)>)> {
        #[cfg(unix)]
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }

        let mut args = vec![
            m3u8_url.to_string(),
            format!("--input-ipc-server={}", self.socket_path.display()),
            format!("--force-media-title={} - {}", anime_title, episode_title),
            "--referrer=https://ashdi.vip/".to_string(),
            "--force-window=yes".to_string(),
            "--no-terminal".to_string(),
            "--vo=gpu-next".to_string(),
        ];

        if let Some(t) = start_time {
            if t > 0.0 {
                args.push(format!("--start={}", t));
            }
        }

        let child = Command::new("mpv")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to start mpv. Please make sure mpv is installed.")?;

        let socket_path_clone = self.socket_path.clone();
        let monitor = tokio::spawn(async move { monitor_ipc(socket_path_clone).await });

        Ok((child, monitor))
    }

    pub fn cleanup(&self) {
        #[cfg(unix)]
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}

async fn monitor_ipc(socket_path: PathBuf) -> (f64, f64) {
    let mut last_known_time = 0.0f64;
    let mut last_known_duration = 0.0f64;

    // Wait for MPV to create the socket/pipe
    for _ in 0..50 {
        #[cfg(unix)]
        if socket_path.exists() {
            break;
        }

        #[cfg(windows)]
        {
            // On Windows, checking existence of a pipe is different,
            // but we can just try to connect.
            break;
        }

        sleep(Duration::from_millis(100)).await;
    }

    #[cfg(unix)]
    let Ok(mut stream) = UnixStream::connect(&socket_path).await else {
        return (last_known_time, last_known_duration);
    };

    #[cfg(windows)]
    let Ok(mut stream) = ClientOptions::new().open(&socket_path) else {
        return (last_known_time, last_known_duration);
    };

    for request in [
        "{\"command\": [\"observe_property\", 1, \"time-pos\"]}\n",
        "{\"command\": [\"observe_property\", 2, \"duration\"]}\n",
    ] {
        if stream.write_all(request.as_bytes()).await.is_err() {
            return (last_known_time, last_known_duration);
        }
    }

    let mut accumulated = String::new();
    let mut buf = vec![0u8; 1024];

    loop {
        match timeout(Duration::from_secs(1), stream.read(&mut buf)).await {
            Ok(Ok(0)) => break, // EOF — MPV closed
            Ok(Ok(n)) => {
                accumulated.push_str(&String::from_utf8_lossy(&buf[..n]));
                while let Some(pos) = accumulated.find('\n') {
                    let line = accumulated[..pos].to_string();
                    accumulated = accumulated[pos + 1..].to_string();
                    if let Ok(parsed) = serde_json::from_str::<Value>(&line) {
                        if parsed["event"] == "property-change" && parsed["name"] == "time-pos" {
                            if let Some(t) = parsed["data"].as_f64() {
                                last_known_time = t;
                            }
                        } else if parsed["event"] == "property-change"
                            && parsed["name"] == "duration"
                        {
                            if let Some(d) = parsed["data"].as_f64() {
                                last_known_duration = d;
                            }
                        }
                    }
                }
            }
            Ok(Err(_)) => break, // IO error
            Err(_) => {}         // timeout — keep reading
        }
    }

    (last_known_time, last_known_duration)
}
