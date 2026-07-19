#!/usr/bin/env bash
set -euo pipefail

BINARY_NAME="anihub-cli"
REPO_URL="${ANIHUB_REPO_URL:-https://github.com/NEO-LAX/anihub-cli}"
# Prefer a specific tag when set, e.g. ANIHUB_RELEASE_TAG=v0.6.0
if [[ -n "${ANIHUB_RELEASE_BASE_URL:-}" ]]; then
    RELEASE_BASE_URL="${ANIHUB_RELEASE_BASE_URL%/}"
elif [[ -n "${ANIHUB_RELEASE_TAG:-}" ]]; then
    RELEASE_BASE_URL="${REPO_URL}/releases/download/${ANIHUB_RELEASE_TAG}"
else
    RELEASE_BASE_URL="${REPO_URL}/releases/latest/download"
fi
INSTALL_DIR="${ANIHUB_INSTALL_DIR:-${HOME}/.local/bin}"
INSTALL_PATH="${INSTALL_DIR}/${BINARY_NAME}"

TMP_DIR=""
REPLACEMENT_PATH=""
MENU_ACTIONS=()
MENU_LABELS=()

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
MAGENTA='\033[0;35m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
DIM='\033[2m'
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
  install.sh                     Interactive arrow-key menu.
  install.sh install             Install or update the latest release.
  install.sh update              Update an existing installation.
  install.sh uninstall           Remove the app and keep user data.
  install.sh uninstall --purge   Remove the app and all user data.
EOF
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        fail "required command not found: $1"
    fi
}

is_installed() {
    [[ -e "${INSTALL_PATH}" || -L "${INSTALL_PATH}" ]]
}

data_dir() {
    if [[ -n "${ANIHUB_DATA_DIR:-}" ]]; then
        printf '%s\n' "${ANIHUB_DATA_DIR}"
        return
    fi

    case "$(uname -s)" in
        Linux)
            printf '%s\n' "${XDG_DATA_HOME:-${HOME}/.local/share}/anihub-cli"
            ;;
        Darwin)
            printf '%s\n' "${HOME}/Library/Application Support/com.shadowgarden.anihub-cli"
            ;;
        *)
            fail "cannot determine the AniHub CLI data directory on this system"
            ;;
    esac
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
            fail "unsupported system: ${os_type}/${arch}. Supported platforms are Linux x86_64 and macOS x86_64/arm64."
            ;;
    esac
}

check_dependencies() {
    info "Checking runtime dependencies..."
    if command -v mpv >/dev/null 2>&1; then
        success "mpv found."
    else
        warning "mpv was not found. Search and library features will work, but playback will not."
    fi
}

checksum_value() {
    local file_path="$1"

    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "${file_path}" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "${file_path}" | awk '{print $1}'
    else
        fail "neither sha256sum nor shasum is available; the release cannot be verified"
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
        fail "SHA256SUMS does not contain a valid checksum for ${asset_name}"
    fi

    actual="$(checksum_value "${binary_path}")"
    expected="$(printf '%s' "${expected}" | tr '[:upper:]' '[:lower:]')"
    actual="$(printf '%s' "${actual}" | tr '[:upper:]' '[:lower:]')"
    if [[ "${actual}" != "${expected}" ]]; then
        fail "SHA256 verification failed for ${asset_name}; the downloaded file was discarded"
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
        *":${INSTALL_DIR}:"*) ;;
        *)
            warning "${INSTALL_DIR} is not currently in PATH."
            printf '%s\n' "For this shell: export PATH=\"${INSTALL_DIR}:\$PATH\""
            printf 'To make it permanent, add that export to ~/.profile or ~/.zprofile.\n'
            ;;
    esac
}

migrate_installed_data() {
    local installed_binary="$1"
    local migrate_log
    migrate_log="$(mktemp "${TMPDIR:-/tmp}/anihub-migrate.XXXXXX")"
    info "Validating and migrating local history and settings..."
    if ! "${installed_binary}" --migrate-data >"${migrate_log}" 2>&1; then
        cat "${migrate_log}" >&2 || true
        if grep -Eqi "unsupported history schema version|unknown argument|невідомі аргументи|--migrate-data" "${migrate_log}" 2>/dev/null; then
            warning "Data migration failed: the downloaded release is too old for the local data."
        else
            warning "Data migration failed."
        fi
        rm -f "${migrate_log}"
        return 1
    fi
    # Surface migrate success details (paths / counts).
    cat "${migrate_log}" || true
    rm -f "${migrate_log}"
    success "Local data is ready for the new version."
}

