//! Small, side-effect-free platform selection helpers used by playback.
//!
//! Keeping command selection here makes the process lifecycle code testable on
//! one host without pretending that every host has the same browser command.

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
    Other,
}

impl Platform {
    pub const fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            return Self::Linux;
        }
        #[cfg(target_os = "macos")]
        {
            return Self::MacOs;
        }
        #[cfg(target_os = "windows")]
        {
            return Self::Windows;
        }
        #[allow(unreachable_code)]
        Self::Other
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

pub fn browser_open_command(platform: Platform, url: &str) -> CommandSpec {
    match platform {
        Platform::Linux => CommandSpec::new("xdg-open", [url]),
        Platform::MacOs => CommandSpec::new("open", [url]),
        // The empty title is intentional: it prevents `start` from treating
        // the URL as a window title when it contains punctuation.
        Platform::Windows => CommandSpec::new("cmd", ["/C", "start", "", url]),
        Platform::Other => CommandSpec::new("xdg-open", [url]),
    }
}

pub fn path_open_command(platform: Platform, path: &str) -> CommandSpec {
    match platform {
        Platform::Linux => CommandSpec::new("xdg-open", [path]),
        Platform::MacOs => CommandSpec::new("open", [path]),
        Platform::Windows => CommandSpec::new("explorer", [path]),
        Platform::Other => CommandSpec::new("xdg-open", [path]),
    }
}

/// Best-effort process-tree termination used only after a graceful shutdown
/// has timed out. The direct child is still waited by the caller.
pub fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        // A process group is preferable when mpv was launched with one. The
        // fallback targets descendants explicitly and is harmless if the
        // utility is unavailable.
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &format!("-{pid}")])
            .status();
        let _ = std::process::Command::new("pkill")
            .args(["-TERM", "-P", &pid.to_string()])
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
    }
    #[cfg(not(any(unix, windows)))]
    let _ = pid;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_command_matches_each_supported_platform() {
        let url = "https://example.test/watch?a=1";
        assert_eq!(
            browser_open_command(Platform::Linux, url),
            CommandSpec::new("xdg-open", [url])
        );
        assert_eq!(
            browser_open_command(Platform::MacOs, url),
            CommandSpec::new("open", [url])
        );
        assert_eq!(
            browser_open_command(Platform::Windows, url),
            CommandSpec::new("cmd", ["/C", "start", "", url])
        );
    }

    #[test]
    fn path_command_matches_each_supported_platform() {
        assert_eq!(
            path_open_command(Platform::Linux, "/tmp/anihub"),
            CommandSpec::new("xdg-open", ["/tmp/anihub"])
        );
        assert_eq!(
            path_open_command(Platform::Windows, r"C:\anihub"),
            CommandSpec::new("explorer", [r"C:\anihub"])
        );
    }
}
