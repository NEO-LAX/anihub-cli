use anyhow::{Context, Result};
use regex::Regex;
use reqwest::Client;
use std::sync::LazyLock;
use std::time::Duration;

// Ashdi: file:'https://...m3u8' або file:"..."
static RE_ASHDI_M3U8: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"file\s*:\s*['"](https?://[^'"]+\.m3u8)['"]"#).unwrap()
});

// MoonAnime патерни (у порядку пріоритету):
// 1. Прямий URL s.moonanime.art
static RE_MOON_SRC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(https://s\.moonanime\.art/[^"'\s\\]+\.m3u8[^"'\s\\]*)"#).unwrap()
});
// 2. "src":"https://...m3u8..." або src:'...'
static RE_MOON_SRC_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"["\']src["\']\s*:\s*["\'](https?://[^"']+\.m3u8[^"']*)["\']"#).unwrap()
});
// 3. file:'...' або file:"..." (як в ashdi, деякі moonanime плеєри теж так)
static RE_MOON_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"file\s*:\s*['"](https?://[^'"]+\.m3u8[^'"]*)['"]\s*[,}]"#).unwrap()
});
// 4. Загальний: будь-який .m3u8 URL у лапках або атрибутах
static RE_MOON_GENERIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"["'](https?://[^"'\s]+\.m3u8\?[^"'\s]+)["']"#).unwrap()
});

pub struct AshdiParser {
    client: Client,
}

impl AshdiParser {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build HTTP client for parser")?;

        Ok(Self { client })
    }

    pub async fn extract_m3u8(&self, ashdi_url: &str) -> Result<String> {
        let response = self
            .client
            .get(ashdi_url)
            .send()
            .await
            .context("Failed to fetch ashdi page")?;

        if !response.status().is_success() {
            anyhow::bail!("Ashdi page returned status: {}", response.status());
        }

        let html = response
            .text()
            .await
            .context("Failed to get HTML from ashdi page")?;

        // Шукаємо рядок `file:'https://ashdi.vip/.../index.m3u8'`
        // або `file: "https://..."`
        if let Some(captures) = RE_ASHDI_M3U8.captures(&html) {
            if let Some(m3u8_url) = captures.get(1) {
                return Ok(m3u8_url.as_str().to_string());
            }
        }

        anyhow::bail!("Could not find m3u8 link in the ashdi page")
    }
}

/// Парсер для MoonAnime embed-сторінок.
/// Завантажує iframe URL та витягує пряме m3u8-посилання.
/// Підписані URL (sig=... expires=...) самодостатні — mpv може грати без cookies.
pub struct MoonAnimeParser {
    client: Client,
}

impl MoonAnimeParser {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build HTTP client for MoonAnime parser")?;
        Ok(Self { client })
    }

    /// Завантажує embed-сторінку і шукає m3u8 URL кількома regex-патернами.
    pub async fn extract_m3u8(&self, iframe_url: &str) -> Result<String> {
        let response = self
            .client
            .get(iframe_url)
            .header("Referer", "https://moonanime.art/")
            .send()
            .await
            .context("Failed to fetch moonanime embed page")?;

        if !response.status().is_success() {
            anyhow::bail!("MoonAnime embed page returned status: {}", response.status());
        }

        let html = response
            .text()
            .await
            .context("Failed to get HTML from moonanime page")?;

        // Перебираємо патерни у порядку точності
        for re in &[
            &*RE_MOON_SRC,
            &*RE_MOON_SRC_KEY,
            &*RE_MOON_FILE,
            &*RE_MOON_GENERIC,
        ] {
            if let Some(cap) = re.captures(&html) {
                if let Some(url) = cap.get(1) {
                    let u = url.as_str().to_string();
                    if !u.is_empty() {
                        return Ok(u);
                    }
                }
            }
        }

        anyhow::bail!("Could not find m3u8 URL in moonanime embed page")
    }
}
