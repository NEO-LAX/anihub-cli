//! Stale-while-revalidate refresh for library franchise metadata.
//!
//! The visible view keeps its own ResourceWorker generation. Library refresh
//! uses a separate high-range generation, so cursor movement and tab changes
//! never cancel background discovery.

use crate::api::resource::{LoadError, ResourceHandle, SearchResultBundle};
use crate::api::{
    AniListMedia, AnimeItem, RequestId, ResourceKey, ResourceValue, ViewGeneration,
    build_franchise_catalogs,
};
use crate::ui::AppState;
use std::collections::{BTreeMap, HashSet, VecDeque};

const FIRST_LIBRARY_GENERATION: u64 = 1 << 63;
const MAX_BACKGROUND_SEARCHES: usize = 2;

#[derive(Default)]
pub struct LibraryRefreshCoordinator {
    active: Option<ActiveRefresh>,
    generation_offset: u64,
}

struct ActiveRefresh {
    generation: ViewGeneration,
    pending: HashSet<RequestId>,
    queued_queries: VecDeque<String>,
    searching: bool,
    failed: bool,
    items: BTreeMap<u32, AnimeItem>,
    media: BTreeMap<u32, AniListMedia>,
}

impl ActiveRefresh {
    fn new(generation: ViewGeneration, queries: Vec<String>) -> Self {
        Self {
            generation,
            pending: HashSet::new(),
            queued_queries: queries.into(),
            searching: true,
            failed: false,
            items: BTreeMap::new(),
            media: BTreeMap::new(),
        }
    }

    fn catalogs(&self) -> Vec<crate::api::FranchiseCatalog> {
        build_franchise_catalogs(
            &self.items.values().cloned().collect::<Vec<_>>(),
            &self.media.values().cloned().collect::<Vec<_>>(),
        )
    }
}

impl LibraryRefreshCoordinator {
    pub const fn generation(&self) -> Option<ViewGeneration> {
        match &self.active {
            Some(active) => Some(active.generation),
            None => None,
        }
    }

    pub async fn start_if_requested(&mut self, app: &mut AppState, handle: &ResourceHandle) {
        if !app.take_library_refresh_request() || self.active.is_some() {
            return;
        }
        let queries = app.library_refresh_queries();
        if queries.is_empty() {
            return;
        }

        let generation =
            ViewGeneration::new(FIRST_LIBRARY_GENERATION.saturating_add(self.generation_offset));
        self.generation_offset = self.generation_offset.saturating_add(1);
        self.active = Some(ActiveRefresh::new(generation, queries));
        self.advance(app, handle).await;
    }

    pub async fn apply_event(
        &mut self,
        app: &mut AppState,
        handle: &ResourceHandle,
        request_id: RequestId,
        key: ResourceKey,
        result: Result<ResourceValue, LoadError>,
    ) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if !active.pending.remove(&request_id) {
            return;
        }

        let mut details_follow_up = None;
        match (key, result) {
            (
                ResourceKey::Search { query, extended },
                Ok(ResourceValue::Search(SearchResultBundle {
                    items,
                    anilist_media,
                    anilist_enrichment_failed,
                })),
            ) => {
                if anilist_enrichment_failed {
                    // A local-only projection can split a known franchise and
                    // overwrite its canonical title. Keep the existing cache
                    // instead of applying a degraded relation graph.
                    active.failed = true;
                } else {
                    let _ = app.metadata_cache.put_search(
                        &query,
                        extended,
                        items.clone(),
                        anilist_media.clone(),
                    );
                    for item in items {
                        active.items.insert(item.id, item);
                    }
                    for node in anilist_media {
                        active.media.insert(node.id, node);
                    }
                }
            }
            (ResourceKey::AniHubByAniList(_), Ok(ResourceValue::AniHubId(Some(anime_id)))) => {
                if !active.items.contains_key(&anime_id) {
                    details_follow_up = Some(ResourceKey::details(anime_id));
                }
            }
            (ResourceKey::AniHubByAniList(_), Ok(ResourceValue::AniHubId(None))) => {}
            (ResourceKey::Details(anime_id), Ok(ResourceValue::Details(details))) => {
                let _ = app.metadata_cache.put_details(details.clone());
                app.details_cache.insert(anime_id, details.clone());
                active
                    .items
                    .insert(anime_id, crate::anime_item_from_details(&details));
            }
            (_, Err(_)) => active.failed = true,
            _ => active.failed = true,
        }

        if let Some(key) = details_follow_up {
            match handle.load(active.generation, key).await {
                Ok(request_id) => {
                    active.pending.insert(request_id);
                }
                Err(_) => active.failed = true,
            }
        }
        self.advance(app, handle).await;
    }

    async fn advance(&mut self, app: &mut AppState, handle: &ResourceHandle) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if !active.pending.is_empty() && !active.searching {
            return;
        }

        if active.searching {
            while active.pending.len() < MAX_BACKGROUND_SEARCHES {
                let Some(query) = active.queued_queries.pop_front() else {
                    break;
                };
                match handle
                    .load(active.generation, ResourceKey::search(query, false))
                    .await
                {
                    Ok(request_id) => {
                        active.pending.insert(request_id);
                    }
                    Err(_) => active.failed = true,
                }
            }
            if !active.pending.is_empty() || !active.queued_queries.is_empty() {
                return;
            }
            active.searching = false;
            let unresolved = active
                .catalogs()
                .into_iter()
                .flat_map(|catalog| catalog.unresolved_anilist_ids)
                .collect::<std::collections::BTreeSet<_>>();
            for anilist_id in unresolved {
                match handle
                    .load(
                        active.generation,
                        ResourceKey::anihub_by_anilist(anilist_id),
                    )
                    .await
                {
                    Ok(request_id) => {
                        active.pending.insert(request_id);
                    }
                    Err(_) => active.failed = true,
                }
            }
            if !active.pending.is_empty() {
                return;
            }
        }

        let active = self.active.take().expect("active refresh exists");
        if let Err(error) = app.apply_library_refresh_catalogs(&active.catalogs()) {
            app.set_error_status(format!("Не вдалося зберегти оновлення бібліотеки: {error}"));
        } else if active.failed {
            app.set_info_status("Не вдалося оновити бібліотеку · показано кеш");
        }
    }
}
