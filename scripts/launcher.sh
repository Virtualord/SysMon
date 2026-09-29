#!/usr/bin/env bash
#
# Launch syswatch in a controlled terminal environment.
#
# This wrapper exists so a desktop shortcut or an alias gets a predictable window
# size and a sane locale, without syswatch itself having to know about terminals.
# It also refuses to start when stdout is not a terminal, where a full screen TUI
# would write escape sequences into a file.
#
# Usage:
#   scripts/launcher.sh              # run syswatch in the current terminal
#   scripts/launcher.sh --view cpu   # any syswatch argument is passed through
#
set -euo pipefail

readonly REPO_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
readonly BINARY="${SYSWATCH_BIN:-${REPO_DIR}/target/release/syswatch}"

if [[ -t 1 ]]; then
    readonly C_RED=$'\033[31m' C_RESET=$'\033[0m'
else
    readonly C_RED='' C_RESET=''
fi

die() { printf '%ssyswatch:%s %s\n' "${C_RED}" "${C_RESET}" "$*" >&2; exit 1; }

# A TUI needs a real terminal on both ends.
[[ -t 0 && -t 1 ]] || die "not attached to a terminal; run syswatch from an interactive shell"

if [[ ! -x "${BINARY}" ]]; then
    die "${BINARY} is not built. Run 'make release' or './install.sh' first."
fi

# 24-bit colour and UTF-8 are what the Unicode graphs and the dark theme assume.
export TERM="${TERM:-xterm-256color}"
export COLORTERM="${COLORTERM:-truecolor}"

# A predictable size keeps the layout stable across terminals with odd defaults.
if [[ -z "${COLUMNS:-}" || -z "${LINES:-}" ]]; then
    COLUMNS=80
    LINES=24
    if command -v stty >/dev/null 2>&1; then
        # `stty size` prints "rows columns".
        if read -r detected_rows detected_columns < <(stty size 2>/dev/null); then
            [[ "${detected_columns}" =~ ^[0-9]+$ ]] && COLUMNS="${detected_columns}"
            [[ "${detected_rows}"    =~ ^[0-9]+$ ]] && LINES="${detected_rows}"
        fi
    fi
    export COLUMNS LINES
fi

# 120 columns by 32 rows is the smallest size where every panel is readable.
if (( COLUMNS < 60 || LINES < 20 )); then
    printf 'warning: terminal is %sx%s; syswatch needs at least 60x20 to be useful\n' \
        "${COLUMNS}" "${LINES}" >&2
fi

# exec so syswatch owns the terminal: signals and the exit code pass straight through.
exec "${BINARY}" "$@"
