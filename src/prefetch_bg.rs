//! Compatibility and prefetch helpers.
//!
//! New callers should use [`PrefetchApi`], whose surface is deliberately
//! limited to metadata and poster requests.  The legacy combine function is
//! retained for the untouched main module, but it only combines responses
//! already present in `sources_cache`; it never starts episode-source I/O.

use crate::api::resource::{PrefetchHandle, ResourceHandle};
use crate::api::{self, ApiClient, EpisodeSourcesResponse};
use moka::sync::Cache;

pub type PrefetchApi = PrefetchHandle;

pub fn new_prefetch_api(resources: ResourceHandle) -> PrefetchApi {
    resources.prefetch()
}

/// Compatibility wrapper for the old main-module call site.
///
/// Source loading belongs to an explicit on-demand request.  This function
/// only merges source responses that have already been cached.
pub async fn compute_library_combined_sources(
    _api_client: ApiClient,
    _details_cache: Cache<u32, api::AnimeDetails>,
    sources_cache: Cache<u32, EpisodeSourcesResponse>,
    _anilist_cache: Cache<u32, Vec<api::anilist::FranchiseMember>>,
    current_tv_ids: Vec<u32>,
    _representative_id: u32,
) -> Option<(EpisodeSourcesResponse, Vec<u32>)> {
    let franchise_order = current_tv_ids.clone();
    let cached_results = current_tv_ids
        .into_iter()
        .filter_map(|anime_id| {
            sources_cache
                .get(&anime_id)
                .map(|sources| (anime_id, sources))
        })
        .collect::<Vec<_>>();

    api::combine_franchise_sources_legacy(&franchise_order, &cached_results)
}
