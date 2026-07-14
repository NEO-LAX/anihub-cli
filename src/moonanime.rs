#![allow(dead_code)]

use crate::debug_log;
use crate::platform;
use crate::player::TaskCancellation;
use anyhow::{Context, Result, anyhow, bail};
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};

const RESOLUTION_TIMEOUT: Duration = Duration::from_secs(40);
const MAX_OUTPUT_LINE: usize = 16 * 1024;
const MAX_DIAGNOSTICS: usize = 24 * 1024;
static NEXT_SCRIPT_ID: AtomicU64 = AtomicU64::new(1);

/// The Python process owns a local HLS server for HLS streams. Its stdin is
/// deliberately kept open: EOF is the lifecycle signal that closes the
/// server and lets the child exit cleanly.
pub struct MoonAnimeProcess {
    child: Option<Child>,
    script_path: PathBuf,
    diagnostics: Option<JoinHandle<String>>,
    reaped: bool,
    released: bool,
}

pub struct MoonAnimeResolution {
    pub url: String,
    pub process: Option<MoonAnimeProcess>,
}

impl MoonAnimeProcess {
    pub fn is_reaped(&self) -> bool {
        self.reaped
    }

    /// Gracefully closes the Python server, then uses bounded forced cleanup.
    pub async fn shutdown(mut self) -> String {
        let diagnostics = self.shutdown_child().await;
        self.cleanup_script();
        diagnostics
    }

    fn cleanup_script(&self) {
        let _ = std::fs::remove_file(&self.script_path);
    }

    async fn shutdown_child(&mut self) -> String {
        let Some(mut child) = self.child.take() else {
            return self.take_diagnostics().await;
        };

        // Dropping stdin is the normal script shutdown protocol.
        let _ = child.stdin.take();
        if !self.reaped {
            let mut exited = false;
            if let Ok(Ok(_)) = timeout(Duration::from_secs(2), child.wait()).await {
                exited = true;
                self.reaped = true;
            }
            if !exited {
                if let Some(pid) = child.id() {
                    platform::kill_process_tree(pid);
                }
                let _ = child.kill().await;
                let _ = timeout(Duration::from_secs(2), child.wait()).await;
                self.reaped = true;
            }
        }
        self.take_diagnostics().await
    }

    async fn take_diagnostics(&mut self) -> String {
        let Some(task) = self.diagnostics.take() else {
            return String::new();
        };
        timeout(Duration::from_millis(500), task)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
    }

    /// Compatibility bridge for the old AppState field. The supervisor uses
    /// `shutdown` and never releases this ownership.
    pub fn into_compat_child(mut self) -> Option<Child> {
        self.released = true;
        if let Some(task) = self.diagnostics.take() {
            task.abort();
        }
        self.cleanup_script_if_reaped();
        self.child.take()
    }

    fn cleanup_script_if_reaped(&self) {
        if self.reaped {
            self.cleanup_script();
        }
    }
}

impl Drop for MoonAnimeProcess {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.stdin.take();
            if !self.reaped {
                if let Some(pid) = child.id() {
                    platform::kill_process_tree(pid);
                }
                let _ = child.start_kill();
            }
        }
        if let Some(task) = self.diagnostics.take() {
            task.abort();
        }
        self.cleanup_script();
    }
}

/// Playwright script used to resolve both HLS and direct WebM MoonAnime
/// sources. HLS stays alive until stdin EOF and explicitly closes its HTTP
/// server; direct WebM prints the final URL and exits.
pub const MOONANIME_PLAYWRIGHT_SCRIPT: &str = r#"
import asyncio, os, re, socket, sys
from http.server import HTTPServer, BaseHTTPRequestHandler
from threading import Thread

_manifest = b""

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/vnd.apple.mpegurl")
        self.send_header("Content-Length", str(len(_manifest)))
        self.end_headers()
        self.wfile.write(_manifest)
    def log_message(self, *args):
        pass

async def js_fetch(page, url):
    return await page.evaluate("""async (url) => {
        const r = await fetch(url, {
            headers: {"Origin": "https://moonanime.art", "Referer": "https://moonanime.art/"}
        });
        return {status: r.status, text: await r.text()};
    }""", url)

async def stdin_eof():
    loop = asyncio.get_running_loop()
    await loop.run_in_executor(None, sys.stdin.buffer.read)

