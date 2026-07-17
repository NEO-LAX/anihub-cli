# AniHub CLI

**English** · [Українська](README.uk.md)

Unofficial terminal client for [AniHub](https://anihub.in.ua) — search, library, and playback with Ukrainian dubs.

![Demo](assets/demo.gif)

<p align="center">
  <img src="assets/installer.png" alt="Installer" width="520" />
</p>

---

## Features

- Search AniHub (strict or extended) with franchise grouping for seasons and films
- Library with statuses, filters, resume, and continue-watching
- Ashdi playback in **mpv** (native playlist, prev/next); MoonAnime opens in the browser
- Posters in capable terminals · themes (AniHub RGB + ANSI 16/256 palettes)
- Optional **Discord Rich Presence** with Spotify-style episode progress
- Offline-friendly metadata and poster caches
- Shortcuts work on **English and Ukrainian/Russian** keyboard layouts

> Changelog lives in [GitHub Releases](https://github.com/NEO-LAX/anihub-cli/releases) — not here.

---

## Install

**Linux / macOS** (interactive menu: Install · Update · Uninstall):

```bash
curl --fail --location --retry 3 \
  https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh | bash
```

```bash
# non-interactive
bash -s -- update
bash -s -- uninstall          # keep user data
bash -s -- uninstall --purge  # wipe user data
```

Default install path: `~/.local/bin` (override with `ANIHUB_INSTALL_DIR`).

**Nix:**

```bash
nix run github:NEO-LAX/anihub-cli
# or: nix profile install github:NEO-LAX/anihub-cli
```

**Windows:** download the asset from [Releases](https://github.com/NEO-LAX/anihub-cli/releases/latest) and put it on `PATH`.

| Platform | Asset |
| --- | --- |
| Linux x86_64 | `anihub-cli-x86_64-unknown-linux-gnu` |
| macOS Intel | `anihub-cli-x86_64-apple-darwin` |
| macOS Apple silicon | `anihub-cli-aarch64-apple-darwin` |
| Windows x86_64 | `anihub-cli-x86_64-pc-windows-msvc.exe` |

---

## Requirements

- **`mpv`** on `PATH` (Ashdi playback)
- A modern terminal (image protocols optional; text UI always works)
- Discord **desktop** client only if you enable Rich Presence

```bash
# Debian/Ubuntu
sudo apt install mpv

# macOS
brew install mpv
```

---

## Controls

Footer hints follow the current screen. `?` / `h` opens full help.

| Key | Action |
| --- | --- |
| `1` `2` `3` | Search · Library · Settings |
| `/` | Search AniHub, or filter the local library |
| `↑` `↓` · `k` `j` | Move selection |
| `Enter` · `→` | Open / play |
| `Esc` · `←` | Back (`Esc` on search root clears results) |
| `c` | Continue watching |
| `e` | Library status |
| `Space` | Toggle watched |
| `o` | Open in browser |
| `r` | Retry after a network error |
| `q` | Quit |

While typing a query, digits stay insertable; switch tabs with `Alt`/`Ctrl` + `1`–`3`.

---

## Settings & data

Persisted under the platform data directory (`settings.json`, `history.json`, caches):

| | Path |
| --- | --- |
| Linux | `~/.local/share/anihub-cli/` |
| macOS | `~/Library/Application Support/com.shadowgarden.anihub-cli/` |
| Windows | `%LOCALAPPDATA%\shadowgarden\anihub-cli\data\` |

Notable options: autoplay, resume threshold, search mode, themes, mpv path/args, Discord presence, poster cache clear, update check (never auto-installs).

---

## Build

Rust **1.85+**:

```bash
git clone https://github.com/NEO-LAX/anihub-cli.git
cd anihub-cli
cargo build --locked --release
```

Binary: `target/release/anihub-cli`

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

---

## Notes

- Unofficial client; depends on live AniHub / stream sources.
- Installer script: Linux & macOS only. Windows uses release binaries.
- Uninstall keeps data unless you pass `--purge`.

## License

[MIT](LICENSE)
