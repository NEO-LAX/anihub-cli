#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
INSTALLER="${ROOT_DIR}/install.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/anihub-install-test.XXXXXX")"
FIXTURE_DIR="${TEST_ROOT}/fixtures"
FAKE_BIN_DIR="${TEST_ROOT}/bin"
INSTALL_DIR="${TEST_ROOT}/install"
CURL_LOG="${TEST_ROOT}/curl.log"
MIGRATION_LOG="${TEST_ROOT}/migration.log"
USER_DATA_DIR="${TEST_ROOT}/user-data/anihub-cli"
ASSET_NAME="anihub-cli-x86_64-unknown-linux-gnu"

cleanup() {
    rm -rf "${TEST_ROOT}"
}

trap cleanup EXIT

fail_test() {
    printf 'TEST FAILED: %s\n' "$1" >&2
    exit 1
}

assert_file_equal() {
    local expected="$1"
    local actual="$2"

    if ! cmp -s "${expected}" "${actual}"; then
        fail_test "${actual} does not match ${expected}"
    fi
}

mkdir -p "${FIXTURE_DIR}" "${FAKE_BIN_DIR}" "${INSTALL_DIR}"
cat > "${FIXTURE_DIR}/${ASSET_NAME}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
    --migrate-data)
        printf 'migrated\n' >> "${TEST_MIGRATION_LOG}"
        if [[ "${TEST_MIGRATION_FAIL:-0}" == '1' ]]; then
            exit 1
        fi
        ;;
    --version)
        printf 'anihub-cli 0.6.0\n'
        ;;
esac
EOF
chmod 0755 "${FIXTURE_DIR}/${ASSET_NAME}"
(
    cd "${FIXTURE_DIR}" || exit 1
    sha256sum "${ASSET_NAME}" > SHA256SUMS
)
cp "${FIXTURE_DIR}/SHA256SUMS" "${FIXTURE_DIR}/SHA256SUMS.valid"

cat > "${FAKE_BIN_DIR}/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

output=''
url=''
arguments="$*"

while (($#)); do
    case "$1" in
        --output)
            output="$2"
            shift 2
            ;;
        *)
            url="$1"
            shift
            ;;
    esac
done

printf '%s\n' "${arguments}" >> "${TEST_CURL_LOG}"

case "${url}" in
    */SHA256SUMS)
        cp "${TEST_FIXTURE_DIR}/SHA256SUMS" "${output}"
        ;;
    */${TEST_ASSET_NAME})
        cp "${TEST_FIXTURE_DIR}/${TEST_ASSET_NAME}" "${output}"
        ;;
    *)
        printf 'Unexpected test URL: %s\n' "${url}" >&2
        exit 1
        ;;
esac
EOF

cat > "${FAKE_BIN_DIR}/uname" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
    -s)
        printf '%s\n' "${TEST_UNAME_SYSTEM:-Linux}"
        ;;
    -m)
        printf '%s\n' "${TEST_UNAME_MACHINE:-x86_64}"
        ;;
    *)
        printf 'Unsupported uname argument: %s\n' "${1:-}" >&2
        exit 1
        ;;
esac
EOF

cat > "${FAKE_BIN_DIR}/mpv" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF

chmod 0755 "${FAKE_BIN_DIR}/curl" "${FAKE_BIN_DIR}/uname" "${FAKE_BIN_DIR}/mpv"

export HOME="${TEST_ROOT}/home"
export PATH="${FAKE_BIN_DIR}:${PATH}"
export TEST_ASSET_NAME="${ASSET_NAME}"
export TEST_CURL_LOG="${CURL_LOG}"
export TEST_FIXTURE_DIR="${FIXTURE_DIR}"
export TEST_MIGRATION_LOG="${MIGRATION_LOG}"
export ANIHUB_INSTALL_DIR="${INSTALL_DIR}"
export ANIHUB_DATA_DIR="${USER_DATA_DIR}"
export ANIHUB_RELEASE_BASE_URL="https://example.invalid/releases/latest/download"
export XDG_DATA_HOME="${TEST_ROOT}/data"

printf 'old binary\n' > "${INSTALL_DIR}/anihub-cli"
chmod 0755 "${INSTALL_DIR}/anihub-cli"
cp "${INSTALL_DIR}/anihub-cli" "${TEST_ROOT}/old-binary"

