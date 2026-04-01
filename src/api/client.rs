use super::models::{AnimeDetails, AnimeItem, AnimeSearchResponse, EpisodeSourcesResponse};
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::{Client, header};
use sha2::{Digest, Sha256};
use std::time::Duration;

const API_BASE_URL: &str = "https://api.anihub.in.ua";
const INTERNAL_API_BASE_URL: &str = "https://anihub.in.ua/api";

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
}

impl ApiClient {
    pub fn http_client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn new() -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"),
        );
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self { client })
    }

    fn generate_api_key(&self) -> String {
        let date_str = Utc::now().format("%Y-%m-%d").to_string();
        let key_str = format!("Ukr@in1anAn1me-S3curity-Key-2025_{}", date_str);

        let mut hasher = Sha256::new();
        hasher.update(key_str.as_bytes());
        let result = hasher.finalize();

        hex::encode(result)
    }

    pub async fn search_anime(&self, query: &str) -> Result<Vec<AnimeItem>> {
        // anihub search — точний substring match. Якщо запит містить ':', шукаємо лише
        // за частиною до двокрапки: "Mushoku Tensei: Jobless..." → "Mushoku Tensei".
        let safe_query = if let Some(p) = query.find(':') {
            query[..p].trim().replace('?', "")
        } else {
            query.replace('?', "")
        };
        let url = format!("{}/anime/?search={}", API_BASE_URL, safe_query);
        let api_key = self.generate_api_key();

        let response = self
            .client
            .get(&url)
            .header("X-API-Key", api_key)
            .send()
            .await
            .context("Failed to send search request")?;

        if !response.status().is_success() {
            anyhow::bail!("API returned error status: {}", response.status());
        }

        let search_response: AnimeSearchResponse = response
            .json()
            .await
            .context("Failed to parse search response")?;

        // Filter items to keep only those with Ukrainian dub
        let filtered_items = search_response
            .items
            .into_iter()
            .filter(|item| item.has_ukrainian_dub)
            .collect();

        Ok(filtered_items)
    }

    pub async fn get_anime_details(&self, anime_id: u32) -> Result<AnimeDetails> {
        let url = format!("{}/anime/{}/", API_BASE_URL, anime_id);
        let api_key = self.generate_api_key();

        let response = self
            .client
            .get(&url)
            .header("X-API-Key", api_key)
            .send()
            .await
            .context("Failed to send anime details request")?;

        if !response.status().is_success() {
            anyhow::bail!("API returned error status: {}", response.status());
        }

        let details: AnimeDetails = response
            .json()
            .await
            .context("Failed to parse anime details response")?;

        Ok(details)
    }

    pub async fn get_episode_sources(
        &self,
        anime_id: u32,
        season: u32,
    ) -> Result<EpisodeSourcesResponse> {
        let url = format!(
            "{}/anime/{}/episode-sources?season={}",
            INTERNAL_API_BASE_URL, anime_id, season
        );
        let api_key = self.generate_api_key();

        let response = self
            .client
            .get(&url)
            .header("X-API-Key", api_key)
            .send()
            .await
            .context("Failed to send episode sources request")?;

        if !response.status().is_success() {
            anyhow::bail!("API returned error status: {}", response.status());
        }

        let sources_response: EpisodeSourcesResponse = response
            .json()
            .await
            .context("Failed to parse episode sources response")?;

        Ok(sources_response)
    }

    /// Пошук аніме за AniList ID. Повертає anihub anime_id або None.
    /// Фільтрує тільки ті що мають українське озвучення.
    pub async fn get_anime_by_anilist_id(&self, anilist_id: u32) -> Result<Option<u32>> {
        let url = format!(
            "{}/anime/?anilist_id={}&page_size=1&has_ukrainian_dub=true",
            API_BASE_URL, anilist_id
        );
        let api_key = self.generate_api_key();
        let response = self
            .client
            .get(&url)
            .header("X-API-Key", api_key)
            .send()
            .await?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let search_response: AnimeSearchResponse = response.json().await?;
        Ok(search_response.items.into_iter().next().map(|a| a.id))
    }

    /// Пошук аніме за AniList ID без фільтру has_ukrainian_dub.
    /// Повертає перший знайдений anihub anime_id або None.
    /// Використовується для отримання метаданих сезонів, які ще не мають плеєрів.
    pub async fn get_anime_by_anilist_id_any(&self, anilist_id: u32) -> Result<Option<u32>> {
        let url = format!(
            "{}/anime/?anilist_id={}&page_size=1",
            API_BASE_URL, anilist_id
        );
        let api_key = self.generate_api_key();
        let response = self
            .client
            .get(&url)
            .header("X-API-Key", api_key)
            .send()
            .await?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let search_response: AnimeSearchResponse = response.json().await?;
        Ok(search_response.items.into_iter().next().map(|a| a.id))
    }

    pub async fn fetch_poster(&self, url: &str) -> Result<image::DynamicImage> {
        let bytes = self.client.get(url).send().await?.bytes().await?;
        Ok(image::load_from_memory(&bytes)?)
    }

    /// Завантажує всі доступні сезони і об'єднує студії в один список.
    /// Ashdi — пріоритет; moonanime — fallback для сезонів без ashdi-даних.
    pub async fn get_episode_sources_for_anime(
        &self,
        anime_id: u32,
    ) -> Result<EpisodeSourcesResponse> {
        let mut all_studios: Vec<super::models::AshdiStudio> = Vec::new();
        let mut consecutive_empty: u32 = 0;

        for season in 1u32..=8 {
            match self.get_episode_sources(anime_id, season).await {
                Ok(sources) if !sources.ashdi.is_empty() => {
                    all_studios.extend(sources.ashdi);
                    consecutive_empty = 0;
                }
                Ok(sources) if !sources.moonanime.is_empty() => {
                    // Fallback: конвертуємо moonanime у AshdiStudio-формат
                    for moon in sources.moonanime {
                        let episodes = moon
                            .episodes
                            .into_iter()
                            .map(|ep| {
                                super::models::AshdiEpisode {
                                    episode_number: ep.episode_number,
                                    display_episode_number: ep.display_episode_number,
                                    title: ep.title,
                                    // iframe_url зберігаємо в полі url; розпізнаємо при відтворенні
                                    url: ep.iframe_url,
                                    ashdi_episode_id: String::new(),
                                }
                            })
                            .collect::<Vec<_>>();
                        all_studios.push(super::models::AshdiStudio {
                            id: moon.id,
                            studio_name: moon.studio_name,
                            season_number: moon.season_number,
                            episodes,
                            episodes_count: moon.episodes_count,
                        });
                    }
                    consecutive_empty = 0;
                }
                _ => {
                    consecutive_empty += 1;
                    if consecutive_empty >= 3 {
                        break;
                    }
                }
            }
        }

        if all_studios.is_empty() {
            anyhow::bail!("No episode sources found for anime {}", anime_id);
        }

        all_studios.sort_by(|a, b| a.season_number.cmp(&b.season_number));

        Ok(EpisodeSourcesResponse {
            ashdi: all_studios,
            moonanime: Vec::new(),
        })
    }
}
