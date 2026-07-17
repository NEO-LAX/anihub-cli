<div align="center">

# AniHub CLI

[![English](https://img.shields.io/badge/🇬🇧_English-0d1117?style=for-the-badge&labelColor=238636)](README.md)
[![Українська](https://img.shields.io/badge/🇺🇦_Українська-161b22?style=for-the-badge&labelColor=30363d)](README.uk.md)

<br/>

**Unofficial terminal client** written in **Rust** 🦀  
for browsing and watching anime from [**AniHub**](https://anihub.in.ua)

Ukrainian dubs · local library · mpv playback · Discord presence

<br/>

[![Rust](https://img.shields.io/badge/Rust-000000?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/Linux%20%7C%20macOS%20%7C%20Windows-111827?style=flat-square)](https://github.com/NEO-LAX/anihub-cli/releases/latest)
[![Release](https://img.shields.io/github/v/release/NEO-LAX/anihub-cli?style=flat-square&color=a855f7)](https://github.com/NEO-LAX/anihub-cli/releases)

</div>

---

<div align="center">

### 🎬 From search to playback

Search AniHub, pick a season and dub, hit play — streams open in **mpv** with a native playlist.

![Demo](assets/demo.gif)

</div>

---

<div align="center">

### 📚 Library

Statuses (Watching, Planned, Completed…), filters, resume, and continue-watching — all offline-friendly.

<img src="assets/library.jpg" alt="Library" width="900" />

</div>

---

<div align="center">

### 🎨 Themes

Original AniHub RGB palette by default. Optional ANSI 16 / ANSI 256 modes with curated palettes  
(Catppuccin, Tokyo Night, Kanagawa, Rosé Pine, Gruvbox, Everforest, Ayu…).

<img src="assets/themes.jpg" alt="Themes" width="900" />

</div>

---

<div align="center">

### 📺 Terminal + mpv

Ashdi episodes run in **mpv** (prev / next via the native playlist).  
Browser-only MoonAnime titles open in your browser after confirmation.

<img src="assets/mpv-terminal.jpg" alt="mpv and terminal" width="900" />

</div>

---

<div align="center">

### ▶️ Continue watching

One key (`c`) jumps back to your latest unfinished episode.

![Continue](assets/continue.gif)

</div>

---

<div align="center">

### 💬 Discord Rich Presence

Opt-in. Shows title, season, episode, studio, poster, and a Spotify-style progress bar while playing.  
On pause the bar hides and the status shows **Пауза**. Desktop Discord only.

<img src="assets/discord.jpg" alt="Discord Rich Presence" width="480" />

</div>

---

## ✨ Features

| | |
| :--- | :--- |
| 🔍 **Search** | Strict (≤20) or extended (≤100) · franchise grouping for seasons & films |
| 📖 **Library** | Statuses, filters, resume timestamps, watched toggle |
| ▶️ **Playback** | Ashdi → mpv · MoonAnime → browser · autoplay next |
| 🖼 **Posters** | Kitty / iTerm2 / Sixel / halfblocks when the terminal supports it |
| 🎨 **Themes** | AniHub RGB + ANSI 16/256 palettes · surface & transparency controls |
| 💬 **Discord** | Rich Presence with progress bar (opt-in) |
| 💾 **Caches** | Metadata SWR cache · ~150 MiB poster cache with prune |
| ⌨️ **Keys** | Shortcuts work on **EN** and **UA/RU (ЙЦУКЕН)** layouts |

> Full changelog → [GitHub Releases](https://github.com/NEO-LAX/anihub-cli/releases)

---

## 📦 Install

### Interactive installer (Linux / macOS)

Arrow-key menu: **Install · Update · Uninstall** (optional data purge).  
Downloads a checksum-verified release binary and migrates local data safely.

<div align="center">
  <img src="assets/installer.png" alt="Interactive installer" width="480" />
</div>

```bash
curl --fail --location --retry 3 \
  https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh | bash
```

```bash
# non-interactive
bash -s -- update
bash -s -- uninstall          # keep history & settings
bash -s -- uninstall --purge  # wipe all user data
```

Default path: `~/.local/bin` · override with `ANIHUB_INSTALL_DIR`.

### Nix

```bash
nix run github:NEO-LAX/anihub-cli
# nix profile install github:NEO-LAX/anihub-cli
```

### Release binaries

| Platform | Asset |
| --- | --- |
| Linux x86_64 | `anihub-cli-x86_64-unknown-linux-gnu` |
| macOS Intel | `anihub-cli-x86_64-apple-darwin` |
| macOS Apple silicon | `anihub-cli-aarch64-apple-darwin` |
| Windows x86_64 | `anihub-cli-x86_64-pc-windows-msvc.exe` |

Windows: grab the asset from [Releases](https://github.com/NEO-LAX/anihub-cli/releases/latest) and put it on `PATH`.

---

## 🔧 Requirements

- **`mpv`** on `PATH` (Ashdi playback)
- A modern terminal (image protocols optional)
- Discord **desktop** app if you enable Rich Presence

```bash
# Debian / Ubuntu
sudo apt install mpv

# macOS
brew install mpv
```

---

## ⌨️ Controls

Footer shows context-aware hints. Press `?` or `h` for full help.

| Key | Action |
| --- | --- |
| `1` `2` `3` | Search · Library · Settings |
| `/` | Search AniHub / filter local library |
| `↑` `↓` · `k` `j` | Move selection |
| `Enter` · `→` | Open / play |
| `Esc` · `←` | Back (`Esc` on search root clears results) |
| `c` | Continue watching |
| `e` | Set library status |
| `Space` | Toggle watched |
| `o` | Open in browser |
| `r` | Retry after a network error |
| `q` | Quit |

While typing a query, digits stay insertable — switch tabs with `Alt`/`Ctrl` + `1`–`3`.

---

## 🗂 Settings & data

Stored in the platform data directory:

| OS | Path |
| --- | --- |
| Linux | `~/.local/share/anihub-cli/` |
| macOS | `~/Library/Application Support/com.shadowgarden.anihub-cli/` |
| Windows | `%LOCALAPPDATA%\shadowgarden\anihub-cli\data\` |

Files: `settings.json`, `history.json`, metadata & poster caches.  
Options include autoplay, resume threshold, search mode, themes, mpv path/args, Discord, poster cache clear, and in-app update check (never auto-installs).

---

## 🛠 Build from source

Rust **1.85+**:

```bash
git clone https://github.com/NEO-LAX/anihub-cli.git
cd anihub-cli
cargo build --locked --release
```

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

---

<div align="center">

**Unofficial** · depends on live AniHub / stream sources  
Uninstall keeps your data unless you pass `--purge`

[MIT License](LICENSE) · [Releases](https://github.com/NEO-LAX/anihub-cli/releases) · [Issues](https://github.com/NEO-LAX/anihub-cli/issues)

</div>
