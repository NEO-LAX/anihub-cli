# AniHub CLI

Terminal client for watching anime via AniHub. Built with Rust.

## Features
- Browse anime list and episodes.
- Terminal image support.
- Integrated with mpv and yt-dlp for playback.
- History and caching.

## Quick Installation

Run the following command to install the latest version:

```bash
curl -fsSL "https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh?t=$(date +%s)" | bash
```

*After installation, restart your terminal (or run `source ~/.bashrc` / `source ~/.zshrc`). Then you can launch the app by simply typing:*

```bash
anihub-cli
```

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

## Build from Source

### 1. Install Rust:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. Install system dependencies:

**Arch Linux:**
```bash
sudo pacman -S base-devel pkgconf
```

**Debian/Ubuntu:**
```bash
sudo apt update && sudo apt install build-essential pkg-config
```

### 3. Clone and build:
```bash
git clone https://github.com/NEO-LAX/anihub-cli.git
cd anihub-cli
cargo build --release
```

The binary will be located at `target/release/anihub-cli`.

## License
MIT
