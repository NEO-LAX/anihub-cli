# AniHub API — Документація

**Base URL:** `https://api.anihub.in.ua`
**Rate limit:** 40 запитів/хв з одного IP
**Auth:** Header `X-API-Key: SHA256("Ukr@in1anAn1me-S3curity-Key-2025_YYYY-MM-DD")`

---

## Аніме

### GET /anime/
Пошук та список аніме з фільтрами і пагінацією.

**Параметри:**
| Параметр | Тип | Опис |
|---|---|---|
| `page` | int | Сторінка (default: 1) |
| `page_size` | int 1-20 | Розмір сторінки (default: 10) |
| `search` | string | **Легкий пошук без fuzzy/trigram** по `title_ukrainian`, `title_english`, `title_original`, `alias` |
| `status` | string | Статус: `ongoing`, `completed`, `announced`, `paused`, `dropped` |
| `type` | string | Тип: `tv`, `movie`, `ova`, `ona`, `tv_special`, `music`, `short` |
| `year` | int | Рік виходу |
| `season_name` | string | Сезон: `winter`, `spring`, `summer`, `fall` |
| `ordering` | string | Сортування: `rating`, `-rating`, `year`, `-year`, `title_ukrainian`, тощо |
| `has_ukrainian_dub` | bool | Тільки з україномовним дубляжем |
| `mal_id` | int | Пошук за MyAnimeList ID |
| `anilist_id` | int | **Пошук за AniList ID** (прямий lookup!) |
| `imdb_id` | string | Пошук за IMDb ID |

**Відповідь:**
```json
{
  "total": 19,
  "page": 1,
  "page_size": 5,
  "total_pages": 4,
  "next_page": 2,
  "previous_page": null,
  "items": [AnimeItem]
}
```

### GET /anime/{anime_id}/
Повна картка аніме.

**Відповідь — AnimeItem:**
```json
{
  "id": 4815,
  "mal_id": 43608,
  "anilist_id": 125367,
  "slug": "vbyvtsia-romantyky",
  "title_ukrainian": "Вбивця романтики",
  "title_original": "Kaguya-sama wa Kokurasetai: Ultra Romantic",
  "title_english": "Kaguya-sama: Love is War -Ultra Romantic-",
  "status": "completed",
  "type": "tv",
  "year": 2022,
  "has_ukrainian_dub": true,
  "poster_url": "https://myanimelist.net/images/anime/...",
  "banner_url": "https://s4.anilist.co/...",
  "episodes_count": 13,
  "imdb_id": "tt9522300",
  "a": "PG-13",
  "description": "...",
  "genres": ["Сейнен", "Комедія", "Романтика"],
  "dubbing_studios": [{"id": 12, "name": "FanVoxUA", "slug": "fanwoxua", "logo_url": ""}],
  "screenshots": [],
  "youtube_trailer": "vFN5K-iAyV0",
  "rating": 8.96
}
```

### GET /anime/popular/
Популярні аніме. Параметр: `limit`.

### GET /anime/recommended/
Рекомендовані. Параметр: `limit`.

### GET /anime/seasonal/
Аніме поточного сезону. Параметр: `limit`.

### GET /anime/announced/
Анонсовані. Параметр: `limit`.

### GET /anime/newest/
Нещодавно додані. Параметр: `limit`.

### GET /anime/random/
Одне випадкове аніме.

---

## Жанри

### GET /genres/
Всі жанри з кількістю аніме.

### GET /genres/{genre_id}/
Жанр з прев'ю аніме.

**Параметри:** `preview_limit`, `show_nsfw`
**Відповідь:** `id`, `slug`, `anime_count`, `top_anime[]`, `recent_anime[]`

---

## Персонажі

### GET /characters/
Пошук персонажів. Параметри: `search`, `page`, `page_size`.

### GET /characters/{character_id}/
Профіль персонажа.

---

## Студії

### GET /studios/
Дублюючі студії (пошук + пагінація).

### GET /studios/{studio_id}/
Деталі студії.

### GET /studios/{studio_id}/anime/
Аніме студії.

### GET /animation-studios/
Анімаційні студії.

### GET /animation-studios/{studio_id}/
### GET /animation-studios/{studio_id}/anime/

---

## Розклад

### GET /airing-schedule/
Розклад виходу серій.

**Параметри:**
| Параметр | Тип | Опис |
|---|---|---|
| `start` | date | Початок діапазону |
| `end` | date | Кінець діапазону |
| `only_ukrainian` | bool | Тільки з укр. дубляжем |
| `group_by` | string | `grouped` або `flat` |

**Відповідь:** `results[]`, `window`, `total_days`/`total_count`

---

## Примітки

- Anihub зберігає `anilist_id` для більшості аніме → надійний cross-reference з AniList
- Пошук **не** є fuzzy — лише точне входження підрядка
- Типи аніме: `tv`, `movie`, `ova`, `ona`, `tv_special`, `music`, `short`
- Для TV-special (наприклад, фільм Каґуя 23630) тип = `tv_special`, не `movie`
- Каґуя S3 (ID 4815) = `anilist_id: 125367`, `episodes_count: 13`, type=`tv`

---

## Приклади запитів

```bash
# Пошук за назвою
GET /anime/?search=kaguya&has_ukrainian_dub=true

# Прямий lookup за AniList ID (найнадійніший спосіб)
GET /anime/?anilist_id=125367

# Пошук за типом
GET /anime/?type=tv&search=каґуя
```
