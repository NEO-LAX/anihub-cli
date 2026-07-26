//! MoonAnime embed decoding.
//!
//! MoonAnime does not expose the stream URL in cleartext the way Ashdi does.
//! The embed page carries it behind two deterministic layers, and both keys are
//! shipped in the page itself, so no JavaScript engine is involved:
//!
//! 1. An `atob("...")` blob whose first byte seeds a running state and whose
//!    next 32 bytes are the key for a rolling XOR over the remainder. The plain
//!    text is the player's real configuration script.
//! 2. Inside that script, `file:`/`subtitle:` values are `_0xd("...")` calls —
//!    base64 followed by XOR against a short key held in a `var k = "..."` in
//!    the same script. That key is regenerated per page, so it is always read
//!    rather than hardcoded.
//!
//! This is deliberate obfuscation on MoonAnime's side, not encryption: there is
//! no DRM on the stream. It also means the layout can change without notice —
//! every entry point here returns `None`/`Err` instead of panicking so callers
//! can fall back to opening the embed in a browser.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use regex::Regex;
use reqwest::Client;
use std::sync::LazyLock;
use std::time::Duration;

/// MoonAnime answers requests without a language preference with HTTP 400.
/// This applies to the embed page, the master playlist and every variant, so it
/// has to reach mpv too — not only the client that resolved the URL. Kept free
/// of commas so it survives mpv's comma-separated header list unescaped; a bare
/// language tag is accepted (verified against the CDN).
pub const ACCEPT_LANGUAGE: &str = "uk";

/// The header in the `Name: Value` form mpv expects.
pub fn accept_language_header() -> String {
    format!("Accept-Language: {ACCEPT_LANGUAGE}")
}

