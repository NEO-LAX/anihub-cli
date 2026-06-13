use crate::api::{self, ApiClient, EpisodeSourcesResponse};
use moka::sync::Cache;
use std::collections::HashMap;

pub async fn compute_library_combined_sources(
    api_client: ApiClient,
    details_cache: Cache<u32, api::AnimeDetails>,
    sources_cache: Cache<u32, EpisodeSourcesResponse>,
    anilist_cache: Cache<u32, Vec<api::anilist::FranchiseMember>>,
    current_tv_ids: Vec<u32>,
    representative_id: u32,
) -> Option<(EpisodeSourcesResponse, Vec<u32>)> {
    let mut tv_with_year: Vec<(u32, u32)> = Vec::new();
    let mut id_to_anilist: HashMap<u32, u32> = HashMap::new();

    for &anime_id in &current_tv_ids {
        let details = if let Some(cached) = details_cache.get(&anime_id) {
            Some(cached)
        } else if let Ok(details) = api_client.get_anime_details(anime_id).await {
            details_cache.insert(anime_id, details.clone());
            Some(details)
        } else {
            None
        };

        let year = details.as_ref().and_then(|d| d.year).unwrap_or(0);
        tv_with_year.push((anime_id, year));
        if let Some(al_id) = details.and_then(|d| d.anilist_id) {
            id_to_anilist.insert(anime_id, al_id);
        }
    }

    let mut first_known_al_id: Option<u32> = None;
    let mut fallback_title_original: Option<String> = None;

    for &id in &current_tv_ids {
        if let Some(details) = details_cache.get(&id) {
            if first_known_al_id.is_none() {
                first_known_al_id = details.anilist_id;
            }
            if fallback_title_original.is_none() {
                fallback_title_original = details.title_original.clone();
            }
        }
    }

    let anilist_members = if let Some(cached) = anilist_cache.get(&representative_id) {
        cached
    } else {
        let members = if let Some(al_id) = first_known_al_id {
            api::anilist::get_franchise_members_by_id(api_client.http_client(), al_id).await
        } else if let Some(title_original) = fallback_title_original {
            api::anilist::get_franchise_members(api_client.http_client(), &title_original).await
        } else {
            Vec::new()
        };
        if !members.is_empty() {
            anilist_cache.insert(representative_id, members.clone());
        }
        members
    };

    let mut extra_tv: Vec<(u32, u32, u32)> = Vec::new();
    if !anilist_members.is_empty() {
        let known_ids: std::collections::HashSet<u32> = current_tv_ids.iter().copied().collect();
        for member in &anilist_members {
            if !member.is_tv { continue; }
            if let Ok(Some(anime_id)) = api_client.get_anime_by_anilist_id(member.anilist_id).await {
                if known_ids.contains(&anime_id) || extra_tv.iter().any(|(id, _, _)| *id == anime_id) {
                    continue;
                }
                extra_tv.push((anime_id, 0, member.anilist_id));
            }
        }
    }

    for (id, _, al_id) in extra_tv {
        let year = details_cache.get(&id).and_then(|d| d.year).unwrap_or(9999);
        if !tv_with_year.iter().any(|(existing_id, _)| *existing_id == id) {
            tv_with_year.push((id, year));
        }
        id_to_anilist.entry(id).or_insert(al_id);
    }

    if !anilist_members.is_empty() {
        tv_with_year.retain(|(id, _)| {
            match id_to_anilist.get(id).copied() {
                Some(al_id) => anilist_members.iter().find(|m| m.anilist_id == al_id).map(|m| m.is_tv).unwrap_or(false),
                None => true,
            }
        });
    }

    tv_with_year.sort_by(|&(a_id, a_year), &(b_id, b_year)| {
        let a_al = id_to_anilist.get(&a_id).copied().unwrap_or(u32::MAX);
        let b_al = id_to_anilist.get(&b_id).copied().unwrap_or(u32::MAX);
        a_al.cmp(&b_al).then(a_year.cmp(&b_year))
    });

    let franchise_tv_ids: Vec<u32> = tv_with_year.into_iter().map(|(id, _)| id).collect();
    if franchise_tv_ids.is_empty() { return None; }

    let multi = franchise_tv_ids.len() > 1;
    let mut combined: Vec<api::models::AshdiStudio> = Vec::new();
    let mut anime_ids: Vec<u32> = Vec::new();

    if multi {
        let mut join_set: tokio::task::JoinSet<(u32, Option<EpisodeSourcesResponse>)> = tokio::task::JoinSet::new();
        for &anime_id in &franchise_tv_ids {
            if let Some(cached) = sources_cache.get(&anime_id) {
                join_set.spawn(async move { (anime_id, Some(cached)) });
            } else {
                let client = api_client.clone();
                join_set.spawn(async move {
                    (anime_id, client.get_episode_sources_for_anime(anime_id).await.ok())
                });
            }
        }

        let mut fetched: Vec<(u32, Option<EpisodeSourcesResponse>)> = Vec::with_capacity(franchise_tv_ids.len());
        while let Some(Ok(result)) = join_set.join_next().await {
            fetched.push(result);
        }
        fetched.sort_by_key(|(id, _)| franchise_tv_ids.iter().position(|&x| x == *id).unwrap_or(usize::MAX));

        let mut all: Vec<(u32, api::models::AshdiStudio)> = Vec::new();
        let mut per_member: HashMap<u32, Vec<api::models::AshdiStudio>> = HashMap::new();
        for (anime_id, sources) in fetched {
            if let Some(sources) = sources {
                sources_cache.insert(anime_id, sources.clone());
                per_member.insert(anime_id, sources.ashdi.clone());
                for studio in sources.ashdi {
                    all.push((anime_id, studio));
                }
            }
        }

        let active_franchise_ids: Vec<u32> = franchise_tv_ids.iter().copied().filter(|id| per_member.contains_key(id)).collect();
        let mut best: Vec<(u32, api::models::AshdiStudio)> = Vec::new();
        for (aid, studio) in &all {
            if let Some(pos) = best.iter().position(|(_, s)| s.season_number == studio.season_number && s.studio_name == studio.studio_name) {
                if studio.episodes.len() >= best[pos].1.episodes.len() {
                    best[pos] = (*aid, studio.clone());
                }
            } else {
                best.push((*aid, studio.clone()));
            }
        }

        let unique_season_nums: Vec<u32> = {
            let mut s: Vec<u32> = best.iter().map(|(_, s)| s.season_number).collect();
            s.sort_unstable();
            s.dedup();
            s
        };
        let n_seasons = unique_season_nums.len();
        let n_members = active_franchise_ids.len();

        if n_members >= n_seasons {
            let mut season_counter = 1;
            let mut claimed_urls = std::collections::HashSet::new();
            for (i, &anime_id) in active_franchise_ids.iter().enumerate() {
                let member_studios = per_member.get(&anime_id).unwrap();
                let chosen = unique_season_nums.get(i).or_else(|| {
                    member_studios.iter().find(|s| s.season_number == season_counter).map(|s| &s.season_number)
                }).or_else(|| {
                    member_studios.iter().map(|s| &s.season_number).find(|s_num| {
                        member_studios.iter().filter(|s| s.season_number == **s_num).any(|s| s.episodes.first().map(|e| !claimed_urls.contains(&e.url)).unwrap_or(false))
                    })
                }).copied();
                let Some(chosen_s) = chosen else { continue };
                for studio in member_studios.iter().filter(|s| s.season_number == chosen_s) {
                    if let Some(ep) = studio.episodes.first() { claimed_urls.insert(ep.url.clone()); }
                    combined.push(api::models::AshdiStudio { season_number: season_counter, ..studio.clone() });
                    anime_ids.push(anime_id);
                }
                season_counter += 1;
            }
        } else {
            let member_offset = n_members.saturating_sub(n_seasons);
            for (data_aid, studio) in best {
                let season_pos = unique_season_nums.iter().position(|&s| s == studio.season_number).unwrap_or(0);
                let new_season = (season_pos + 1) as u32;
                let owner_id = active_franchise_ids.get(season_pos + member_offset).copied().unwrap_or_else(|| active_franchise_ids.last().copied().unwrap_or(data_aid));
                anime_ids.push(owner_id);
                combined.push(api::models::AshdiStudio { season_number: new_season, ..studio });
            }
        }
    } else {
        let anime_id = franchise_tv_ids[0];
        let sources = if let Some(cached) = sources_cache.get(&anime_id) {
            Some(cached)
        } else {
            api_client.get_episode_sources_for_anime(anime_id).await.ok()
        };
        if let Some(sources) = sources {
            sources_cache.insert(anime_id, sources.clone());
            for studio in sources.ashdi {
                anime_ids.push(anime_id);
                combined.push(studio);
            }
        }
    }

    if !combined.is_empty() {
        let final_sources = EpisodeSourcesResponse {
            ashdi: combined,
            moonanime: Vec::new(),
        };
        Some((final_sources, anime_ids))
    } else {
        None
    }
}
