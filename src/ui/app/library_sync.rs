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
        if updates.is_empty() {
            return;
        }
        match self.storage.set_anime_statuses(&updates) {
            Ok(history) => {
                self.history = history;
                self.rebuild_history_indexes();
            }
            Err(error) => {
                self.set_error_status(format!("Не вдалося оновити формат бібліотеки: {error}"));
            }
        }
    }
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

    #[test]
    fn refresh_queries_trim_drop_empty_and_deduplicate_case_insensitively() {
        assert_eq!(
            refresh_queries(["  Frieren  ", "", "frieren", " Каґуя ", "   "]),
            vec!["Frieren", "Каґуя"]
        );
    }
}