static RE_OUTER_BLOB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"atob\("([A-Za-z0-9+/=]{40,})"\)"#).unwrap());
static RE_INNER_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"var\s+k\s*=\s*"([^"]+)""#).unwrap());
static RE_FILE_FIELD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"file\s*:\s*_0xd\("([^"]+)"\)"#).unwrap());
static RE_SUBTITLE_FIELD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"subtitle\s*:\s*_0xd\("([^"]+)"\)"#).unwrap());
/// The subtitle field is a comma-separated `[label]url` list.
static RE_SUBTITLE_TRACK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]*)\]([^,]+)").unwrap());

/// Undoes layer 1: `cipher[0]` seeds the state, `cipher[1..33]` is the key.
fn rolling_xor(cipher: &[u8]) -> Option<Vec<u8>> {
    if cipher.len() <= 33 {
        return None;
    }
    let mut state = cipher[0];
    let key = &cipher[1..33];
    let mut plain = Vec::with_capacity(cipher.len() - 33);
    for byte in &cipher[33..] {
        let k = key[plain.len() % key.len()];
        plain.push(byte ^ k ^ state);
        state = byte.wrapping_add(k);
    }
    Some(plain)
}

fn xor_with_key(data: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    if key.is_empty() {
        return None;
    }
    Some(
        data.iter()
            .enumerate()
            .map(|(index, byte)| byte ^ key[index % key.len()])
            .collect(),
    )
}

/// Recovers the player configuration script from the embed page.
fn decode_player_script(html: &str) -> Option<String> {
    let blob = RE_OUTER_BLOB.captures(html)?.get(1)?.as_str();
    let cipher = BASE64.decode(blob).ok()?;
    String::from_utf8(rolling_xor(&cipher)?).ok()
}

/// Decodes one `_0xd("...")` field out of the player script.
fn decode_field(script: &str, field: &Regex) -> Option<String> {
    let key = RE_INNER_KEY.captures(script)?.get(1)?.as_str();
    let payload = field.captures(script)?.get(1)?.as_str();
    let raw = BASE64.decode(payload).ok()?;
    String::from_utf8(xor_with_key(&raw, key.as_bytes())?).ok()
}

/// Rejects anything that is not the expected kind of file served by MoonAnime,
/// so a changed page layout cannot turn into an arbitrary URL handed to mpv.
fn validate_url(candidate: &str, extension: &str) -> Option<String> {
    let url = reqwest::Url::parse(candidate.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if !url.path().ends_with(extension) {
        return None;
    }
    let host = url.host_str()?;
    if host != "moonanime.art" && !host.ends_with(".moonanime.art") {
        return None;
    }
    Some(candidate.trim().to_string())
}

/// What a MoonAnime embed yields once decoded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoonAnimeStream {
    pub manifest_url: String,
    /// External WebVTT tracks, full subtitles first as the page orders them.
    /// Labels are dropped: mpv cannot name external subtitle files from the
    /// command line, so only the order carries meaning.
    pub subtitle_urls: Vec<String>,
}

fn parse_subtitle_tracks(field: &str) -> Vec<String> {
    RE_SUBTITLE_TRACK
        .captures_iter(field)
        .filter_map(|track| validate_url(track.get(2)?.as_str(), ".vtt"))
        .collect()
}

/// Extracts the master playlist and any external subtitles from an embed page.
pub fn extract_stream(html: &str) -> Option<MoonAnimeStream> {
    let script = decode_player_script(html)?;
    let manifest_url = validate_url(&decode_field(&script, &RE_FILE_FIELD)?, ".m3u8")?;
    Some(MoonAnimeStream {
        manifest_url,
        subtitle_urls: decode_field(&script, &RE_SUBTITLE_FIELD)
            .map(|field| parse_subtitle_tracks(&field))
            .unwrap_or_default(),
    })
}

pub struct MoonAnimeParser {
    client: Client,
}

impl MoonAnimeParser {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .timeout(Duration::from_secs(15))
            .build()
            .context("Failed to build HTTP client for MoonAnime")?;
        Ok(Self { client })
    }

    pub async fn extract_stream(&self, iframe_url: &str) -> Result<MoonAnimeStream> {
        let response = self
            .client
            .get(iframe_url)
            .header(reqwest::header::ACCEPT_LANGUAGE, ACCEPT_LANGUAGE)
            .send()
            .await
            .context("Failed to fetch the MoonAnime embed page")?;

        if !response.status().is_success() {
            bail!("MoonAnime embed returned status: {}", response.status());
        }

        let html = response
            .text()
            .await
            .context("Failed to read the MoonAnime embed page")?;

        extract_stream(&html).context(
            "Could not decode a stream URL from the MoonAnime embed \
             (the page layout may have changed)",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a page in the same shape MoonAnime serves, so the decoder is
    /// exercised end to end without committing a real signed URL.
    fn obfuscated_page(manifest: &str, inner_key: &str) -> String {
        page_with_subtitles(manifest, inner_key, None)
    }

    fn page_with_subtitles(manifest: &str, inner_key: &str, subtitle: Option<&str>) -> String {
        let encode = |text: &str| {
            BASE64.encode(xor_with_key(text.as_bytes(), inner_key.as_bytes()).unwrap())
        };
        let subtitle_field = subtitle
            .map(|field| format!(r#", subtitle: _0xd("{}")"#, encode(field)))
            .unwrap_or_default();
        let script = format!(
            r#"function _0xd(e){{var k="{inner_key}";}} var player = {{ file: _0xd("{}"){subtitle_field} }};"#,
            encode(manifest)
        );

        // Re-apply layer 1: pick a seed and key, then walk the plaintext.
        let seed = 0x5Au8;
        let key: Vec<u8> = (0u8..32)
            .map(|index| index.wrapping_mul(7).wrapping_add(3))
            .collect();
        let mut cipher = vec![seed];
        cipher.extend_from_slice(&key);
        let mut state = seed;
        for (index, plain) in script.as_bytes().iter().enumerate() {
            let k = key[index % key.len()];
            // plain = c ^ k ^ state  =>  c = plain ^ k ^ state
            let c = plain ^ k ^ state;
            cipher.push(c);
            state = c.wrapping_add(k);
        }
        format!(
            "<html><script>eval(atob(\"{}\"));</script></html>",
            BASE64.encode(&cipher)
        )
    }

    const MANIFEST: &str = "https://s.moonanime.art/content/stream/anime/44/abc/hls/manifest.m3u8?expires=2000000000&sig=deadbeef";

    #[test]
    fn decodes_a_stream_url_through_both_obfuscation_layers() {
        let page = obfuscated_page(MANIFEST, "Ox7YCNNPwP8J");
        assert_eq!(
            extract_stream(&page).map(|s| s.manifest_url).as_deref(),
            Some(MANIFEST)
        );
    }

    #[test]
    fn reads_the_inner_key_from_the_page_rather_than_assuming_one() {
        // MoonAnime regenerates this key per page; a hardcoded one would break.
        for key in [
            "757bdn5JBD0U",
            "obGBHkxyYdS0",
            "4I43iCXA7nwS",
            "pPw1ZuFtFW96",
        ] {
            let page = obfuscated_page(MANIFEST, key);
            assert_eq!(
                extract_stream(&page).map(|s| s.manifest_url).as_deref(),
                Some(MANIFEST),
                "failed for inner key {key}"
            );
        }
    }

    #[test]
    fn decodes_subtitle_tracks_in_page_order() {
        let field = "[Повні UKR]https://s.moonanime.art/subtitles/full.vtt?expires=1&sig=a,\
                     [ukr Написи]https://s.moonanime.art/subtitles/signs.vtt?expires=1&sig=b";
        let page = page_with_subtitles(MANIFEST, "Ox7YCNNPwP8J", Some(field));
        let stream = extract_stream(&page).unwrap();

        assert_eq!(stream.manifest_url, MANIFEST);
        assert_eq!(
            stream.subtitle_urls,
            vec![
                "https://s.moonanime.art/subtitles/full.vtt?expires=1&sig=a".to_string(),
                "https://s.moonanime.art/subtitles/signs.vtt?expires=1&sig=b".to_string(),
            ]
        );
    }

    #[test]
    fn a_page_without_subtitles_still_yields_the_stream() {
        let stream = extract_stream(&obfuscated_page(MANIFEST, "Ox7YCNNPwP8J")).unwrap();
        assert_eq!(stream.manifest_url, MANIFEST);
        assert!(stream.subtitle_urls.is_empty());
    }

    #[test]
    fn subtitle_tracks_are_filtered_to_moonanime_vtt_files() {
        assert_eq!(
            parse_subtitle_tracks(
                "[a]https://evil.example/x.vtt,\
                 [b]https://s.moonanime.art/y.exe,\
                 [c]https://s.moonanime.art/ok.vtt,\
                 [d]javascript:alert(1)//z.vtt"
            ),
            vec!["https://s.moonanime.art/ok.vtt".to_string()]
        );
        assert!(parse_subtitle_tracks("").is_empty());
        // Missing labels must not drop an otherwise valid track.
        assert_eq!(
            parse_subtitle_tracks("[]https://s.moonanime.art/ok.vtt"),
            vec!["https://s.moonanime.art/ok.vtt".to_string()]
        );
    }

    #[test]
    fn rejects_streams_that_are_not_moonanime_hls() {
        for candidate in [
            // Right host, wrong kind of file.
            "https://s.moonanime.art/content/video.mp4",
            // Valid HLS, but somebody else's host.
            "https://evil.example/manifest.m3u8",
            // Host that merely ends with the brand name.
            "https://notmoonanime.art/manifest.m3u8",
            "javascript:alert(1)//manifest.m3u8",
            "",
        ] {
            let page = obfuscated_page(candidate, "Ox7YCNNPwP8J");
            assert_eq!(
                extract_stream(&page).map(|s| s.manifest_url),
                None,
                "should have rejected {candidate}"
            );
        }
    }

    #[test]
    fn a_page_without_the_expected_shape_decodes_to_nothing() {
        assert!(extract_stream("<html>no player here</html>").is_none());
        // Blob present but not valid base64 of a long enough payload.
        assert!(
            extract_stream(
                r#"<script>atob("QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=")</script>"#
            )
            .is_none()
        );
    }

    #[test]
    fn rolling_xor_rejects_input_too_short_to_hold_a_key() {
        assert_eq!(rolling_xor(&[]), None);
        assert_eq!(rolling_xor(&[0; 33]), None);
        assert_eq!(rolling_xor(&[0; 34]).map(|plain| plain.len()), Some(1));
    }
}
