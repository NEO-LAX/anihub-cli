#!/usr/bin/env bash
set -euo pipefail

BINARY_NAME="anihub-cli"
REPO_URL="${ANIHUB_REPO_URL:-https://github.com/NEO-LAX/anihub-cli}"
RELEASE_BASE_URL="${ANIHUB_RELEASE_BASE_URL:-${REPO_URL}/releases/latest/download}"
RELEASE_BASE_URL="${RELEASE_BASE_URL%/}"
INSTALL_DIR="${ANIHUB_INSTALL_DIR:-${HOME}/.local/bin}"
INSTALL_PATH="${INSTALL_DIR}/${BINARY_NAME}"

TMP_DIR=""
REPLACEMENT_PATH=""

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

cleanup() {
    if [[ -n "${REPLACEMENT_PATH}" && -e "${REPLACEMENT_PATH}" ]]; then
        rm -f "${REPLACEMENT_PATH}"
    fi
    if [[ -n "${TMP_DIR}" && -d "${TMP_DIR}" ]]; then
        rm -rf "${TMP_DIR}"
    fi
}

trap cleanup EXIT

info() {
    printf '%b%s%b\n' "${BLUE}" "$1" "${NC}"
}

success() {
    printf '%b%s%b\n' "${GREEN}" "$1" "${NC}"
}

warning() {
    printf '%b%s%b\n' "${YELLOW}" "$1" "${NC}" >&2
}

fail() {
    printf '%bError: %s%b\n' "${RED}" "$1" "${NC}" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage:
  install.sh install      Install the latest supported release.
  install.sh uninstall    Remove the installed binary and keep user data.

With no argument, an interactive menu is shown only from a terminal. Use an
explicit mode when piping this script to bash.
EOF
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        fail "Required command not found: $1"
    fi
}

detect_release() {
    local os_type arch
    os_type="$(uname -s)"
    arch="$(uname -m)"

    case "${os_type}:${arch}" in
        Linux:x86_64|Linux:amd64)
            printf '%s\n' "anihub-cli-x86_64-unknown-linux-gnu"
            ;;
        Darwin:x86_64|Darwin:amd64)
            printf '%s\n' "anihub-cli-x86_64-apple-darwin"
            ;;
        Darwin:arm64|Darwin:aarch64)
            printf '%s\n' "anihub-cli-aarch64-apple-darwin"
            ;;
        *)
            fail "Unsupported OS/architecture: ${os_type}/${arch}. Supported combinations are Linux x86_64, macOS x86_64, and macOS arm64."
            ;;
    esac
}

check_dependencies() {
    info "Checking runtime dependencies..."

    if command -v mpv >/dev/null 2>&1; then
        success "mpv found (episode playback is available)."
    else
        warning "mpv not found. Install mpv before trying to play an episode."
    fi

}

checksum_value() {
    local file_path="$1"

    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "${file_path}" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "${file_path}" | awk '{print $1}'
    else
        fail "Neither sha256sum nor shasum is available; cannot verify the release."
    fi
}

verify_checksum() {
    local checksums_path="$1"
    local binary_path="$2"
    local asset_name="$3"
    local expected actual

    expected="$(awk -v asset="${asset_name}" '
        {
            filename = $2
            sub(/^\*/, "", filename)
            if (filename == asset) {
                print $1
                exit
            }
        }
    ' "${checksums_path}")"

    if [[ ! "${expected}" =~ ^[0-9a-fA-F]{64}$ ]]; then
        fail "SHA256SUMS does not contain a valid checksum for ${asset_name}."
    fi

    actual="$(checksum_value "${binary_path}")"
    expected="$(printf '%s' "${expected}" | tr '[:upper:]' '[:lower:]')"
    actual="$(printf '%s' "${actual}" | tr '[:upper:]' '[:lower:]')"

    if [[ "${actual}" != "${expected}" ]]; then
        fail "Checksum verification failed for ${asset_name}. The downloaded file was discarded."
    fi
}

download() {
    local url="$1"
    local destination="$2"

    curl --fail --location --retry 3 --silent --show-error --output "${destination}" "${url}"
}

path_guidance() {
    local current_path="${PATH-}"

    case ":${current_path}:" in
        *":${INSTALL_DIR}:"*)
            ;;
        *)
            warning "${INSTALL_DIR} is not currently in PATH."
            printf '%s\n' "For this shell: export PATH=\"${INSTALL_DIR}:\$PATH\""
            printf 'Persist it by adding that export to ~/.profile (bash) or ~/.zprofile (zsh), then open a new shell.\n'
            ;;
    esac
}

install_app() {
    local asset_name binary_url checksums_url downloaded_binary downloaded_checksums

    asset_name="$(detect_release)"
    require_command curl
    require_command mktemp
    require_command awk
    require_command install
    require_command mv

    info "Installing ${BINARY_NAME} (${asset_name})..."
    check_dependencies

    mkdir -p "${INSTALL_DIR}"
    TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/anihub-cli.XXXXXX")"
    downloaded_binary="${TMP_DIR}/${asset_name}"
    downloaded_checksums="${TMP_DIR}/SHA256SUMS"
    binary_url="${RELEASE_BASE_URL}/${asset_name}"
    checksums_url="${RELEASE_BASE_URL}/SHA256SUMS"

    info "Downloading binary and SHA256SUMS..."
    download "${binary_url}" "${downloaded_binary}"
    download "${checksums_url}" "${downloaded_checksums}"
    verify_checksum "${downloaded_checksums}" "${downloaded_binary}" "${asset_name}"
    chmod 0755 "${downloaded_binary}"

    # Create the replacement in the destination directory so rename is atomic
    # on the target filesystem. The existing binary is untouched until mv.
    REPLACEMENT_PATH="$(mktemp "${INSTALL_DIR}/.${BINARY_NAME}.tmp.XXXXXX")"
    install -m 0755 "${downloaded_binary}" "${REPLACEMENT_PATH}"
    mv -f "${REPLACEMENT_PATH}" "${INSTALL_PATH}"
    REPLACEMENT_PATH=""

    success "Installed ${INSTALL_PATH}"
    path_guidance
}

uninstall_app() {
    if [[ -e "${INSTALL_PATH}" || -L "${INSTALL_PATH}" ]]; then
        rm -f "${INSTALL_PATH}"
        success "Removed ${INSTALL_PATH}. History and other user data were kept."
    else
        warning "${INSTALL_PATH} was not found; nothing to remove."
    fi
}

show_menu() {
    local option

    while true; do
        printf '\n%b--- AniHub CLI installer ---%b\n' "${BLUE}" "${NC}"
        printf '%s\n' '1) Install' '2) Uninstall' '3) Exit'
        printf 'Select option [1-3]: '
        if ! IFS= read -r option < /dev/tty; then
            fail "Unable to read the interactive menu. Use install.sh install or install.sh uninstall."
        fi

        case "${option}" in
            1)
                install_app
                return 0
                ;;
            2)
                uninstall_app
                return 0
                ;;
            3)
                return 0
                ;;
            *)
                warning "Invalid choice."
                ;;
        esac
    done
}

main() {
    if [[ "$#" -gt 1 ]]; then
        usage >&2
        exit 2
    fi

    case "${1:-}" in
        install)
            install_app
            ;;
        uninstall)
            uninstall_app
            ;;
        help|-h|--help)
            usage
            ;;
        '')
            if [[ -t 0 && -t 1 ]]; then
                show_menu
            else
                usage >&2
                fail "Non-interactive mode requires an explicit install or uninstall argument."
            fi
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
}

main "$@"