restore_install_transaction() {
    local had_binary="$1"
    local previous_binary="$2"
    local directory="$3"
    local data_existed="$4"
    local data_backup="$5"

    warning "Rolling back the binary and user data..."
    if [[ "${had_binary}" == 'true' ]]; then
        cp -p "${previous_binary}" "${INSTALL_PATH}"
        chmod 0755 "${INSTALL_PATH}"
    else
        rm -f "${INSTALL_PATH}"
    fi

    if [[ -e "${directory}" || -L "${directory}" ]]; then
        rm -rf -- "${directory}"
    fi
    if [[ "${data_existed}" == 'true' ]]; then
        mkdir -p "$(dirname "${directory}")"
        cp -a "${data_backup}" "${directory}"
    fi
}

install_app() {
    local asset_name binary_url checksums_url downloaded_binary downloaded_checksums action
    local migration_directory previous_binary data_backup had_binary data_existed

    if is_installed; then
        action="Updating"
    else
        action="Installing"
    fi
    asset_name="$(detect_release)"
    require_command curl
    require_command mktemp
    require_command awk
    require_command install
    require_command mv
    require_command cp

    info "${action} ${BINARY_NAME} (${asset_name})..."
    check_dependencies

    mkdir -p "${INSTALL_DIR}"
    TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/anihub-cli.XXXXXX")"
    downloaded_binary="${TMP_DIR}/${asset_name}"
    downloaded_checksums="${TMP_DIR}/SHA256SUMS"
    binary_url="${RELEASE_BASE_URL}/${asset_name}"
    checksums_url="${RELEASE_BASE_URL}/SHA256SUMS"

    info "Downloading the binary and SHA256SUMS..."
    download "${binary_url}" "${downloaded_binary}"
    download "${checksums_url}" "${downloaded_checksums}"
    verify_checksum "${downloaded_checksums}" "${downloaded_binary}" "${asset_name}"
    chmod 0755 "${downloaded_binary}"

    REPLACEMENT_PATH="$(mktemp "${INSTALL_DIR}/.${BINARY_NAME}.tmp.XXXXXX")"
    install -m 0755 "${downloaded_binary}" "${REPLACEMENT_PATH}"

    # Binary replacement and schema migration form one transaction. Keep both
    # sides so a migration failure cannot leave a new schema with an old app,
    # or a new app with partially migrated user data.
    migration_directory="$(validated_data_dir)"
    previous_binary="${TMP_DIR}/previous-binary"
    data_backup="${TMP_DIR}/previous-data"
    had_binary='false'
    data_existed='false'
    if is_installed; then
        cp -p "${INSTALL_PATH}" "${previous_binary}"
        had_binary='true'
    fi
    if [[ -e "${migration_directory}" || -L "${migration_directory}" ]]; then
        cp -a "${migration_directory}" "${data_backup}"
        data_existed='true'
    fi

    mv -f "${REPLACEMENT_PATH}" "${INSTALL_PATH}"
    REPLACEMENT_PATH=""
    if ! migrate_installed_data "${INSTALL_PATH}"; then
        restore_install_transaction \
            "${had_binary}" \
            "${previous_binary}" \
            "${migration_directory}" \
            "${data_existed}" \
            "${data_backup}"
        fail "data migration failed; the previous binary and user data were restored"
    fi

    success "Done: ${INSTALL_PATH}"
    path_guidance
}

update_app() {
    if ! is_installed; then
        fail "${INSTALL_PATH} was not found. Install the app first"
    fi
    install_app
}

validated_data_dir() {
    local directory leaf
    directory="$(data_dir)"
    case "${directory}" in
        ''|'/'|"${HOME}")
            fail "refusing to remove unsafe data directory: ${directory:-<empty>}"
            ;;
        /*) ;;
        *)
            fail "refusing to remove a non-absolute data directory: ${directory}"
            ;;
    esac
    leaf="${directory##*/}"
    case "${leaf}" in
        anihub-cli|com.shadowgarden.anihub-cli) ;;
        *)
            fail "refusing to remove an unexpected data directory: ${directory}"
            ;;
    esac

    printf '%s\n' "${directory}"
}

purge_user_data() {
    local directory="$1"

    if [[ -e "${directory}" || -L "${directory}" ]]; then
        rm -rf -- "${directory}"
        success "Removed user data: ${directory}"
    else
        warning "User data directory was not found: ${directory}"
    fi
}

