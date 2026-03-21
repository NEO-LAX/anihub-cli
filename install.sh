#!/bin/bash

# Змінні
BINARY_NAME="anihub-cli"
REPO_URL="https://github.com/NEO-LAX/anihub-cli"
INSTALL_DIR="$HOME/.local/bin"
LATEST_RELEASE_URL="$REPO_URL/releases/latest/download/anihub-cli"

# Кольори для краси
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Функція перевірки залежностей
check_dependencies() {
    echo -e "${BLUE}🔍 Перевірка залежностей...${NC}"
    deps=("mpv" "yt-dlp")
    missing_deps=()

    for dep in "${deps[@]}"; do
        if ! command -v "$dep" &> /dev/null; then
            missing_deps+=("$dep")
        fi
    done

    if [ ${#missing_deps[@]} -ne 0 ]; then
        echo -e "${YELLOW}⚠️  Відсутні залежності: ${missing_deps[*]}${NC}"
        echo "Рекомендуємо встановити їх для коректної роботи плеєра."
    else
        echo -e "${GREEN}✅ Залежності знайдено (mpv, yt-dlp).${NC}"
    fi
}

# Функція встановлення
install_app() {
    echo -e "${BLUE}🚀 Встановлення $BINARY_NAME...${NC}"
    check_dependencies

    if [ ! -d "$INSTALL_DIR" ]; then
        mkdir -p "$INSTALL_DIR"
    fi

    echo -e "${BLUE}📥 Завантаження останнього релізу...${NC}"
    if curl -fsSL "$LATEST_RELEASE_URL" -o "$INSTALL_DIR/$BINARY_NAME"; then
        chmod +x "$INSTALL_DIR/$BINARY_NAME"
        echo -e "${GREEN}✅ Успішно встановлено в $INSTALL_DIR/$BINARY_NAME${NC}"
        
        if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
            echo -e "${YELLOW}⚠️  Додайте $INSTALL_DIR у ваш \$PATH${NC}"
        fi
    else
        echo -e "${RED}❌ Помилка завантаження. Перевірте GitHub Releases.${NC}"
        exit 1
    fi
}

# Функція видалення
uninstall_app() {
    echo -e "${RED}🗑️  Видалення $BINARY_NAME...${NC}"
    if [ -f "$INSTALL_DIR/$BINARY_NAME" ]; then
        rm "$INSTALL_DIR/$BINARY_NAME"
        echo -e "${GREEN}✅ Програму видалено.${NC}"
    else
        echo -e "${YELLOW}ℹ️  Програма не знайдена.${NC}"
    fi
}

# Інтерактивне меню
show_menu() {
    clear
    echo -e "${BLUE}====================================${NC}"
    echo -e "${BLUE}      AniHub CLI Installer          ${NC}"
    echo -e "${BLUE}====================================${NC}"
    echo "1) Встановити (Install)"
    echo "2) Видалити (Uninstall)"
    echo "3) Вихід (Exit)"
    echo -e "${BLUE}------------------------------------${NC}"
    read -p "Виберіть опцію [1-3]: " opt

    case $opt in
        1) install_app ;;
        2) uninstall_app ;;
        3) exit 0 ;;
        *) echo -e "${RED}Невірний вибір${NC}"; sleep 1; show_menu ;;
    esac
}

# Якщо є аргумент — виконуємо його, якщо ні — показуємо меню
if [ -z "$1" ]; then
    show_menu
else
    case "$1" in
        install) install_app ;;
        uninstall) uninstall_app ;;
        *) show_menu ;;
    esac
fi
