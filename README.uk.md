# AniHub CLI

[English](README.md) · **Українська**

Неофіційний термінальний клієнт для [AniHub](https://anihub.in.ua) — пошук, бібліотека й перегляд з українським дубляжем.

![Демо](assets/demo.gif)

<p align="center">
  <img src="assets/installer.png" alt="Інсталер" width="520" />
</p>

---

## Можливості

- Пошук на AniHub (строгий або розширений) з групуванням франшиз (сезони, фільми)
- Бібліотека зі статусами, фільтрами, resume та «продовжити перегляд»
- Відтворення Ashdi в **mpv** (нативний плейлист, prev/next); MoonAnime — у браузері
- Постери в підтримуваних терміналах · теми (AniHub RGB + ANSI 16/256)
- Опційний **Discord Rich Presence** з прогресом серії як у Spotify
- Кеш метаданих і постерів (зручно офлайн / при повторних запитах)
- Шорткати працюють на **англійській та українській/російській** розкладках

> Список змін — у [GitHub Releases](https://github.com/NEO-LAX/anihub-cli/releases), не в README.

---

## Встановлення

**Linux / macOS** (меню: Встановити · Оновити · Видалити):

```bash
curl --fail --location --retry 3 \
  https://raw.githubusercontent.com/NEO-LAX/anihub-cli/main/install.sh | bash
```

```bash
# без меню
bash -s -- update
bash -s -- uninstall          # залишити дані
bash -s -- uninstall --purge  # стерти дані
```

Типовий шлях: `~/.local/bin` (змінити: `ANIHUB_INSTALL_DIR`).

**Nix:**

```bash
nix run github:NEO-LAX/anihub-cli
# або: nix profile install github:NEO-LAX/anihub-cli
```

**Windows:** бінарник з [Releases](https://github.com/NEO-LAX/anihub-cli/releases/latest) і додати в `PATH`.

| Платформа | Файл |
| --- | --- |
| Linux x86_64 | `anihub-cli-x86_64-unknown-linux-gnu` |
| macOS Intel | `anihub-cli-x86_64-apple-darwin` |
| macOS Apple silicon | `anihub-cli-aarch64-apple-darwin` |
| Windows x86_64 | `anihub-cli-x86_64-pc-windows-msvc.exe` |

---

## Залежності

- **`mpv`** у `PATH` (відтворення Ashdi)
- Сучасний термінал (протоколи зображень — опційно)
- Discord **десктоп**, якщо вмикаєте Rich Presence

```bash
# Debian/Ubuntu
sudo apt install mpv

# macOS
brew install mpv
```

---

## Керування

Підказки в футері залежать від екрана. `?` / `h` — повна довідка.

| Клавіша | Дія |
| --- | --- |
| `1` `2` `3` | Пошук · Бібліотека · Налаштування |
| `/` | Пошук AniHub або фільтр локальної бібліотеки |
| `↑` `↓` · `k` `j` | Рух по списку |
| `Enter` · `→` | Відкрити / грати |
| `Esc` · `←` | Назад (`Esc` на корені пошуку очищує результати) |
| `c` | Продовжити перегляд |
| `e` | Статус у бібліотеці |
| `Space` | Переглянуто / не переглянуто |
| `o` | Відкрити в браузері |
| `r` | Повторити запит після мережевої помилки |
| `q` | Вийти |

Під час введення запиту цифри лишаються літерами; вкладки — `Alt`/`Ctrl` + `1`–`3`.

---

## Налаштування та дані

Зберігаються в data-директорії платформи (`settings.json`, `history.json`, кеші):

| | Шлях |
| --- | --- |
| Linux | `~/.local/share/anihub-cli/` |
| macOS | `~/Library/Application Support/com.shadowgarden.anihub-cli/` |
| Windows | `%LOCALAPPDATA%\shadowgarden\anihub-cli\data\` |

Ключове: autoplay, resume, режим пошуку, теми, шлях/аргументи mpv, Discord, очищення кешу постерів, перевірка оновлень (без авто-встановлення).

---

## Збірка

Rust **1.85+**:

```bash
git clone https://github.com/NEO-LAX/anihub-cli.git
cd anihub-cli
cargo build --locked --release
```

Бінарник: `target/release/anihub-cli`

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

---

## Нотатки

- Неофіційний клієнт; залежить від live AniHub / джерел стрімів.
- Скрипт інсталера — лише Linux і macOS. Windows — релізні бінарники.
- Uninstall зберігає дані, якщо не вказати `--purge`.

## Ліцензія

[MIT](LICENSE)