uninstall_app() {
    local purge_data="${1:-false}" purge_directory=''
    if [[ "${purge_data}" == 'true' ]]; then
        purge_directory="$(validated_data_dir)"
    fi

    if is_installed; then
        rm -f "${INSTALL_PATH}"
        success "Removed ${INSTALL_PATH}."
    else
        warning "${INSTALL_PATH} was not found."
    fi

    if [[ "${purge_data}" == 'true' ]]; then
        purge_user_data "${purge_directory}"
    else
        success "User data was kept."
    fi
}

render_menu() {
    local selected="$1"
    shift
    local labels=("$@")
    local index marker style

    printf '\033[2J\033[H'
    printf '%b╭────────────────────────────────────╮%b\n' "${MAGENTA}" "${NC}"
    printf '%b│%b  %bAniHub CLI%b · installer           %b│%b\n' "${MAGENTA}" "${NC}" "${BOLD}" "${NC}" "${MAGENTA}" "${NC}"
    printf '%b╰────────────────────────────────────╯%b\n\n' "${MAGENTA}" "${NC}"
    if is_installed; then
        printf '%bInstalled:%b %s\n\n' "${DIM}" "${NC}" "${INSTALL_PATH}"
    else
        printf '%bNot installed yet%b\n\n' "${DIM}" "${NC}"
    fi

    for index in "${!labels[@]}"; do
        marker=' '
        style="${NC}"
        if ((index == selected)); then
            marker='▶'
            style="${MAGENTA}${BOLD}"
        fi
        printf '%s %b[ %-12s ]%b\n' "${marker}" "${style}" "${labels[index]}" "${NC}"
    done
    printf '\n%b↑/↓ or j/k · Enter select · q quit%b\n' "${DIM}" "${NC}"
}

read_menu_action() {
    local selected=0 key rest

    while true; do
        render_menu "${selected}" "${MENU_LABELS[@]}" > /dev/tty
        key=''
        if ! IFS= read -rsn1 key < /dev/tty; then
            fail "could not read a key from the terminal"
        fi
        if [[ "${key}" == $'\033' ]]; then
            rest=''
            IFS= read -rsn2 -t 0.15 rest < /dev/tty || true
            key+="${rest}"
        fi

        case "${key}" in
            $'\033[A'|k)
                selected=$(((selected - 1 + ${#MENU_ACTIONS[@]}) % ${#MENU_ACTIONS[@]}))
                ;;
            $'\033[B'|j)
                selected=$(((selected + 1) % ${#MENU_ACTIONS[@]}))
                ;;
            '')
                printf '%s\n' "${MENU_ACTIONS[selected]}"
                return
                ;;
            q|Q|$'\033')
                printf '%s\n' 'exit'
                return
                ;;
            [1-9])
                if ((key <= ${#MENU_ACTIONS[@]})); then
                    printf '%s\n' "${MENU_ACTIONS[key - 1]}"
                    return
                fi
                ;;
        esac
    done
}

show_uninstall_menu() {
    local action
    MENU_ACTIONS=(keep-data purge-data cancel)
    MENU_LABELS=("Keep user data" "Delete user data" "Cancel")

    action="$(read_menu_action)"
    printf '\033[2J\033[H' > /dev/tty
    case "${action}" in
        keep-data) uninstall_app false ;;
        purge-data) uninstall_app true ;;
        cancel|exit) return 0 ;;
    esac
}

show_menu() {
    local action

    if is_installed; then
        MENU_ACTIONS=(update uninstall exit)
        MENU_LABELS=("Update" "Uninstall" "Exit")
    else
        MENU_ACTIONS=(install exit)
        MENU_LABELS=("Install" "Exit")
    fi

    action="$(read_menu_action)"
    printf '\033[2J\033[H' > /dev/tty
    case "${action}" in
        install) install_app ;;
        update) update_app ;;
        uninstall) show_uninstall_menu ;;
        exit) return 0 ;;
    esac
}

main() {
    if [[ "$#" -gt 2 ]]; then
        usage >&2
        exit 2
    fi

    case "${1:-}" in
        install)
            [[ "$#" -eq 1 ]] || { usage >&2; exit 2; }
            install_app
            ;;
        update)
            [[ "$#" -eq 1 ]] || { usage >&2; exit 2; }
            update_app
            ;;
        uninstall)
            case "${2:-}" in
                '') uninstall_app false ;;
                --purge) uninstall_app true ;;
                *) usage >&2; exit 2 ;;
            esac
            ;;
        help|-h|--help) usage ;;
        '')
            if [[ -r /dev/tty && -w /dev/tty ]]; then
                show_menu
            else
                usage >&2
                fail "the interactive menu requires a TTY; pass install, update, or uninstall explicitly"
            fi
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
}

main "$@"
