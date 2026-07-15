use super::models::AnimeItem;
use std::collections::HashSet;

/// Remove duplicate AniHub search rows while preserving response order.
pub fn deduplicate_anime(items: Vec<AnimeItem>) -> Vec<AnimeItem> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.id))
        .collect()
}

/// Conservative display-name fallback for callers without an enriched
/// [`FranchiseCatalog`](crate::api::FranchiseCatalog).
///
/// This function never decides franchise membership; relation-graph grouping
/// lives in `api::franchise`.
pub fn franchise_display_name<'a>(items: &'a [AnimeItem], group: &[usize]) -> &'a str {
    group
        .iter()
        .filter_map(|&index| items.get(index))
        .map(|item| item.title_ukrainian.as_str())
        .min_by_key(|title| title.len())
        .unwrap_or("")
}