async def main():
    global _manifest
    from playwright.async_api import async_playwright
    iframe_url = sys.argv[1]
    async with async_playwright() as p:
        browser = await p.firefox.launch(headless=True)
        ctx = await browser.new_context(
            user_agent="Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0",
            extra_http_headers={
                "Accept-Language": "uk,en-US;q=0.9,en;q=0.8",
                "Accept": "text/html,application/xhtml+xml,*/*;q=0.8",
            },
        )
        page = await ctx.new_page()
        master_url = None
        webm_url = None
        async def on_req(req):
            nonlocal master_url, webm_url
            if "s.moonanime" in req.url and ".m3u8" in req.url and not master_url:
                master_url = req.url
            elif "s.moonanime" in req.url and "/content/v/" in req.url and not webm_url:
                webm_url = req.url
        page.on("request", on_req)
        try:
            await page.goto(iframe_url, wait_until="networkidle", timeout=20000)
            await asyncio.sleep(2)
            if not master_url:
                try:
                    await page.click("body")
                except Exception:
                    pass
                await asyncio.sleep(3)
        except Exception:
            pass

        if master_url:
            master = await js_fetch(page, master_url)
            if master["status"] != 200:
                await browser.close(); sys.exit(1)
            variants = re.findall(r'(https://s\.moonanime\.art/[^\s]+\.m3u8[^\s]*)', master["text"])
            if not variants:
                await browser.close(); sys.exit(1)
            best = next((v for v in variants if "1080" in v), next((v for v in variants if "720" in v), variants[0]))
            variant = await js_fetch(page, best)
            if variant["status"] != 200:
                await browser.close(); sys.exit(1)
            _manifest = variant["text"].encode("utf-8")
            await browser.close()
            s = socket.socket(); s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]; s.close()
            srv = HTTPServer(("127.0.0.1", port), Handler)
            thread = Thread(target=srv.serve_forever, daemon=True)
            thread.start()
            print(f"http://127.0.0.1:{port}/stream.m3u8", flush=True)
            await stdin_eof()
            srv.shutdown(); srv.server_close(); thread.join(timeout=2)
        elif webm_url:
            result = await page.evaluate("""async (url) => {
                try {
                    const r = await fetch(url, {redirect: "follow", headers: {Range: "bytes=0-0"}});
                    return {finalUrl: r.url, status: r.status};
                } catch(e) { return {error: e.toString()}; }
            }""", webm_url)
            await browser.close()
            final_url = result.get("finalUrl", "")
            if not final_url or result.get("status", 0) not in (200, 206):
                sys.exit(1)
            print(final_url, flush=True)
        else:
            await browser.close(); sys.exit(1)
    try:
        os.unlink(__file__)
    except Exception:
        pass

asyncio.run(main())
"#;

fn unique_script_path(session_id: u64) -> PathBuf {
    let nonce = NEXT_SCRIPT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "anihub-moon-{}-{session_id}-{nonce}.py",
        std::process::id()
    ))
}

fn write_unique_script(session_id: u64) -> Result<PathBuf> {
    let path = unique_script_path(session_id);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| {
            format!(
                "failed to create unique MoonAnime script {}",
                path.display()
            )
        })?;
    file.write_all(MOONANIME_PLAYWRIGHT_SCRIPT.as_bytes())?;
    file.flush()?;
    Ok(path)
}

async fn capture_diagnostics(mut stderr: tokio::process::ChildStderr) -> String {
    let mut output = Vec::new();
    let mut buffer = [0u8; 2048];
    while output.len() < MAX_DIAGNOSTICS {
        let read = match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let remaining = MAX_DIAGNOSTICS - output.len();
        output.extend_from_slice(&buffer[..read.min(remaining)]);
    }
    String::from_utf8_lossy(&output).into_owned()
}

async fn terminate_failed_child(
    mut child: Child,
    diagnostics: Option<JoinHandle<String>>,
    script_path: &PathBuf,
) -> String {
    let _ = child.stdin.take();
    if let Some(pid) = child.id() {
        platform::kill_process_tree(pid);
    }
    let _ = child.kill().await;
    let _ = timeout(Duration::from_secs(2), child.wait()).await;
    let diagnostics = match diagnostics {
        Some(task) => timeout(Duration::from_millis(500), task)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default(),
        None => String::new(),
    };
    let _ = std::fs::remove_file(script_path);
    diagnostics
}

async fn read_stream_url(
    mut reader: BufReader<tokio::process::ChildStdout>,
    cancel: &TaskCancellation,
) -> Result<String> {
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = tokio::select! {
            _ = cancel.cancelled() => bail!("MoonAnime resolution cancelled"),
            result = reader.read_line(&mut line) => result?,
        };
        if bytes == 0 {
            bail!("MoonAnime resolver exited without a stream URL");
        }
        if line.len() > MAX_OUTPUT_LINE {
            bail!("MoonAnime resolver output line is too large");
        }
        let candidate = line.trim();
        if candidate.starts_with("http://") || candidate.starts_with("https://") {
            return Ok(candidate.to_string());
        }
    }
}

