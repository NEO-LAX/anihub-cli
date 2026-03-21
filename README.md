# AniHub CLI

@unofficial Terminal client for watching anime via AniHub. Built with Rust.

## Features
- Browse anime list and episodes.
- Terminal image support.
- Integrated with mpv and yt-dlp for playback.
- History and caching.

## Quick Installation (Linux & macOS)

Run the following command to install the latest version:

```bash
curl -fsSL "https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh?t=$(date +%s)" | bash
```

*After installation, restart your terminal (or run `source ~/.bashrc` / `source ~/.zshrc`). Then you can launch the app by simply typing:*

```bash
anihub-cli
```

## Windows Installation

1. Download `anihub-cli-windows.exe` from the [Latest Release](https://github.com/NEO-LAX/anihub-cli/releases/latest).
2. Rename it to `anihub-cli.exe` (optional).
3. Ensure you have `mpv` and `yt-dlp` installed and added to your `PATH`.

**Recommendation:** Use [Windows Terminal](https://aka.ms/terminal) for the best experience (including image support).

## Dependencies

To play video, you need to install:
- **mpv**
- **yt-dlp**

### Arch-based (Manjaro, EndeavourOS):
```bash
sudo pacman -S mpv yt-dlp
```

### Debian-based (Ubuntu, Linux Mint, Pop!_OS):
```bash
sudo apt update && sudo apt install mpv yt-dlp
```

### macOS:
```bash
brew install mpv yt-dlp
```

### Windows (using [Scoop](https://scoop.sh/)):
```powershell
scoop install mpv yt-dlp
```

## Build from Source

### 1. Install Rust:
[rustup.rs](https://rustup.rs/)

### 2. Clone and build:
```bash
git clone https://github.com/NEO-LAX/anihub-cli.git
cd anihub-cli
cargo build --release
```

The binary will be located at `target/release/anihub-cli` (or `anihub-cli.exe` on Windows).

## License
MIT
