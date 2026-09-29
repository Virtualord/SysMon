#!/usr/bin/env bash
#
# Build and install syswatch.
#
# The script only builds, copies a file and seeds a configuration. It never touches
# anything else, and it never overwrites an existing configuration.
#
# Usage:
#   ./install.sh                    # build release, install to ~/.local/bin
#   ./install.sh --prefix /usr/local
#   ./install.sh --no-config        # do not seed the default configuration
#   ./install.sh --offline          # fail instead of fetching missing crates
#   ./install.sh --uninstall        # delegate to uninstall.sh
#
set -euo pipefail

readonly PROGRAM="${0##*/}"
readonly REPO_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly BINARY="syswatch"

PREFIX="${HOME}/.local"
BIN_SUBDIR="bin"
CONFIG_SUBDIR="syswatch"
SEED_CONFIG=1
OFFLINE=0
CARGO_BIN="${CARGO:-cargo}"

# Colours are only used when stdout is a terminal, so redirected output stays clean.
if [[ -t 1 ]]; then
    readonly C_RESET=$'\033[0m'
    readonly C_BOLD=$'\033[1m'
    readonly C_GREEN=$'\033[32m'
    readonly C_YELLOW=$'\033[33m'
    readonly C_RED=$'\033[31m'
else
    readonly C_RESET='' C_BOLD='' C_GREEN='' C_YELLOW='' C_RED=''
fi

log()  { printf '%s==>%s %s\n' "${C_GREEN}" "${C_RESET}" "$*"; }
warn() { printf '%s[warn]%s %s\n' "${C_YELLOW}" "${C_RESET}" "$*" >&2; }
die()  { printf '%s[error]%s %s\n' "${C_RED}" "${C_RESET}" "$*" >&2; exit 1; }

usage() {
    sed -n '3,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

# Prints the help text, reusing the header comment so the two cannot drift apart.
parse_arguments() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -p|--prefix)
                [[ $# -ge 2 ]] || die "--prefix needs a value"
                PREFIX="$2"
                shift 2
                ;;
            --bin-dir)
                [[ $# -ge 2 ]] || die "--bin-dir needs a value"
                BIN_SUBDIR="$2"
                shift 2
                ;;
            --no-config)
                SEED_CONFIG=0
                shift
                ;;
            --offline)
                OFFLINE=1
                shift
                ;;
            -u|--uninstall)
                exec "${REPO_DIR}/uninstall.sh"
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                die "unknown option: $1 (try --help)"
                ;;
        esac
    done
}

require_rust() {
    command -v "${CARGO_BIN}" >/dev/null 2>&1 \
        || die "cargo not found. Install a Rust toolchain (https://rustup.rs) and try again."

    local version
    version="$("${CARGO_BIN}" --version 2>/dev/null || true)"
    [[ -n "${version}" ]] || die "cargo is present but not runnable: ${version}"

    log "found ${version}"

    # Edition 2024 needs a reasonably recent toolchain; warn rather than fail so an
    # older distro toolchain still gets a clear diagnostic from cargo itself.
    if "${CARGO_BIN}" verify-project --quiet >/dev/null 2>&1; then
        log "manifest accepted by this toolchain"
    else
        warn "this toolchain may be too old for edition 2024; the build below is authoritative"
    fi
}

build() {
    local -a args=(build --release)
    if (( OFFLINE )); then
        args+=(--offline)
        log "offline build; every dependency must already be in the cargo registry"
    fi
    log "building ${BINARY} (release)"
    ( cd "${REPO_DIR}" && "${CARGO_BIN}" "${args[@]}" ) \
        || die "build failed. Fix the errors above, or run with --offline once the crates are vendored."
}

install_binary() {
    local source="${REPO_DIR}/target/release/${BINARY}"
    [[ -f "${source}" ]] || die "expected ${source} to exist after a successful build"

    local target_dir="${PREFIX}/${BIN_SUBDIR}"
    mkdir -p "${target_dir}" || die "cannot create ${target_dir}"

    # Install to a temporary name and rename, so an interrupted install cannot leave a
    # half written binary on PATH.
    local staged="${target_dir}/.${BINARY}.new"
    install -m 0755 "${source}" "${staged}" || die "cannot write to ${target_dir}"
    mv -f "${staged}" "${target_dir}/${BINARY}" || die "cannot replace ${target_dir}/${BINARY}"

    log "installed ${target_dir}/${BINARY}"
}

install_config() {
    (( SEED_CONFIG )) || { log "skipping the configuration (--no-config)"; return; }

    local base="${XDG_CONFIG_HOME:-${HOME}/.config}"
    local config_dir="${base}/${CONFIG_SUBDIR}"
    local config_file="${config_dir}/config.toml"

    if [[ -e "${config_file}" ]]; then
        # Never clobber user settings.
        log "keeping the existing configuration at ${config_file}"
        return
    fi

    mkdir -p "${config_dir}" || die "cannot create ${config_dir}"
    install -m 0644 "${REPO_DIR}/config/default.toml" "${config_file}" \
        || die "cannot write ${config_file}"
    log "seeded ${config_file}"
}

check_path() {
    local bindir="${PREFIX}/${BIN_SUBDIR}"
    case ":${PATH}:" in
        *":${bindir}:"*)
            log "${bindir} is already on PATH"
            ;;
        *)
            warn "${bindir} is not on PATH."
            printf '    Add it for the current shell:\n'
            printf '        export PATH="%s:$PATH"\n' "${bindir}"
            printf '    And for future shells, add the same line to ~/.bashrc or ~/.profile.\n'
            ;;
    esac
}

main() {
    parse_arguments "$@"
    require_rust
    build
    install_binary
    install_config
    check_path
    log "done. Run '${BINARY}' to start, and press '?' inside for the key bindings."
}

main "$@"
