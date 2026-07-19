//! Library catalog refresh orchestration.
//!
//! The pure catalog rules live in `library_catalog`; this module owns the
//! storage write and the minimal UI refresh required after it succeeds.

use super::*;

impl AppState {
    pub fn take_library_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.library.refresh_requested)
    }

    pub fn library_refresh_queries(&self) -> Vec<String> {
        refresh_queries(
            self.library
                .all_items
                .iter()
                .map(|anime| anime.anime_title.as_str()),
        )
    }

    pub fn apply_library_refresh_catalogs(
        &mut self,
        catalogs: &[FranchiseCatalog],
    ) -> anyhow::Result<()> {
        let updates = library_catalog_updates(&self.history, catalogs);
        if updates.is_empty() {
            return Ok(());
        }
        self.history = self.storage.set_anime_statuses(&updates)?;
        self.rebuild_history_indexes();
        if self.is_library_mode() {
            self.reload_library_after_mutation();
        }
        Ok(())
    }

    /// Persist the complete available franchise whenever one of its releases
    /// already belongs to the library or has playback progress. This both
    /// upgrades old records and keeps future restarts independent of search.
    pub(crate) fn hydrate_library_catalog_metadata(&mut self) {
        let updates = library_catalog_updates(&self.history, &self.search.franchise_catalogs);
        if !updates.is_empty() {
            match self.storage.set_anime_statuses(&updates) {
                Ok(history) => {
                    self.history = history;
                    self.rebuild_history_indexes();
                }
                Err(error) => {
                    self.set_error_status(format!("Не вдалося оновити формат бібліотеки: {error}"));
                    return;
                }
            }
        }

        let baseline = newly_tracked_catalog_baseline(
            &self.history,
            &self.settings,
            &self.search.franchise_catalogs,
        );
        if baseline.is_empty() {
            return;
        }
        for (anime_id, episodes_count) in baseline {
            self.settings.acknowledge_release(anime_id, episodes_count);
        }
        if let Err(error) = self.settings_store.save(&self.settings) {
            self.set_error_status(format!(
                "Не вдалося зберегти baseline нових випусків: {error}"
            ));
        }
    }
}

/// Releases that already exist when the user starts tracking a franchise are
/// its baseline, not newly discovered content. Once any release in that
/// franchise is acknowledged, later releases stay unacknowledged and receive
/// the normal "new season/movie" badge.
fn newly_tracked_catalog_baseline(
    history: &AppHistory,
    settings: &Settings,
    catalogs: &[FranchiseCatalog],
) -> Vec<(u32, Option<u32>)> {
    if !settings.new_content_initialized {
        return Vec::new();
    }

    let mut baseline = BTreeMap::new();
    for catalog in catalogs {
        let available = catalog
            .releases
            .iter()
            .filter(|release| release.availability == ReleaseAvailability::Available)
            .filter_map(|release| release.anihub_id.map(|anime_id| (anime_id, release)))
            .collect::<Vec<_>>();
        let is_tracked = available.iter().any(|(anime_id, _)| {
            history
                .library
                .get(anime_id)
                .is_some_and(|record| record.status != AnimeStatus::NotAdded)
                || history
                    .progress
                    .values()
                    .any(|progress| progress.anime_id == *anime_id)
        });
        let has_acknowledged_release = available
            .iter()
            .any(|(anime_id, _)| settings.acknowledged_release_ids.contains(anime_id));
        if !is_tracked || has_acknowledged_release {
            continue;
        }

        for (anime_id, release) in available {
            baseline.insert(
                anime_id,
                release.available_episodes.or(release.episodes_count),
            );
        }
    }
    baseline.into_iter().collect()
}

fn refresh_queries<'a>(titles: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut queries = titles
        .into_iter()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    queries.sort_by_key(|title| title.to_lowercase());
    queries.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    queries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn season_release(anime_id: u32, season: u32, episodes: u32) -> ReleaseEntry {
        ReleaseEntry {
            anihub_id: Some(anime_id),
            anilist_id: Some(anime_id + 10_000),
            title: format!("K-On season {season}"),
            anime_type: "TV".to_string(),
            year: Some(2008 + season),
            poster_url: None,
            episodes_count: Some(episodes),
            available_episodes: Some(episodes),
            airing_status: Some("FINISHED".to_string()),
            next_airing_episode: None,
            next_airing_at: None,
            description: None,
            rating: None,
            genres: None,
            dubbing_studios: None,
            conceptual_season: Some(season),
            part: Some(1),
            classification: ReleaseClassification::MainlineSeason,
            availability: ReleaseAvailability::Available,
        }
    }

    fn two_season_catalog() -> FranchiseCatalog {
        FranchiseCatalog {
            anchor_anilist_id: Some(10_001),
            canonical_title: "K-On".to_string(),
            canonical_poster_url: None,
            unresolved_anilist_ids: Vec::new(),
            releases: vec![season_release(1, 1, 13), season_release(2, 2, 26)],
        }
    }

    fn tracked_first_season() -> AppHistory {
        let mut history = AppHistory::default();
        history.library.insert(
            1,
            crate::storage::history::AnimeLibraryRecord {
                title: "K-On".to_string(),
                status: AnimeStatus::Watching,
                updated_at: 10,
                release: None,
            },
        );
        history
    }

    #[test]
    fn refresh_queries_trim_drop_empty_and_deduplicate_case_insensitively() {
        assert_eq!(
            refresh_queries(["  Frieren  ", "", "frieren", " Каґуя ", "   "]),
            vec!["Frieren", "Каґуя"]
        );
    }

    #[test]
    fn newly_tracked_franchise_uses_its_existing_seasons_as_the_baseline() {
        let history = tracked_first_season();
        let mut settings = Settings {
            new_content_initialized: true,
            ..Settings::default()
        };
        let catalog = two_season_catalog();

        assert_eq!(
            newly_tracked_catalog_baseline(&history, &settings, std::slice::from_ref(&catalog)),
            vec![(1, Some(13)), (2, Some(26))]
        );

        settings.acknowledge_release(1, Some(13));
        assert!(
            newly_tracked_catalog_baseline(&history, &settings, &[catalog]).is_empty(),
            "once a franchise has a baseline, later releases must remain new"
        );
    }
}
