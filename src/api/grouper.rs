use std::collections::HashSet;
use super::models::AnimeItem;

/// Видаляє дублікати по `id`.
pub fn deduplicate_anime(items: Vec<AnimeItem>) -> Vec<AnimeItem> {
    let mut seen = HashSet::new();
    items.into_iter().filter(|item| seen.insert(item.id)).collect()
}

/// Групує індекси в `items` за франшизою через prefix-matching назв.
/// Кожна група відсортована за роком (від старого до нового).
pub fn group_into_franchises(items: &[AnimeItem]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();

    'outer: for (i, item) in items.iter().enumerate() {
        for group in &mut groups {
            if same_franchise(&items[group[0]].title_ukrainian, &item.title_ukrainian) {
                group.push(i);
                continue 'outer;
            }
        }
        groups.push(vec![i]);
    }

    for group in &mut groups {
        group.sort_by_key(|&i| items[i].year.unwrap_or(0));
    }

    groups
}

/// Індекс найкращого представника групи для завантаження джерел серій.
/// Перевага: найновіший TV-запис; fallback: останній за роком.
pub fn representative_idx(items: &[AnimeItem], group: &[usize]) -> usize {
    for &idx in group.iter().rev() {
        let t = items[idx].anime_type.to_lowercase();
        if !t.contains("ova") && !t.contains("спец") && !t.contains("special") {
            return idx;
        }
    }
    *group.last().unwrap()
}

/// Базова назва франшизи — найкоротший рядок у групі.
pub fn franchise_display_name<'a>(items: &'a [AnimeItem], group: &[usize]) -> &'a str {
    group.iter()
        .map(|&i| items[i].title_ukrainian.as_str())
        .min_by_key(|s| s.len())
        .unwrap_or("")
}

/// Базовий префікс назви до першого ':' або '?', без числових суфіксів.
/// "Реінкарнація безробітного 2: ..." → "Реінкарнація безробітного"
fn franchise_base(s: &str) -> &str {
    let end = s.find(':').or_else(|| s.find('?')).unwrap_or(s.len());
    s[..end].trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ')
}

fn same_franchise(a: &str, b: &str) -> bool {
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if longer.starts_with(shorter) {
        let rest = &longer[shorter.len()..];
        return rest.is_empty() || rest.starts_with(' ') || rest.starts_with(':') || rest.starts_with('?');
    }
    // Кейс "Назва: ..." ↔ "Назва 2: ..." або "Назва II: ..." — порівнюємо базу до ':'
    let ba = franchise_base(a);
    let bb = franchise_base(b);
    ba.len() >= 15 && ba == bb
}
