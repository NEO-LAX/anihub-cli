# AniHub CLI

AniHub CLI is an unofficial Rust terminal client for browsing and watching anime from [AniHub](https://anihub.in.ua).

![AniHub CLI demo](assets/demo.gif)

## Unreleased

- The original AniHub RGB theme remains the default, with an opt-in ANSI mode offering Violet, Ocean, Amber, and Monochrome palettes.
- Optional Discord Rich Presence for mpv/Ashdi playback, including title, season, episode, dubbing studio, elapsed time, and poster artwork.
- Persistent 150 MiB poster cache with offline fallback, automatic pruning, corruption recovery, and an explicit clear action in Settings.

## What's new in v0.6.0

- Persistent **Settings** (General / About): autoplay, resume, watched threshold slider, startup screen, library filter, posters, custom `mpv` path/args.
- In-app **GitHub update check** with a confirmation popup (never installs automatically).
- Interactive **installer menu** (Install / Update / Uninstall, optional data purge) plus `--migrate-data` validation on update.
- Library and history use stable `history.json` with automatic import from older formats.
- Strict AniHub search (20 results) plus an optional extended mode (up to 100 results).
- Persistent stale-while-revalidate metadata cache for instant repeat searches and offline fallback.
- Polished TUI: tabs, centered dialogs, status editor, contextual footer with `anihub-cli` branding.
- `Esc` steps back through panels; on the search root it clears the result list to a clean home.

## Supported functionality

- Search AniHub by title in strict mode (one page, at most 20 Ukrainian-dubbed entries, tight title matching) or enable extended mode in Settings (up to five pages / 100 entries).
- Browse anime details, posters, seasons, dubbing options, and episodes.
- Group related seasons and films deterministically and persist search graphs/details for fast repeat navigation.
- Keep the interface responsive while search, posters, episode sources, and stream resolution run in bounded background workers.
- Play Ashdi streams with `mpv`; browser-only MoonAnime episodes open their direct embed after confirmation.
- Save watch progress, watched state, and explicit anime statuses in a local library, with resume support.
- Filter the library by Watching, Planned, Completed, On Hold, Dropped, or the complete collection.
- Show the active title, season, episode, dubbing studio, position, and duration while playback is running.
- Render posters in terminals supported by `ratatui-image` and provide a Ukrainian interface.
- Publish the current mpv/Ashdi episode to Discord Rich Presence when the user explicitly enables it.

The application depends on the live AniHub/API and streaming pages. Search and stream availability can change when those services change.

## Installation

The installer supports Linux x86_64 and macOS x86_64/arm64. It downloads the matching release binary, verifies it against `SHA256SUMS`, validates/migrates local data, and installs it to `~/.local/bin` by default. Run it without an action to open the arrow-key menu; it shows **Install** for a fresh setup and **Update / Uninstall** when the binary already exists.

![AniHub CLI installer](assets/installer.png)

```bash
curl --fail --location --retry 3 https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh | bash
```

Use `↑`/`↓` (or `j`/`k`) and `Enter` in the menu. Uninstall asks whether to keep or delete local history/settings. Automation can pass `install`, `update`, or `uninstall` explicitly:

```bash
curl --fail --location --retry 3 https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh | bash -s -- update

# Remove the app but keep history and settings
curl --fail --location --retry 3 https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh | bash -s -- uninstall

# Remove the app and all AniHub CLI user data
curl --fail --location --retry 3 https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh | bash -s -- uninstall --purge
```

To install into another directory, set `ANIHUB_INSTALL_DIR` before running the script. The installer runs the downloaded, checksum-verified binary in `--migrate-data` mode before replacing the installed executable. A failed validation leaves the current executable and source data intact. User data is deleted only after selecting **Delete user data** or passing the explicit `uninstall --purge` option.

### Nix

With flakes enabled, run AniHub CLI directly from GitHub:

```bash
nix run github:NEO-LAX/anihub-cli
```

The flake supports x86_64/aarch64 Linux and Intel/Apple-silicon macOS. It builds the locked Rust dependencies, provides `mpv` on the application's runtime `PATH`, and configures the Nix CA certificate bundle. To install it into your Nix profile instead of running it ephemerally:

```bash
nix profile install github:NEO-LAX/anihub-cli
```

After installation, make sure the install directory is in `PATH`:

```bash
export PATH="$HOME/.local/bin:$PATH"
anihub-cli
```

To make this permanent, add the export to `~/.profile` for bash or `~/.zprofile` for zsh, then open a new shell.

### Release binaries

| Platform | Release asset |
| --- | --- |
| Linux x86_64 | `anihub-cli-x86_64-unknown-linux-gnu` |
| macOS Intel | `anihub-cli-x86_64-apple-darwin` |
| macOS Apple silicon | `anihub-cli-aarch64-apple-darwin` |
| Windows x86_64 | `anihub-cli-x86_64-pc-windows-msvc.exe` |

Windows users can download the Windows asset from the [latest release](https://github.com/NEO-LAX/anihub-cli/releases/latest), put it in a directory on `PATH`, and launch it from Windows Terminal or another capable terminal. The installer script itself is for Linux and macOS.

## Runtime dependencies

### `mpv`

`mpv` is required for episode playback. AniHub CLI starts the `mpv` executable directly, so it must be available in `PATH`.

Examples:

```bash
# Debian/Ubuntu
sudo apt update && sudo apt install mpv

# macOS with Homebrew
brew install mpv
```

No separate command-line stream extractor is required: Ashdi page parsing is implemented in Rust.

## Basic controls

The footer shows shortcuts for the current screen. Press `?` or `h` outside search input to open the complete built-in help.

### Global navigation

| Key | Action |
| --- | --- |
| `1` / `2` / `3` | Switch Search / Library / Settings (does not open the search editor) |
| `Alt+1` / `Alt+2` / `Alt+3` or `Ctrl+1`… | Same tab switch while the search field is focused |
| `/` | Search AniHub from Search, or filter only the local library from Library |
| `?` or `h` | Open help |
| `Up` / `Down` or `k` / `j` | Move through the active list |
| `Page Up` / `Page Down` | Move ten entries at a time |
| `Home` / `End` | Jump to the beginning or end of the active list |
| `Right` or `Enter` | Open the selected level or play the selected episode |
| `Left` or `Esc` | Return to the previous level |
| `q` | Save final playback progress, stop owned processes, and quit |
| `Ctrl+C` | Quit from any screen, including search input |

### Anime and library actions

| Key | Action |
| --- | --- |
| `c` | Continue the latest unfinished episode |
| `e` | Choose Not Added / Planned / Watching / Completed / On Hold / Dropped |
| `Space` | Toggle watched state for the selected season or episode, including browser-only MoonAnime episodes |
| `Backspace` | Clear only the selected episode's resume timestamp |
| `o` | Open the selected anime in a browser |
| `d` | Delete the selected library progress after confirmation |
| `Tab` / `Shift+Tab` | Cycle library status filters forward or backward |

Library search is local and immediate: press `/` anywhere inside Library, type part of a title, and press `Enter` to keep the filter. `Esc` clears it. It never starts an AniHub network search.

While editing a search query, use `Left`/`Right`, `Home`/`End`, `Backspace`, and `Delete` normally. `Enter` starts the search and `Esc` cancels it. Digits stay typeable; switch tabs with `Alt+1`/`Alt+2`/`Alt+3` (or `Ctrl`) without leaving the editor first.

### Settings

Settings are persisted in `settings.json` beside the history file. Existing `settings-v1.json` data is imported automatically and retained as a safety copy. In the Settings screen, use `Tab` / `Shift+Tab` to switch General, Themes, and About; use `Up`/`Down` to select a row and `Space` or `Enter` to change it. **Search mode** switches between strict (20 results) and extended (up to 100 results). The original AniHub RGB palette is the default. The Themes tab can explicitly enable ANSI colors and choose a terminal-native palette that respects the terminal's own color configuration. Text values for the Discord Application ID, `mpv` path, and extra arguments open a small editor; `Enter` saves and `Esc` cancels. About shows data paths and runtime diagnostics, clears the disposable poster cache, opens the project/data directory on explicit action, and checks the latest GitHub release without installing anything automatically.

### Discord Rich Presence

Discord integration is opt-in and only describes playback owned by AniHub CLI through mpv/Ashdi. Browser-only MoonAnime episodes are intentionally excluded because the CLI cannot reliably observe browser pause, progress, or completion.

1. Create an application in the [Discord Developer Portal](https://discord.com/developers/applications).
2. Copy its numeric **Application ID**. This ID is public; never paste a bot token or client secret into AniHub CLI.
3. Open **Settings → General**, enter the Application ID, and enable **Discord Rich Presence**.
4. Keep the Discord desktop client running with activity sharing enabled.

The IPC connection runs on a background thread, reconnects when Discord starts later, and never blocks the TUI. Closing playback or AniHub CLI clears the activity. Discord web/mobile clients cannot receive local Rich Presence updates.

## Metadata cache

Successful searches are stored in `metadata-cache.json` together with the AniList relationship graph used to reconstruct seasons, cours, films, and extras. Release details are cached by AniHub ID. On a repeated search the disk snapshot is rendered immediately, while AniHub/AniList are refreshed in the background. If that refresh fails, the cached result remains usable. Details are considered fresh for 24 hours; entries older than 30 days are pruned, and the cache is bounded to 64 searches and 500 detail records.

The cache never contains watch history, library statuses, M3U8 URLs, MoonAnime iframe URLs, or authentication data. Deleting `metadata-cache.json` only makes the next lookup slower; `history.json` and `settings.json` remain untouched. Corrupt cache files are preserved with a `.corrupt-*` suffix and rebuilt automatically.

## Poster cache

Downloaded poster response bytes are stored under the `posters` directory beside `history.json`. A repeat visit decodes the local file before attempting the network, so cached metadata and artwork remain useful during a temporary outage. The cache is limited to 150 MiB and prunes the oldest files automatically. A corrupt image is deleted and fetched again without affecting history or settings.

Use **Settings → About → Clear poster cache** to remove it. Clearing the poster cache never removes library progress, statuses, settings, or the metadata relationship cache.

## History and recovery

Progress and library statuses use the v2 schema stored under the stable `history.json` filename. On update, `history-v2.json`, schema-v1 history, and the original unversioned `history.json` format are imported automatically. Legacy files are retained, and an in-place schema migration keeps the original bytes in `history.json.bak` before writing the canonical format.

| Platform | History file |
| --- | --- |
| Linux | `${XDG_DATA_HOME:-$HOME/.local/share}/anihub-cli/history.json` |
| macOS | `$HOME/Library/Application Support/com.shadowgarden.anihub-cli/history.json` |
| Windows | `%LOCALAPPDATA%\\shadowgarden\\anihub-cli\\data\\history.json` |

Normal uninstall leaves this data in place; the interactive purge option and `uninstall --purge` remove the complete AniHub CLI data directory. Back it up before purging or manual recovery:

```bash
cp "/path/to/history.json" "/path/to/history.json.backup"
```

Writes are atomic and the previous valid file is retained as `history.json.bak`. If the primary JSON is damaged, the application preserves it with a `.corrupt-*` suffix and restores the valid backup automatically. If both files are damaged, startup reports an error instead of silently replacing the library. For manual recovery, quit the application and restore a known-good copy:

```bash
mv "/path/to/history.json" "/path/to/history.json.corrupt"
cp "/path/to/history.json.bak" "/path/to/history.json"
```

If no valid backup exists, keep the corrupt files for manual JSON recovery before creating a new history file.

## Troubleshooting

- `anihub-cli: command not found`: add `~/.local/bin` (or your `ANIHUB_INSTALL_DIR`) to `PATH`, then start a new shell.
- Playback reports that `mpv` cannot start: install `mpv` and confirm `command -v mpv` prints its path.
- MoonAnime episodes are browser-only; confirm the popup to open the selected direct embed.
- The installer reports an unsupported platform: release installation currently supports only Linux x86_64 and macOS x86_64/arm64. Windows uses the downloadable release asset.
- Search returns no entries: verify network access and remember that the client filters API results to entries with Ukrainian dubbing.
- Images are missing: use a terminal with image protocol support or continue using the text interface; playback and history do not depend on poster rendering.
- Discord status is missing: use the desktop Discord client, enable activity sharing, and verify that Settings contains a numeric Application ID. Browser/mobile Discord cannot receive local IPC updates.
- A source page or API is unavailable: the client cannot repair upstream outages or changes to AniHub, AniList, or Ashdi.

## Build from source

Install Rust 1.85 or newer with [rustup](https://rustup.rs/), then clone and build:

```bash
git clone https://github.com/NEO-LAX/anihub-cli.git
cd anihub-cli
cargo build --locked --release
```

The binary is written to `target/release/anihub-cli` (or `anihub-cli.exe` on Windows).

Before submitting a change, run the same core checks as CI:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
bash -n install.sh tests/install/test_install.sh
bash tests/install/test_install.sh
```

## License

AniHub CLI is released under the [MIT License](LICENSE).
