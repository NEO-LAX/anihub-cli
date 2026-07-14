# AniHub CLI

AniHub CLI is an unofficial Rust terminal client for browsing and watching anime from [AniHub](https://anihub.in.ua).

![AniHub CLI demo](assets/demo.gif)

## What's new in v0.5.0

- A responsive terminal layout that gives the active panel more space and hides the poster sidebar in narrow terminals.
- A two-line contextual footer with relevant shortcuts, loading activity, selection position, version, and current playback progress.
- Editable Unicode search with cursor movement, `Home`/`End`, `Delete`, and automatic restoration of the previous query.
- Library sections for Continue, Bookmarks, Completed, and All, including bookmarks that do not have watch history yet.
- Faster long-list navigation with `j`/`k`, `Page Up`/`Page Down`, and `Home`/`End`.
- Clearer breadcrumbs, empty states, modal errors, and a fully Ukrainian in-app help screen.

## Supported functionality

- Search AniHub by title. Results are limited to entries that currently have Ukrainian dubbing in the AniHub API.
- Browse anime details, posters, seasons, dubbing options, and episodes.
- Group related seasons and films deterministically and cache loaded metadata for repeated navigation.
- Keep the interface responsive while search, posters, episode sources, and stream resolution run in bounded background workers.
- Play Ashdi streams with `mpv` and use the MoonAnime fallback through Python Playwright and headless Firefox.
- Save watch progress, bookmarks, and watched state in a local library, with resume support.
- Filter the library by unfinished titles, bookmarks, completed titles, or the complete collection.
- Show the active title, season, episode, dubbing studio, position, and duration while playback is running.
- Render posters in terminals supported by `ratatui-image` and provide a Ukrainian interface.

The application depends on the live AniHub/API and streaming pages. Search and stream availability can change when those services change.

## Installation

The installer supports Linux x86_64 and macOS x86_64/arm64. It downloads the matching release binary, verifies it against `SHA256SUMS`, and installs it to `~/.local/bin` by default.

```bash
curl --fail --location --retry 3 https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh | bash -s -- install
```

Run the installer non-interactively with `install` or `uninstall`:

```bash
curl --fail --location --retry 3 https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh | bash -s -- uninstall
```

To install into another directory, set `ANIHUB_INSTALL_DIR` before running the script. The installer never removes the history directory when uninstalling.

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

### Python Playwright and Firefox

Python is only needed for the MoonAnime extraction fallback. The binary tries `python3` and then `python` on Linux/macOS, and `py -3` and then `python` on Windows. Set `ANIHUB_PYTHON` to use a specific interpreter. It imports the Python `playwright` package and launches Playwright's Firefox browser in headless mode.

```bash
python3 -m pip install --user playwright
python3 -m playwright install firefox
```

If the system Python refuses a user install, use a virtual environment and put its `bin` directory first in `PATH` before launching AniHub CLI:

```bash
python3 -m venv "$HOME/.local/share/anihub-cli-python"
"$HOME/.local/share/anihub-cli-python/bin/python" -m pip install playwright
"$HOME/.local/share/anihub-cli-python/bin/python" -m playwright install firefox
export PATH="$HOME/.local/share/anihub-cli-python/bin:$PATH"
```

The installer and application accept `ANIHUB_PYTHON` to select the interpreter used for Playwright/Firefox, for example:

```bash
ANIHUB_PYTHON="$HOME/.local/share/anihub-cli-python/bin/python" bash install.sh install
```

No separate command-line stream extractor is required: Ashdi page parsing is implemented in Rust.

## Basic controls

The footer shows shortcuts for the current screen. Press `?` or `h` outside search input to open the complete built-in help.

### Global navigation

| Key | Action |
| --- | --- |
| `/` | Open search and restore the previous query |
| `l` | Open the library |
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
| `b` | Add or remove a bookmark |
| `x` | Toggle watched state |
| `o` | Open the selected anime in a browser |
| `d` | Delete the selected library progress after confirmation |
| `Tab` / `Shift+Tab` | Cycle library sections forward or backward |
| `1` / `2` / `3` / `4` | Select Continue / Bookmarks / Completed / All at the library root |

While editing a search query, use `Left`/`Right`, `Home`/`End`, `Backspace`, and `Delete` normally. `Enter` starts the search and `Esc` cancels it.

## History and recovery

Progress and bookmarks are stored as `history.json` under the application data directory. The current paths are:

| Platform | History file |
| --- | --- |
| Linux | `${XDG_DATA_HOME:-$HOME/.local/share}/anihub-cli/history.json` |
| macOS | `$HOME/Library/Application Support/com.shadowgarden.anihub-cli/history.json` |
| Windows | `%LOCALAPPDATA%\\shadowgarden\\anihub-cli\\data\\history.json` |

The installer only replaces the executable; uninstall leaves this data in place. Back it up before manual recovery:

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
- MoonAnime does not extract a stream: check `command -v python3`, then run `python3 -c 'from playwright.async_api import async_playwright'` and `python3 -m playwright install firefox`.
- The installer reports an unsupported platform: release installation currently supports only Linux x86_64 and macOS x86_64/arm64. Windows uses the downloadable release asset.
- Search returns no entries: verify network access and remember that the client filters API results to entries with Ukrainian dubbing.
- Images are missing: use a terminal with image protocol support or continue using the text interface; playback and history do not depend on poster rendering.
- A source page or API is unavailable: the client cannot repair upstream outages or changes to AniHub, AniList, Ashdi, or MoonAnime.

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
