#!/usr/bin/env bash
#
# Remove an installed syswatch.
#
# Only files that install.sh created are removed: the binary in the chosen prefix and,
# if you agree to it, the configuration directory. Nothing else in ~/.local or
# ~/.config is touched.
#
# Usage:
#   ./uninstall.sh                 # ask before deleting the configuration
#   ./uninstall.sh --yes           # assume yes
#   ./uninstall.sh --no-config     # keep the configuration, only remove the binary
#   ./uninstall.sh --keep-config   # same as --no-config
#   ./uninstall.sh --prefix DIR    # look in a different prefix
#
set -euo pipefail

readonly PROGRAM="${0##*/}"
readonly BINARY="syswatch"

PREFIX="${HOME}/.local"
BIN_SUBDIR="bin"
CONFIG_SUBDIR="syswatch"
ASSUME_YES=0
REMOVE_CONFIG=1

if [[ -t 1 ]]; then
    readonly C_RESET=$'\033[0m'
    readonly C_BOLD=$'\033[1m'
    readonly C_GREEN=$'\033[32m'
    readonly C_RED=$'\033[31m'
else
    readonly C_RESET='' C_BOLD='' C_GREEN='' C_RED=''
fi

log()  { printf '%s==>%s %s\n' "${C_GREEN}" "${C_RESET}" "$*"; }
warn() { printf '%s[warn]%s %s\n' "${C_YELLOW:-}" "${C_RESET}" "$*" >&2; }
die()  { printf '%s[error]%s %s\n' "${C_RED}" "${C_RESET}" "$*" >&2; exit 1; }

usage() {
    sed -n '3,14p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

parse_arguments() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -p|--prefix)
                [[ $# -ge 2 ]] || die "--prefix needs a value"
                PREFIX="$2"; shift 2 ;;
            --bin-dir)
                [[ $# -ge 2 ]] || die "--bin-dir needs a value"
                BIN_SUBDIR="$2"; shift 2 ;;
            -y|--yes)
                ASSUME_YES=1; shift ;;
            --keep-config|--no-config)
                REMOVE_CONFIG=0; shift ;;
            -h|--help)
                usage; exit 0 ;;
            *)
                die "unknown option: $1 (try --help)" ;;
        esac
    done
}

confirm() {
    (( ASSUME_YES )) && return 0
    [[ -t 0 ]] || return 1
    local answer
    printf 'Remove %s? [y/N] ' "$1"
    read -r answer
    [[ "${answer}" == "y" || "${answer}" == "Y" ]]
}

remove_binary() {
    local target="${PREFIX}/${BIN_SUBDIR}/${BINARY}"
    if [[ ! -e "${target}" ]]; then
        warn "no binary at ${target}; nothing to remove"
        return 0
    fi
    if ! confirm "${target}"; then
        warn "kept ${target}"
        return 0
    fi
    rm -f -- "${target}" || die "cannot remove ${target}"
    log "removed ${target}"

    # Only clean the directory if we emptied it; a shared bin dir must survive.
    local dir="${PREFIX}/${BIN_SUBDIR}"
    if [[ -d "${dir}" ]] && [[ -z "$(ls -A "${dir}" 2>/dev/null)" ]]; then
        rmdir -- "${dir}" && log "removed the now empty ${dir}"
    fi
}

remove_config() {
    local config_dir="${XDG_CONFIG_HOME:-${HOME}/.config}/${CONFIG_SUBDIR}"

    if [[ ! -d "${config_dir}" ]]; then
        log "no configuration directory at ${config_dir}"
        return 0
    fi

    local contents
    contents="$(ls -A "${config_dir}" 2>/dev/null || true)"

    if [[ -z "${contents}" ]]; then
        rmdir -- "${config_dir}" 2>/dev/null && log "removed the empty ${config_dir}"
        return 0
    fi

    printf '%sConfiguration files in %s:%s\n' "${C_BOLD}" "${config_dir}" "${C_RESET}" >&2
    printf '  %s\n' ${contents} >&2
    if ! confirm "the configuration directory ${config_dir}"; then
        warn "kept ${config_dir}"
        return 0
    fi
    rm -rf -- "${config_dir}" || die "cannot remove ${config_dir}"
    log "removed ${config_dir}"
}

main() {
    parse_arguments "$@"
    remove_binary
    if (( REMOVE_CONFIG )); then
        remove_config
    else
        log "keeping the configuration (--keep-config)"
    fi
    log "uninstall complete"
}

main "$@"
