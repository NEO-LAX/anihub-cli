//! Library sorting and bulk watched-state actions.

use super::*;

impl AppState {
    pub(super) fn open_library_sort_popup(&mut self) {
        let selected = LibrarySort::ALL
            .iter()
            .position(|sort| *sort == self.library.sort)
            .unwrap_or(0);
        self.library.sort_popup = Some(selected);
    }

    pub(super) fn handle_library_sort_popup(&mut self, key_code: KeyCode) -> bool {
        let Some(selected) = self.library.sort_popup else {
            return false;
        };
        match key_code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.library.sort_popup = Some(selected.saturating_sub(1));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.library.sort_popup = Some((selected + 1).min(LibrarySort::ALL.len() - 1));
            }
            KeyCode::Enter => {
                let selected_sort = LibrarySort::ALL[selected];
                if selected_sort == self.library.sort {
                    self.library.sort_reversed = !self.library.sort_reversed;
                } else {
                    self.library.sort = selected_sort;
                    self.library.sort_reversed = false;
                }
                self.settings.library_sort = library_sort_to_setting(self.library.sort);
                self.settings.library_sort_reversed = self.library.sort_reversed;
                self.library.sort_popup = None;
                self.apply_library_filter();
            }
            KeyCode::Esc | KeyCode::Char('s') => self.library.sort_popup = None,
            _ => {}
        }
        true
    }

    pub(super) fn remember_library_selection(&mut self) {
        if let Some(anime_id) = self.library_selected_anime_id() {
            self.settings.last_library_anime_id = Some(anime_id);
        }
    }

    pub(super) fn acknowledge_selected_library_release(&mut self) {
        let Some(release) = self.library_selected_season() else {
            return;
        };
        let (anime_id, episodes_count) = (release.anime_id, release.episodes_count);
        self.settings.acknowledge_release(anime_id, episodes_count);
    }

    pub fn persist_library_session(&mut self) -> anyhow::Result<()> {
        self.remember_library_selection();
        self.settings_store.save(&self.settings)
    }

    pub(super) fn episode_targets_for_release(
        &self,
        anime_id: u32,
        release: &LibraryReleaseMetadata,
    ) -> BTreeMap<u32, String> {
        let mut target_episodes = self
            .history
            .progress
            .values()
            .filter(|progress| progress.anime_id == anime_id && progress.season == release.season)
            .map(|progress| (progress.episode, progress.studio_name.clone()))
            .collect::<BTreeMap<_, _>>();
        let source_key = EpisodeSourcesKey::new(anime_id, release.season);
        let sources = self.sources_cache.get(&source_key).or_else(|| {
            (self.content.current_sources_key == Some(source_key))
                .then(|| self.content.current_sources.clone())
                .flatten()
        });
        let mut has_source_episodes = false;
        if let Some(sources) = sources {
            for studio in sources
                .ashdi
                .iter()
                .filter(|studio| studio.season_number == release.season)
            {
                for episode in &studio.episodes {
                    has_source_episodes = true;
                    target_episodes
                        .entry(episode.episode_number)
                        .or_insert_with(|| studio.studio_name.clone());
                }
            }
            if !has_source_episodes {
                for studio in sources
                    .moonanime
                    .iter()
                    .filter(|studio| studio.season_number == release.season)
                {
                    for episode in &studio.episodes {
                        has_source_episodes = true;
                        target_episodes
                            .entry(episode.episode_number)
                            .or_insert_with(|| studio.studio_name.clone());
                    }
                }
            }
        }
        if !has_source_episodes {
            if let Some(count) = release.episodes_count {
                let first = release.first_episode.unwrap_or(1);
                for episode in first..first.saturating_add(count) {
                    target_episodes.insert(episode, "Статус".to_string());
                }
            }
        }
        target_episodes
    }

    pub(super) fn open_library_watched_confirmation(&mut self) {
        let Some(anime) = self.library_selected_anime().cloned() else {
            return;
        };
        if anime.seasons.is_empty() {
            return;
        }
        let all_watched = anime.seasons.iter().all(|release| {
            if release.status == AnimeStatus::Completed {
                return true;
            }
            let targets = self.episode_targets_for_release(release.anime_id, &release.metadata());
            !targets.is_empty()
                && targets.keys().all(|episode| {
                    self.watched_index
                        .contains(&(release.anime_id, release.season, *episode))
                })
        });
        self.library.pending_watched_confirmation = Some(LibraryWatchedConfirmation {
            anime_title: anime.anime_title,
            releases: anime.seasons,
            mark_watched: !all_watched,
        });
    }

    pub(super) fn handle_library_watched_confirmation(&mut self, key_code: KeyCode) -> bool {
        let Some(confirmation) = self.library.pending_watched_confirmation.clone() else {
            return false;
        };
        match key_code {
            KeyCode::Enter => {
                self.library.pending_watched_confirmation = None;
                let mut status_updates = Vec::with_capacity(confirmation.releases.len());
                let mut episode_updates = Vec::new();
                for release in &confirmation.releases {
                    let metadata = release.metadata();
                    let status =
                        if confirmation.mark_watched && !release_metadata_is_ongoing(&metadata) {
                            AnimeStatus::Completed
                        } else {
                            AnimeStatus::Watching
                        };
                    status_updates.push(AnimeStatusUpdate {
                        anime_id: release.anime_id,
                        title: confirmation.anime_title.clone(),
                        status,
                        release: Some(metadata.clone()),
                    });
                    episode_updates.extend(
                        self.episode_targets_for_release(release.anime_id, &metadata)
                            .into_iter()
                            .map(|(episode, studio_name)| EpisodeWatchedUpdate {
                                anime_id: release.anime_id,
                                anime_title: confirmation.anime_title.clone(),
                                season: release.season,
                                episode,
                                studio_name,
                                watched: confirmation.mark_watched,
                            }),
                    );
                }
                match self
                    .storage
                    .set_releases_watched(&status_updates, &episode_updates)
                {
                    Ok(history) => {
                        self.history = history;
                        self.rebuild_history_indexes();
                        self.reload_library_after_mutation();
                        self.set_info_status(if confirmation.mark_watched {
                            format!("\"{}\" позначено як переглянуте", confirmation.anime_title)
                        } else {
                            format!(
                                "\"{}\" позначено як непереглянуте",
                                confirmation.anime_title
                            )
                        });
                    }
                    Err(error) => {
                        self.set_error_status(format!("Не вдалося оновити аніме: {error}"));
                    }
                }
            }
            KeyCode::Esc => self.library.pending_watched_confirmation = None,
            _ => {}
        }
        true
    }
}
