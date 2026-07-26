# Changelog

Notable changes per release. Older releases are documented only in
[GitHub Releases](https://github.com/NEO-LAX/anihub-cli/releases); this file
starts at 0.8.0.

## 0.8.0

### Added

- **MoonAnime plays in mpv** (experimental, off by default — Settings → Основні
  → ІНТЕГРАЦІЇ). MoonAnime hides its stream URL behind two layers of
  obfuscation on the embed page; anihub-cli now decodes them with no extra tools
  installed. Watch progress, resume, autoplay and the native mpv playlist all
  work as they do for Ashdi. Subtitles are attached when MoonAnime provides them
  but are not switched on — `j` in mpv cycles them. When decoding fails, `o`
  still opens the episode in a browser.
- **Synopsis on `i`** — the description of the selected title, scrollable.
- **Statistics tab** in settings: episodes watched, titles per status and
  estimated watch time. mpv only reports a duration for episodes that were
  actually played, so episodes marked watched with Space have none; those are
  estimated from the median of the known durations and the total is prefixed
  with `≈` rather than presented as a measurement.
- **Stream quality preference** — Auto / highest / lowest, handed to mpv as
  `--hls-bitrate`. Not a resolution picker: the app does not parse the variant
  list. An explicit `--hls-bitrate` in your own mpv arguments still wins.
- **Library export** to a timestamped JSON file in the data directory
  (settings → Про → Експортувати бібліотеку).
- **Next-episode countdown in search results**, matching the library.
- **`? Довідка` pointer** in the bottom-right of the status bar. The help key
  was previously documented only inside the help popup itself and in the README.

### Fixed

- A dub available on both hosts under the same name showed only the Ashdi copy,
  even when MoonAnime carried far more of the run. Hunter x Hunter offered 62
  episodes instead of 146. The name merge exists to fold alias spellings
  together, so it now keeps whichever source reaches further; the episode count
  on each row tells them apart.
- Titles containing a standalone `x`, such as "Hunter x Hunter", were read as
  roman-numeral season 10. Since the mainline counter takes the maximum season
  it has seen, this would also have pushed later entries in the same franchise
  to 11 and beyond.

### Changed

- The General settings tab is grouped into ВІДТВОРЕННЯ / ІНТЕРФЕЙС /
  ІНТЕГРАЦІЇ / ПЛЕЄР instead of one flat list, and is now generated from a
  single row table so the rendered list, the selectable-row count and the
  activation dispatch cannot drift apart.
- Settings written by 0.8.0 stay readable by older versions, and settings from
  older versions load unchanged: the new keys default to their previous
  behaviour.
