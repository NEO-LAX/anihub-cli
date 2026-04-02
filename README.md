# 🎬 AniHub CLI

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org/)

@unofficial Terminal client for watching anime via [AniHub](https://anihub.in.ua). Built with Rust for speed and simplicity.

## 📺 Preview

![AniHub CLI Demo](assets/demo.gif)

---

## ✨ Features
- **Browse** the full AniHub catalog, genres, and characters.
- **Instant Navigation:** Advanced caching and background prefetching for zero-latency browsing.
- **Smart Flow:** Automatically skips season selection for movies and single-season entries.
- **Rich Metadata:** View release years and detailed information instantly.
- **Terminal Image Support:** Supports Kitty, WezTerm, and Windows Terminal.
- **Smooth Playback:** Powered by `mpv`, `yt-dlp`, and Playwright for complex stream extraction.
- **History & Progress:** O(1) history indexing ensures fast resume even with large libraries.
- **Ukrainian Interface** support.

---

## Quick Installation preview
![Interface Preview](assets/script-screenshot.png)


## 🚀 Quick Installation (Linux & macOS)

Run the following command to install the latest version automatically:

```bash
curl -fsSL "[https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh?t=$(date](https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh?t=$(date) +%s)" | bash
```

*After installation, restart your terminal or run `source ~/.bashrc` (or `source ~/.zshrc`).*

**Launch the app:**
```bash
anihub-cli
```

---

## 🪟 Windows Installation

1. Download `anihub-cli-windows.exe` from the [Latest Release](https://github.com/NEO-LAX/anihub-cli/releases/latest).
2. Rename it to `anihub-cli.exe` (optional).
3. Add the folder containing the `.exe` to your system **PATH**.
4. Ensure you have `mpv` and `yt-dlp` installed.

> **Recommendation:** Use [Windows Terminal](https://aka.ms/terminal) for the best experience.

---

## 🛠 Dependencies

To play all sources (including MoonAnime), you need:
- **[mpv](https://mpv.io/)** (Media player)
- **[yt-dlp](https://github.com/yt-dlp/yt-dlp)** (Stream handler)
- **[Playwright](https://playwright.dev/python/)** (Required for MoonAnime stream extraction)

### Install commands:

| OS | Command |
| :--- | :--- |
| **Arch Linux** | `sudo pacman -S mpv yt-dlp python-playwright && playwright install firefox` |
| **Ubuntu/Debian** | `sudo apt update && sudo apt install mpv yt-dlp python3-pip && pip install playwright && playwright install firefox` |
| **macOS** | `brew install mpv yt-dlp playwright && playwright install firefox` |
| **Windows** | `scoop install mpv yt-dlp && pip install playwright && playwright install firefox` |

---

## 🏗 Build from Source

1. **Install Rust:** [rustup.rs](https://rustup.rs/)
2. **Clone and build:**
   ```bash
   git clone [https://github.com/NEO-LAX/anihub-cli.git](https://github.com/NEO-LAX/anihub-cli.git)
   cd anihub-cli
   cargo build --release
   ```
The binary will be located at `target/release/anihub-cli`.

---

## 📄 License
This project is licensed under the **MIT License**.
