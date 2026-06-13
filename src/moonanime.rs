use crate::debug_log;


/// Python-скрипт для Playwright: відкриває iframe сторінку headless Firefox,
/// перехоплює перший мережевий запит до `s.moonanime.art/*.m3u8` і виводить URL у stdout.
/// m3u8 URL формується обфускованим JS на стороні клієнта — статичний парсинг HTML не працює.
/// Playwright скрипт для MoonAnime:
/// 1. Firefox headless завантажує iframe сторінку (обходить TLS fingerprint CDN s.moonanime.art)
/// 2. Перехоплює master m3u8 URL (генерується obfuscated JS)
/// 3. Завантажує master manifest через JS fetch (browser контекст)
/// 4. Знаходить найкращу якість (1080p → 720p → перша)
/// 5. Завантажує variant manifest через JS fetch
/// 6. Зберігає у /tmp/anihub_moon_stream.m3u8 (segments — прямі s3.mooncdn.space URLs, доступні без proxy)
/// 7. Виводить шлях до файлу в stdout → mpv грає локальний manifest
/// Playwright скрипт для MoonAnime:
/// 1. Firefox headless завантажує iframe (обходить TLS fingerprint CDN s.moonanime.art)
/// 2. Перехоплює master m3u8 URL, завантажує manifest через JS fetch
/// 3. Знаходить найкращу якість, завантажує variant manifest
/// 4. Запускає локальний HTTP сервер (Content-Type: application/vnd.apple.mpegurl)
///    щоб mpv правильно розпізнав HLS (без HTTP → трактує як M3U плейлист)
/// 5. Виводить http://127.0.0.1:PORT/stream.m3u8 → Rust передає mpv
/// 6. Завершується коли Rust закриває stdin (mpv завершив відтворення)
pub const MOONANIME_PLAYWRIGHT_SCRIPT: &str = r#"
import asyncio, sys, re, socket
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
    def log_message(self, *a): pass

async def js_fetch(page, url):
    return await page.evaluate("""async (url) => {
        const r = await fetch(url, {
            headers: {"Origin": "https://moonanime.art", "Referer": "https://moonanime.art/"}
        });
        return {status: r.status, text: await r.text()};
    }""", url)

async def main():
    global _manifest
    from playwright.async_api import async_playwright
    iframe_url = sys.argv[1]

    # Один браузерний сеанс: завантажуємо iframe, ловимо media URL через event.
    # Підтримуємо два формати:
    #   HLS:  s.moonanime.art/...m3u8  — завантажуємо variant manifest, роздаємо через HTTP proxy
    #   WebM: s.moonanime.art/content/v/...  — отримуємо redirect URL (s1.mooncdn.space),
    #         який mpv може грати напряму без proxy
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
            # WebM-плеєр потребує кліку для старту відтворення
            if not master_url:
                try:
                    await page.click("body")
                except Exception:
                    pass
                await asyncio.sleep(3)
        except Exception:
            pass

        if master_url:
            # --- HLS шлях: завантажуємо variant manifest і роздаємо через HTTP proxy ---
            master = await js_fetch(page, master_url)
            if master["status"] != 200:
                await browser.close()
                sys.exit(1)

            variants = re.findall(r'(https://s\.moonanime\.art/[^\s]+\.m3u8[^\s]*)', master["text"])
            if not variants:
                await browser.close()
                sys.exit(1)
            best = next((v for v in variants if "1080" in v),
                   next((v for v in variants if "720" in v), variants[0]))

            variant = await js_fetch(page, best)
            if variant["status"] != 200:
                await browser.close()
                sys.exit(1)

            _manifest = variant["text"].encode("utf-8")
            await browser.close()

            s = socket.socket(); s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]; s.close()
            srv = HTTPServer(("127.0.0.1", port), Handler)
            Thread(target=srv.serve_forever, daemon=True).start()

            print(f"http://127.0.0.1:{port}/stream.m3u8", flush=True)

            import time
            while True:
                time.sleep(60)

        elif webm_url:
            # --- WebM шлях: отримуємо фінальний URL через редирект, mpv грає напряму ---
            result = await page.evaluate("""async (url) => {
                try {
                    const r = await fetch(url, {redirect: "follow", headers: {Range: "bytes=0-0"}});
                    return {finalUrl: r.url, status: r.status};
                } catch(e) {
                    return {error: e.toString()};
                }
            }""", webm_url)
            await browser.close()

            final_url = result.get("finalUrl", "")
            status = result.get("status", 0)
            if not final_url or status not in (200, 206):
                sys.exit(1)

            # s1.mooncdn.space доступний для mpv без proxy
            print(final_url, flush=True)
            # Виходимо — proxy не потрібен

        else:
            await browser.close()
            sys.exit(1)

asyncio.run(main())
"#;



/// Запускає MoonAnime HLS proxy (Python + Playwright Firefox headless).
/// Скрипт завантажує variant manifest через browser JS fetch (обходить TLS fingerprint),
/// стартує локальний HTTP сервер з Content-Type: application/vnd.apple.mpegurl,
/// виводить URL в stdout. Процес живе доки Rust не викличе kill().
///
/// Повертає `Some((http_url, child_process))` або `None` якщо не вдалось.
pub async fn try_moonanime_stream(
    iframe_url: &str,
) -> Option<(String, tokio::process::Child)> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let script_path = std::env::temp_dir().join("anihub_moon_extract.py");
    std::fs::write(&script_path, MOONANIME_PLAYWRIGHT_SCRIPT).ok()?;

    let mut child = tokio::process::Command::new("python3")
        .arg(&script_path)
        .arg(iframe_url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // Читаємо першу непорожню лінію зі stdout — це HTTP URL proxy
    let stdout = child.stdout.take()?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let read_result = tokio::time::timeout(
        tokio::time::Duration::from_secs(40),
        reader.read_line(&mut line),
    )
    .await;

    match read_result {
        Ok(Ok(_)) => {
            let url = line.trim().to_string();
            // Приймаємо і HLS proxy (http://127.0.0.1) і WebM direct URL (https://s1.mooncdn.space)
            if url.starts_with("http://") || url.starts_with("https://") {
                debug_log(&format!("[moonanime] stream OK: {}", url));
                Some((url, child))
            } else {
                debug_log(&format!("[moonanime] bad output: {:?}", url));
                let _ = child.kill().await;
                None
            }
        }
        Ok(Err(e)) => {
            debug_log(&format!("[moonanime] read error: {}", e));
            let _ = child.kill().await;
            None
        }
        Err(_) => {
            debug_log("[moonanime] proxy timeout (>40s)");
            let _ = child.kill().await;
            None
        }
    }
}
