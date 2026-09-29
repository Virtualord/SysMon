#!/usr/bin/env bash
#
# Collect a snapshot of the system metrics syswatch displays, as JSON.
#
# The TUI is the product, but a machine readable snapshot is what makes syswatch
# scriptable: it can be polled from a status bar, logged, or compared over time
# without attaching a terminal.
#
# The data is read from the same sources the TUI uses, in the same order, so the two
# cannot disagree by more than one refresh interval.
#
# Usage:
#   scripts/collect.sh                     # human readable summary
#   scripts/collect.sh --json              # one JSON object
#   scripts/collect.sh --pretty            # indented JSON
#   scripts/collect.sh --cpu-only          # only the CPU section
#
set -euo pipefail

readonly PROC_STAT=/proc/stat
readonly MEMINFO=/proc/meminfo
readonly NET_DEV=/proc/net/dev
readonly LOADAVG=/proc/loadavg
readonly OS_RELEASE=/etc/os-release

if [[ -t 1 ]]; then
    readonly C_RESET=$'\033[0m' C_BOLD=$'\033[1m'
else
    readonly C_RESET='' C_BOLD=''
fi

OUTPUT=json
PRETTY=0
SECTIONS="cpu memory storage network system"

die() { printf 'collect.sh: %s\n' "$*" >&2; exit 1; }

# Reads the aggregate `cpu` line of /proc/stat. Fields are jiffies since boot.
read_cpu() {
    [[ -r ${PROC_STAT} ]] || { printf '0 0 0 0 0 0 0 0\n'; return; }
    awk '/^cpu  /{ printf "%s %s %s %s %s %s %s %s\n", $2,$3,$4,$5,$6,$7,$8,$9; exit }' "${PROC_STAT}"
}

# The first "cpu " line holds the machine-wide totals; the rest are per core.
read_cpu_cores() {
    [[ -r ${PROC_STAT} ]] || return 0
    awk '/^cpu[0-9]/ { printf "%s %s\n", $1, $2+$3+$4+$6+$7+$8+$9 }' "${PROC_STAT}" \
        | awk '{ total = $1 + $2 + $5; if (total > 0) printf "%.1f\n", ($2 / total) * 100 }'
}

# Prints `key: value` lines from a whitespace separated /proc file.
meminfo_value() { awk -v key="$1" '$1 == key":" { print $2 * 1024; found = 1; exit } END { if (!found) print 0 }' "${MEMINFO}"; }

human_bytes() {
    # `i` rather than `index`: awk has a built-in function by that name.
    awk -v bytes="$1" 'BEGIN {
        split("B KiB MiB GiB TiB PiB", unit, " ")
        i = 1
        while (bytes >= 1024 && i < 6) { bytes /= 1024; i++ }
        if (i == 1) printf "%d %s", bytes, unit[i]
        else printf "%.1f %s", bytes, unit[i]
    }'
}

has_section() { [[ " ${SECTIONS} " == *" $1 "* ]]; }

usage() {
    sed -n '3,15p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --json)     OUTPUT=json; shift ;;
        --pretty)   OUTPUT=json; PRETTY=1; shift ;;
        --human)    OUTPUT=human; shift ;;
        --cpu-only) SECTIONS="cpu"; shift ;;
        -h|--help)  usage; exit 0 ;;
        *)          die "unknown option: $1 (try --help)" ;;
    esac
done

[[ -r ${PROC_STAT} ]] || die "cannot read ${PROC_STAT}; is this Linux?"

read -r user nice system idle iowait irq softirq steal <<<"$(read_cpu)"
total=$(( user + nice + system + idle + iowait + irq + softirq + steal ))
if (( total > 0 )); then
    busy=$(( user + nice + system + irq + softirq + steal ))
    busy_pct=$(awk -v b="${busy}" -v t="${total}" 'BEGIN { printf "%.1f", b / t * 100 }')
    idle_pct=$(awk -v i="${idle}" -v t="${total}" 'BEGIN { printf "%.1f", i / t * 100 }')
    # These are fractions of the whole lifetime, not of the last interval; the TUI
    # reports the interval because that is the only meaningful figure for a live view.
    usage_pct=$(awk -v u="$(( user + nice ))" -v t="${total}" 'BEGIN { printf "%.1f", u / t * 100 }')
    sys_pct=$(awk -v s="${system}" -v t="${total}" 'BEGIN { printf "%.1f", s / t * 100 }')
    wait_pct=$(awk -v w="${iowait}" -v t="${total}" 'BEGIN { printf "%.1f", w / t * 100 }')
else
    busy_pct=0; idle_pct=0; usage_pct=0; sys_pct=0; wait_pct=0
