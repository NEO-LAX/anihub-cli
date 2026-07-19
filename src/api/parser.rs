use anyhow::{Context, Result};
use regex::Regex;
use reqwest::Client;
use std::sync::LazyLock;
use std::time::Duration;

// Ashdi player configuration: file:'https://...m3u8' or file:"...".
// Capture first and validate separately so query strings and JSON-escaped
// slashes stay supported without accepting a non-HTTP player value.
static RE_ASHDI_M3U8: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"file\s*:\s*['"]([^'"]+\.m3u8(?:\?[^'"]*)?)['"]"#).unwrap());

fn extract_m3u8_from_html(html: &str) -> Option<String> {
    let raw = RE_ASHDI_M3U8.captures(html)?.get(1)?.as_str();
    let normalized = raw.replace("\\/", "/");
    let url = reqwest::Url::parse(&normalized).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.path().ends_with(".m3u8") {
        return None;
    }
    Some(normalized)
}

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

        if let Some(m3u8_url) = extract_m3u8_from_html(&html) {
            return Ok(m3u8_url);
        }

        anyhow::bail!("Could not find m3u8 link in the ashdi page")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_legacy_single_quoted_player_value() {
        let html = include_str!("../../tests/fixtures/ashdi/player-single-quoted.html");
        assert_eq!(
            extract_m3u8_from_html(html).as_deref(),
            Some("https://ashdi.vip/media/episode-1/index.m3u8")
        );
    }

    #[test]
    fn normalizes_escaped_slashes_and_keeps_signed_query_parameters() {
        let html = include_str!("../../tests/fixtures/ashdi/player-escaped-query.html");
        assert_eq!(
            extract_m3u8_from_html(html).as_deref(),
            Some("https://cdn.example/anime/master.m3u8?token=abc123&expires=2000000000")
        );
    }

    #[test]
    fn rejects_pages_without_an_http_hls_stream() {
        let html = include_str!("../../tests/fixtures/ashdi/player-without-stream.html");
        assert_eq!(extract_m3u8_from_html(html), None);
        assert_eq!(
            extract_m3u8_from_html("file: 'javascript:alert(1).m3u8'"),
            None
        );
    }
}
