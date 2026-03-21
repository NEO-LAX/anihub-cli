#!/bin/bash

# Змінні
BINARY_NAME="anihub-cli"
REPO_URL="https://github.com/NEO-LAX/anihub-cli"
INSTALL_DIR="$HOME/.local/bin"
LATEST_RELEASE_URL="$REPO_URL/releases/latest/download/anihub-cli"

# Кольори
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

check_dependencies() {
    echo -e "${BLUE}🔍 Перевірка залежностей...${NC}"
    for dep in mpv yt-dlp; do
        if command -v "$dep" &> /dev/null; then
            echo -e "${GREEN}✅ $dep знайдено.${NC}"
        else
            echo -e "${YELLOW}⚠️  $dep не знайдено. Встановіть його для роботи плеєра.${NC}"
        fi
    done
}

install_app() {
    echo -e "${BLUE}🚀 Встановлення...${NC}"
    check_dependencies
    mkdir -p "$INSTALL_DIR"

    echo -e "${BLUE}📥 Завантаження бінарника...${NC}"
    if curl -L "$LATEST_RELEASE_URL" -o "$INSTALL_DIR/$BINARY_NAME"; then
        chmod +x "$INSTALL_DIR/$BINARY_NAME"
        echo -e "${GREEN}✅ Успішно встановлено в $INSTALL_DIR/$BINARY_NAME${NC}"
        echo -e "${YELLOW}Переконайтеся, що $INSTALL_DIR є у вашому PATH.${NC}"
    else
        echo -e "${RED}❌ Помилка: Не вдалося завантажити файл. Перевірте, чи створено Release на GitHub.${NC}"
    fi
}

uninstall_app() {
    if [ -f "$INSTALL_DIR/$BINARY_NAME" ]; then
        rm "$INSTALL_DIR/$BINARY_NAME"
        echo -e "${GREEN}✅ Видалено.${NC}"
    else
        echo -e "${YELLOW}ℹ️  Файл не знайдено.${NC}"
    fi
}

show_menu() {
    while true; do
        echo -e "\n${BLUE}--- AniHub CLI Installer ---${NC}"
        echo "1) Встановити (Install)"
        echo "2) Видалити (Uninstall)"
        echo "3) Вихід (Exit)"
        
        printf "Виберіть опцію [1-3]: "
        read -r opt < /dev/tty

        case "$opt" in
            1) install_app; break ;;
            2) uninstall_app; break ;;
            3) exit 0 ;;
            *) echo -e "${RED}Невірний вибір. Спробуйте ще раз.${NC}" ;;
        esac
    done
}

# Головна логіка
if [ "$1" == "install" ]; then
    install_app
elif [ "$1" == "uninstall" ]; then
    uninstall_app
else
    show_menu
fi