fi
core_count=$(awk '/^cpu[0-9]/ { count++ } END { print count + 0 }' "${PROC_STAT}")

# Memory. "used" excludes reclaimable page cache, the same way the TUI computes it.
mem_total=$(meminfo_value MemTotal)
mem_available=$(meminfo_value MemAvailable)
mem_free=$(meminfo_value MemFree)
mem_cached=$(meminfo_value Cached)
mem_buffers=$(meminfo_value Buffers)
mem_used=$(( mem_total - mem_available ))
mem_pct=$(awk -v u="${mem_used}" -v t="${mem_total}" 'BEGIN { if (t > 0) printf "%.1f", u / t * 100; else print 0 }')
swap_total=$(meminfo_value SwapTotal)
swap_free=$(meminfo_value SwapFree)
swap_used=$(( swap_total - swap_free ))

# Network: cumulative counters only. A rate needs two samples, which this one-shot
# script does not take, so the numbers are labelled as totals and not as throughput.
rx_total=0; tx_total=0; ifaces=0
if [[ -r ${NET_DEV} ]]; then
    while read -r name rest; do
        [[ "${name}" == "lo:" || "${name}" == "face" || "${name}" == "Inter-|" ]] && continue
        [[ "${name}" == *:* ]] || continue
        set -- ${rest}
        rx_total=$(( rx_total + ${1:-0} ))
        tx_total=$(( tx_total + ${9:-0} ))
        ifaces=$(( ifaces + 1 ))
    done < "${NET_DEV}"
fi

read -r load1 load5 load15 _ < "${LOADAVG}" 2>/dev/null || { load1=0; load5=0; load15=0; _=''; }
uptime_seconds=$(awk '{ print int($1) }' /proc/uptime 2>/dev/null || echo 0)
distro=$(awk -F= '/^PRETTY_NAME=/{ gsub(/"/, "", $2); print $2 }' "${OS_RELEASE}" 2>/dev/null || echo unknown)
kernel=$(uname -r)
hostname=$(uname -n)

if (( PRETTY )); then
    readonly SEP=': '
else
    readonly SEP=':'
fi

if [[ ${OUTPUT} == human ]]; then
    printf '%sCPU%s\n' "${C_BOLD}" "${C_RESET}"
    printf '  cores:      %s logical\n' "${core_count}"
    printf '  usage:      %s%% (user %s%%, system %s%%)\n' "${busy_pct}" "${usage_pct}" "${sys_pct}"
    printf '  idle:       %s%%\n' "${idle_pct}"
    printf '  iowait:     %s%%\n' "${wait_pct}"
    printf '  note:       percentages are fractions of the time since boot, not the last second\n'
    if has_section memory; then
        printf '%sMEMORY%s\n' "${C_BOLD}" "${C_RESET}"
        printf '  total:      %s\n' "$(human_bytes "${mem_total}")"
        printf '  used:       %s (%s%%)\n' "$(human_bytes "${mem_used}")" "${mem_pct}"
        printf '  available:  %s\n' "$(human_bytes "${mem_available}")"
        printf '  cached:     %s\n' "$(human_bytes "${mem_cached}")"
        printf '  buffers:    %s\n' "$(human_bytes "${mem_buffers}")"
        printf '  swap:       %s / %s\n' "$(human_bytes "${swap_used}")" "$(human_bytes "${swap_total}")"
    fi
    if has_section storage; then
        printf '%sSTORAGE%s\n' "${C_BOLD}" "${C_RESET}"
        df -h -x tmpfs -x devtmpfs 2>/dev/null | awk 'NR == 1 { next } { print "  " $1, $2, $3, $4, $5 }'
    fi
    if has_section network; then
        printf '%sNETWORK%s\n' "${C_BOLD}" "${C_RESET}"
        printf '  interfaces: %s\n' "${ifaces}"
        printf '  received:   %s total\n' "$(human_bytes "${rx_total}")"
        printf '  transmitted:%s total\n' " $(human_bytes "${tx_total}")"
        printf '  note:       totals only; rates need two samples over time\n'
    fi
    if has_section system; then
        printf '%sSYSTEM%s\n' "${C_BOLD}" "${C_RESET}"
        printf '  host:       %s\n' "${hostname}"
        printf '  distro:     %s\n' "${distro}"
        printf '  kernel:     %s\n' "${kernel}"
        printf '  uptime:     %s seconds\n' "${uptime_seconds}"
        printf '  load:       %s %s %s\n' "${load1}" "${load5}" "${load15}"
    fi
    exit 0
fi