printf '%064d  %s\n' 0 "${ASSET_NAME}" > "${FIXTURE_DIR}/SHA256SUMS"
if bash "${INSTALLER}" install > "${TEST_ROOT}/checksum-failure.log" 2>&1; then
    fail_test 'checksum mismatch unexpectedly succeeded'
fi
assert_file_equal "${TEST_ROOT}/old-binary" "${INSTALL_DIR}/anihub-cli"

cp "${FIXTURE_DIR}/SHA256SUMS.valid" "${FIXTURE_DIR}/SHA256SUMS"
bash "${INSTALLER}" install > "${TEST_ROOT}/install.log" 2>&1
assert_file_equal "${FIXTURE_DIR}/${ASSET_NAME}" "${INSTALL_DIR}/anihub-cli"
if [[ ! -x "${INSTALL_DIR}/anihub-cli" ]]; then
    fail_test 'installed binary is not executable'
fi
if ! grep -F 'migrated' "${MIGRATION_LOG}" >/dev/null; then
    fail_test 'installer did not run the downloaded binary data migration'
fi

if ! grep -F -- '--fail --location --retry 3' "${CURL_LOG}" >/dev/null; then
    fail_test 'installer did not use the required curl retry/failure options'
fi

export TEST_UNAME_MACHINE='aarch64'
if bash "${INSTALLER}" install > "${TEST_ROOT}/unsupported.log" 2>&1; then
    fail_test 'unsupported platform unexpectedly succeeded'
fi
if ! grep -F 'unsupported system' "${TEST_ROOT}/unsupported.log" >/dev/null; then
    fail_test 'unsupported platform error was not clear'
fi
unset TEST_UNAME_MACHINE

cp "${INSTALL_DIR}/anihub-cli" "${TEST_ROOT}/installed-before-failed-update"
export TEST_MIGRATION_FAIL=1
if bash "${INSTALLER}" update > "${TEST_ROOT}/migration-failure.log" 2>&1; then
    fail_test 'update unexpectedly succeeded when data migration failed'
fi
unset TEST_MIGRATION_FAIL
assert_file_equal "${TEST_ROOT}/installed-before-failed-update" "${INSTALL_DIR}/anihub-cli"

bash "${INSTALLER}" update > "${TEST_ROOT}/update.log" 2>&1
assert_file_equal "${FIXTURE_DIR}/${ASSET_NAME}" "${INSTALL_DIR}/anihub-cli"

mkdir -p "${USER_DATA_DIR}"
printf '{"schema_version":2,"progress":{},"library":{}}\n' > "${USER_DATA_DIR}/history.json"
bash "${INSTALLER}" uninstall > "${TEST_ROOT}/uninstall.log" 2>&1
if [[ -e "${INSTALL_DIR}/anihub-cli" ]]; then
    fail_test 'uninstall did not remove the installed binary'
fi
if [[ ! -e "${USER_DATA_DIR}/history.json" ]]; then
    fail_test 'safe uninstall unexpectedly removed user data'
fi

bash "${INSTALLER}" install > "${TEST_ROOT}/reinstall.log" 2>&1
bash "${INSTALLER}" uninstall --purge > "${TEST_ROOT}/purge.log" 2>&1
if [[ -e "${INSTALL_DIR}/anihub-cli" ]]; then
    fail_test 'purge uninstall did not remove the installed binary'
fi
if [[ -e "${USER_DATA_DIR}" ]]; then
    fail_test 'purge uninstall did not remove user data'
fi
bash "${INSTALLER}" install > "${TEST_ROOT}/reinstall-for-safety.log" 2>&1
if ANIHUB_DATA_DIR='/' bash "${INSTALLER}" uninstall --purge > "${TEST_ROOT}/unsafe-purge.log" 2>&1; then
    fail_test 'unsafe purge path unexpectedly succeeded'
fi
if ! grep -F 'refusing to remove unsafe data directory' "${TEST_ROOT}/unsafe-purge.log" >/dev/null; then
    fail_test 'unsafe purge path did not produce a clear error'
fi
if [[ ! -e "${INSTALL_DIR}/anihub-cli" ]]; then
    fail_test 'unsafe purge removed the installed binary before validating the data path'
fi
bash "${INSTALLER}" uninstall > "${TEST_ROOT}/final-uninstall.log" 2>&1

if bash "${INSTALLER}" update > "${TEST_ROOT}/missing-update.log" 2>&1; then
    fail_test 'update without an installed binary unexpectedly succeeded'
fi

printf 'Installer tests passed.\n'
