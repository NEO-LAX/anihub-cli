use anyhow::{Context, Result};
use regex::Regex;
use reqwest::Client;
use std::sync::LazyLock;
use std::time::Duration;

static RE_M3U8: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"file\s*:\s*['"](https?://[^'"]+\.m3u8)['"]"#).unwrap()
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
        if let Some(captures) = RE_M3U8.captures(&html) {
            if let Some(m3u8_url) = captures.get(1) {
                return Ok(m3u8_url.as_str().to_string());
            }
        }

        anyhow::bail!("Could not find m3u8 link in the ashdi page")
    }
}
