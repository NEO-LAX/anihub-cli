#!/bin/bash

# Змінні
BINARY_NAME="anihub-cli"
REPO_URL="https://github.com/NEO-LAX/anihub-cli"
INSTALL_DIR="$HOME/.local/bin"

# Визначаємо OS для вибору правильного бінарника
OS_TYPE=$(uname -s)
if [ "$OS_TYPE" == "Linux" ]; then
    LATEST_RELEASE_URL="$REPO_URL/releases/latest/download/anihub-cli-linux"
elif [ "$OS_TYPE" == "Darwin" ]; then
    LATEST_RELEASE_URL="$REPO_URL/releases/latest/download/anihub-cli"
else
    echo "⚠️  Unsupported OS: $OS_TYPE"
    exit 1
fi

# Кольори
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

check_dependencies() {
    echo -e "${BLUE}🔍 Checking dependencies...${NC}"
    for dep in mpv yt-dlp; do
        if command -v "$dep" &> /dev/null; then
            echo -e "${GREEN}✅ $dep found.${NC}"
        else
            echo -e "${YELLOW}⚠️  $dep NOT found.${NC}"
            if [ "$OS_TYPE" == "Darwin" ]; then
                echo "Run: brew install mpv yt-dlp"
            else
                echo "Install it via your package manager."
            fi
        fi
    done
}

install_app() {
    echo -e "${BLUE}🚀 Installing AniHub CLI for $OS_TYPE...${NC}"
    check_dependencies
    mkdir -p "$INSTALL_DIR"

    echo -e "${BLUE}📥 Downloading binary...${NC}"
    if curl -L "$LATEST_RELEASE_URL" -o "$INSTALL_DIR/$BINARY_NAME"; then
        chmod +x "$INSTALL_DIR/$BINARY_NAME"
        echo -e "${GREEN}✅ Installed successfully in $INSTALL_DIR/$BINARY_NAME${NC}"
        echo -e "${YELLOW}Ensure $INSTALL_DIR is in your PATH.${NC}"
    else
        echo -e "${RED}❌ Error: Download failed.${NC}"
    fi
}

uninstall_app() {
    if [ -f "$INSTALL_DIR/$BINARY_NAME" ]; then
        rm "$INSTALL_DIR/$BINARY_NAME"
        echo -e "${GREEN}✅ Uninstalled.${NC}"
    else
        echo -e "${YELLOW}ℹ️  File not found.${NC}"
    fi
}

show_menu() {
    while true; do
        echo -e "\n${BLUE}--- AniHub CLI Installer ($OS_TYPE) ---${NC}"
        echo "1) Install"
        echo "2) Uninstall"
        echo "3) Exit"
        
        printf "Select option [1-3]: "
        read -r opt < /dev/tty

        case "$opt" in
            1) install_app; break ;;
            2) uninstall_app; break ;;
            3) exit 0 ;;
            *) echo -e "${RED}Invalid choice.${NC}" ;;
        esac
    done
}

if [ "$1" == "install" ]; then
    install_app
elif [ "$1" == "uninstall" ]; then
    uninstall_app
else
    show_menu
fi