/// Resolve a MoonAnime iframe into a stream while retaining the child only
/// when it is needed as an HLS proxy. Direct WebM children are waited/reaped
/// before the result is returned.
pub async fn resolve_moonanime_stream(
    iframe_url: &str,
    session_id: u64,
    cancel: &TaskCancellation,
) -> Result<MoonAnimeResolution> {
    let script_path = write_unique_script(session_id)?;
    let mut child = None;
    let mut spawn_errors = Vec::new();
    for candidate in platform::current_python_candidates() {
        match Command::new(&candidate.program)
            .args(&candidate.args)
            .arg(&script_path)
            .arg(iframe_url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(process) => {
                child = Some(process);
                break;
            }
            Err(error) => spawn_errors.push(format!("{}: {error}", candidate.program)),
        }
    }
    let mut child = match child {
        Some(child) => child,
        None => {
            let _ = std::fs::remove_file(&script_path);
            bail!("could not start Python: {}", spawn_errors.join("; "));
        }
    };

    let stdout = child
        .stdout
        .take()
        .context("MoonAnime resolver has no stdout")?;
    let diagnostics = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(capture_diagnostics(stderr)));
    let url_result = tokio::select! {
        _ = cancel.cancelled() => Err(anyhow!("MoonAnime resolution cancelled")),
        result = timeout(RESOLUTION_TIMEOUT, read_stream_url(BufReader::new(stdout), cancel)) => {
            result.map_err(|_| anyhow!("MoonAnime resolution timed out"))?
        }
    };

    let url = match url_result {
        Ok(url) => url,
        Err(error) => {
            let diagnostics = terminate_failed_child(child, diagnostics, &script_path).await;
            if !diagnostics.is_empty() {
                debug_log(&format!(
                    "[moonanime] resolver failed: {error}; {diagnostics}"
                ));
            }
            return Err(error);
        }
    };

    let direct_webm = !url.starts_with("http://127.0.0.1:");
    if direct_webm {
        let mut reaped = false;
        if let Ok(Ok(_)) = timeout(Duration::from_secs(2), child.wait()).await {
            reaped = true;
        }
        if !reaped {
            let diagnostics = terminate_failed_child(child, diagnostics, &script_path).await;
            if !diagnostics.is_empty() {
                debug_log(&format!(
                    "[moonanime] direct WebM child did not exit: {diagnostics}"
                ));
            }
            return Err(anyhow!("MoonAnime direct WebM resolver did not exit"));
        }
        let diagnostics = match diagnostics {
            Some(task) => timeout(Duration::from_millis(500), task)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default(),
            None => String::new(),
        };
        if !diagnostics.is_empty() {
            debug_log(&format!(
                "[moonanime] direct WebM diagnostics: {diagnostics}"
            ));
        }
        return Ok(MoonAnimeResolution {
            url,
            process: Some(MoonAnimeProcess {
                child: Some(child),
                script_path,
                diagnostics: None,
                reaped: true,
                released: false,
            }),
        });
    }

    debug_log(&format!("[moonanime] HLS proxy ready: {url}"));
    Ok(MoonAnimeResolution {
        url,
        process: Some(MoonAnimeProcess {
            child: Some(child),
            script_path,
            diagnostics,
            reaped: false,
            released: false,
        }),
    })
}

/// Compatibility wrapper retained for the current AppState. New code should
/// use `resolve_moonanime_stream` so the proxy remains supervisor-owned.
pub async fn try_moonanime_stream(iframe_url: &str) -> Option<(String, tokio::process::Child)> {
    let cancellation = TaskCancellation::new();
    let resolution = resolve_moonanime_stream(iframe_url, 0, &cancellation)
        .await
        .ok()?;
    let process = resolution.process?;
    Some((resolution.url, process.into_compat_child()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_is_not_fixed_to_a_shared_temp_name() {
        let first = unique_script_path(10);
        let second = unique_script_path(10);
        assert_ne!(first, second);
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("anihub-moon-")
        );
    }

    #[test]
    fn script_uses_stdin_eof_to_close_hls_server() {
        assert!(MOONANIME_PLAYWRIGHT_SCRIPT.contains("await stdin_eof()"));
        assert!(MOONANIME_PLAYWRIGHT_SCRIPT.contains("srv.shutdown()"));
    }
}