# JSON. Values are numbers where they are measurements and strings where they are
# labels, so the output can be fed to jq without post-processing.
# Emits `"key": value`, with a trailing comma unless `last` is 1.
emit() {
    local key="$1" value="$2" last="${3:-0}"
    if [[ ${PRETTY} == 1 ]]; then
        if (( last )); then
            printf '    "%s": %s\n' "${key}" "${value}"
        else
            printf '    "%s": %s,\n' "${key}" "${value}"
        fi
    elif (( last )); then
        printf '"%s":%s' "${key}" "${value}"
    else
        printf '"%s":%s,' "${key}" "${value}"
    fi
}

# A string value with the quotes and the minimal escaping JSON requires.
json_string() {
    local text="$1"
    text="${text//\\/\\\\}"
    text="${text//\"/\\\"}"
    printf '"%s"' "${text}"
}

# True when `name` is the last enabled section, so it must not be followed by a comma.
# The whole remainder of the list has to be scanned: the first match is not enough,
# because an earlier section may also be enabled.
is_last_section() {
    local name="$1" section
    local seen=0
    for section in ${SECTIONS}; do
        if (( seen )); then
            return 1
        fi
        [[ ${section} == "${name}" ]] && seen=1
    done
    (( seen ))
}

# Opens a section object, indented for pretty output.
open_section() {
    local name="$1"
    if (( PRETTY )); then
        printf '\n  "%s": {\n' "${name}"
    else
        printf '"%s":{' "${name}"
    fi
}

# Closes a section object, adding the comma when another section follows.
close_section() {
    local name="$1"
    if (( PRETTY )); then
        if is_last_section "${name}"; then
            printf '  }\n'
        else
            printf '  },\n'
        fi
    else
        if is_last_section "${name}"; then
            printf '}'
        else
            printf '},'
        fi
    fi
}

printf '{'
if has_section cpu; then
    open_section cpu
    emit "logical_cores" "${core_count}"
    emit "busy_percent" "${busy_pct}"
    emit "user_percent" "${usage_pct}"
    emit "system_percent" "${sys_pct}"
    emit "idle_percent" "${idle_pct}"
    emit "iowait_percent" "${wait_pct}"
    emit "since_boot" true 1
    close_section cpu
fi
if has_section memory; then
    open_section memory
    emit "total_bytes" "${mem_total}"
    emit "used_bytes" "${mem_used}"
    emit "available_bytes" "${mem_available}"
    emit "free_bytes" "${mem_free}"
    emit "cached_bytes" "${mem_cached}"
    emit "buffers_bytes" "${mem_buffers}"
    emit "swap_total_bytes" "${swap_total}"
    emit "swap_used_bytes" "${swap_used}"
    emit "used_percent" "${mem_pct}" 1
    close_section memory
fi
if has_section storage; then
    # `df -B1` gives the same statvfs numbers the TUI shows, without needing root.
    # The first line of a filesystem is a sample; the totals come from the summary.
    storage_total=0; storage_used=0; storage_avail=0; fs_count=0
    if command -v df >/dev/null 2>&1; then
        while read -r blocks used available _; do
            [[ ${blocks} =~ ^[0-9]+$ ]] || continue
            storage_total=$(( storage_total + blocks ))
            storage_used=$(( storage_used + used ))
            storage_avail=$(( storage_avail + available ))
            fs_count=$(( fs_count + 1 ))
        done < <(df -B1 --output=size,used,avail -x tmpfs -x devtmpfs 2>/dev/null | tail -n +2)
    fi
    storage_pct=$(awk -v u="${storage_used}" -v t="$(( storage_total ))" 'BEGIN { if (t > 0) printf "%.1f", u / t * 100; else print 0 }')
    open_section storage
    emit "filesystem_count" "${fs_count}"
    emit "total_bytes" "${storage_total}"
    emit "used_bytes" "${storage_used}"
    emit "available_bytes" "${storage_avail}"
    emit "used_percent" "${storage_pct}" 1
    close_section storage
fi
if has_section network; then
    open_section network
    emit "interface_count" "${ifaces}"
    emit "received_total_bytes" "${rx_total}"
    emit "transmitted_total_bytes" "${tx_total}" 1
    close_section network
fi
if has_section system; then
    open_section system
    emit "hostname" "$(json_string "${hostname}")"
    emit "distribution" "$(json_string "${distro}")"
    emit "kernel" "$(json_string "${kernel}")"
    emit "uptime_seconds" "${uptime_seconds}"
    emit "load_one" "${load1}"
    emit "load_five" "${load5}"
    emit "load_fifteen" "${load15}" 1
    close_section system
fi
printf '}\n'
