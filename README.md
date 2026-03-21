# AniHub CLI

Термінальний клієнт для перегляду аніме через AniHub. Побудований на Rust з використанням TUI (Ratatui).

## Особливості
- Перегляд списку доступних аніме та серій.
- Підтримка зображень прямо в терміналі (завдяки `ratatui-image`).
- Інтеграція з `mpv` та `yt-dlp` для відтворення.
- Кешування даних та історія переглядів.

## Швидке встановлення (Binary)

Якщо ви використовуєте Linux (x86_64), ви можете встановити програму однією командою:

```bash
curl -fsSL https://raw.githubusercontent.com/NEO-LAX/anihub-cli/master/install.sh | bash
```
*Ця команда запустить інтерактивний інсталятор з вибором опцій.*

## Залежності (Runtime)

Для відтворення відео необхідно встановити:
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

## Збірка з сирців

### 1. Встановіть Rust:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. Встановіть системні залежності для білда:

**Arch Linux:**
```bash
sudo pacman -S base-devel pkgconf
```

**Debian/Ubuntu:**
```bash
sudo apt update && sudo apt install build-essential pkg-config
```

### 3. Клонуйте та зберіть:
```bash
git clone https://github.com/NEO-LAX/anihub-cli.git
cd anihub-cli
cargo build --release
```

Бінарник буде знаходитись у `target/release/anihub-cli`.

## Ліцензія
MIT
