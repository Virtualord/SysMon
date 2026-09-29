#!/usr/bin/env bash
#
# Full SMART data for the drives syswatch lists.
#
# syswatch deliberately shows only what the kernel publishes in sysfs, because the
# wear numbers everyone actually wants live behind an NVMe admin ioctl that needs root.
# This script shells out to the official tools so you can get them on demand, without
# turning the TUI into something that spawns processes and escalates privileges.
#
# Usage:
#   scripts/disk-health.sh              # every NVMe device
#   scripts/disk-health.sh /dev/nvme0   # one device
#   scripts/disk-health.sh --json      # machine readable
#
# Requires: nvme-cli (or smartmontools) and read access to the device, usually root.
#
set -uo pipefail

if [[ -t 1 ]]; then
    readonly C_RESET=$'\033[0m' C_BOLD=$'\033[1m'
    readonly C_GREEN=$'\033[32m' C_RED=$'\033[31m' C_YELLOW=$'\033[33m'
else
    readonly C_RESET='' C_BOLD='' C_GREEN='' C_RED='' C_YELLOW=''
fi

OUTPUT=human
DEVICES=()

die() { printf 'disk-health.sh: %s\n' "$*" >&2; exit 1; }

usage() {
    sed -n '3,15p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --json) OUTPUT=json; shift ;;
        -h|--help) usage; exit 0 ;;
        -*) die "unknown option: $1 (try --help)" ;;
        *) DEVICES+=("$1"); shift ;;
    esac
done

# Colour a percentage by how much of the drive's rated write endurance is used.
# The NVMe spec counts this from 0% upwards, so 100% means the drive has consumed the
# endurance it was warrantied for; it does not mean the drive has failed.
wear_colour() {
    local used="$1"
    awk -v u="$used" 'BEGIN {
        if (u >= 100) exit 2
        if (u >= 50) exit 1
        exit 0
    }'
}

wear_paint() {
    local used="$1"
    case "$(wear_colour "${used}"; echo $?)" in
        2) printf '%s' "${C_RED}${used}%%${C_RESET}" ;;
        1) printf '%s' "${C_YELLOW}${used}%%${C_RESET}" ;;
        *) printf '%s%%' "${used}" ;;
    esac
}

# Collects the NVMe device paths to report on.
discover() {
    if (( ${#DEVICES[@]} > 0 )); then
        printf '%s\n' "${DEVICES[@]}"
        return
    fi
    local found=()
    # /sys/class/nvme lists controllers, which is the namespace that nvme-cli expects.
    for controller in /sys/class/nvme/nvme*; do
        [[ -e "${controller}" ]] || continue
        found+=("/dev/$(basename "${controller}")")
    done
    if (( ${#found[@]} == 0 )); then
        for disk in /sys/block/nvme*n1; do
            [[ -e "${disk}" ]] || continue
            found+=("/dev/$(basename "${disk}")")
        done
    fi
    (( ${#found[@]} > 0 )) || die "no NVMe devices found; pass a device explicitly"
    printf '%s\n' "${found[@]}"
}

json_escape() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }

emit_nvme() {
    local device="$1"
    local output
    if ! output="$(nvme smart-log "${device}" 2>&1)"; then
        printf 'nvme %s: %s\n' "${device}" "${output}" >&2
        return 1
    fi
    # `nvme smart-log` prints "Some Key Name:      value", and the key names contain
    # spaces, so fields are matched on the key with spaces, underscores and hyphens
    # removed and case folded. An exact match keeps "Temperature" from also matching
    # "Temperature Sensor 1".
    field() {
        printf '%s' "${output}" | awk -v want="$1" '
            BEGIN { key = tolower(want); gsub(/[ _-]/, "", key) }
            {
                head = $0
                sub(/:.*/, "", head)
                head = tolower(head)
                gsub(/[ _-]/, "", head)
                if (head == key) {
                    value = $0
                    sub(/^[^:]*:[ \t]*/, "", value)
                    sub(/[ \t]+$/, "", value)
                    gsub(/,/, "", value)
                    print value
                    exit
                }
            }'
    }

    # `nvme smart-log` prints values with their units attached — "3%", "37 Celsius",
    # "100%" — so numeric fields are stripped to digits and a decimal point.
    numeric() { printf '%s' "$1" | tr -cd '0-9.'; }

    local model firmware serial used hours power media unsafe shutdowns temp spare
    model="$(field 'Model Number')"
    firmware="$(field 'Firmware Version')"
    serial="$(field 'Serial Number')"
    used="$(numeric "$(field 'Percentage Used')")"
    hours="$(numeric "$(field 'Power On Hours')")"
    power="$(numeric "$(field 'Power Cycles')")"
    media="$(numeric "$(field 'Media and Data Integrity Errors')")"
    unsafe="$(numeric "$(field 'Error Information Log Entries')")"
    shutdowns="$(numeric "$(field 'Unsafe Shutdowns')")"
    temp="$(numeric "$(field 'Temperature')")"
    spare="$(numeric "$(field 'Available Spare')")"

    if [[ ${OUTPUT} == json ]]; then
        printf '  {"device":"%s","model":"%s","firmware":"%s","serial":"%s",' \
            "$(json_escape "${device}")" \
            "$(json_escape "${model}")" \
            "$(json_escape "${firmware}")" \
            "$(json_escape "${serial}")"
        printf '"percentage_used":%s,"power_on_hours":%s,"power_cycles":%s,' \
            "${used:-null}" "${hours:-null}" "${power:-null}"
        printf '"media_errors":%s,"error_log_entries":%s,"unsafe_shutdowns":%s,' \
            "${media:-null}" "${unsafe:-null}" "${shutdowns:-null}"
        printf '"temperature_c":%s,"available_spare_pct":%s}' \
            "${temp:-null}" "${spare:-null}"
    else
        printf '%s%s%s  %s\n' "${C_BOLD}" "${device}" "${C_RESET}" "${model:-unknown}"
        printf '  firmware:         %s\n' "${firmware:-N/A}"
        printf '  serial:           %s\n' "${serial:-N/A}"
        printf '  percentage used:  %s\n' "$(wear_paint "${used:-?}")"
        printf '  power on hours:   %s\n' "${hours:-N/A}"
        printf '  power cycles:     %s\n' "${power:-N/A}"
        printf '  media errors:     %s\n' "${media:-N/A}"
        printf '  error log:        %s\n' "${unsafe:-N/A}"
        printf '  unsafe shutdowns: %s\n' "${shutdowns:-N/A}"
        printf '  temperature:      %s C\n' "${temp:-N/A}"
        printf '  spare remaining:  %s %%\n' "${spare:-N/A}"
    fi
}

emit_sata() {
    local device="$1"
    local output
    if ! output="$(smartctl -A -H "${device}" 2>&1)"; then
        printf 'smartctl %s: %s\n' "${device}" "${output}" >&2
        return 1
    fi
    # SATA is not attribute-indexed like NVMe; the wear indicators differ per vendor.
    local wear line value
    wear="$(printf '%s' "${output}" | awk '
        /Percent_Lifetime_Used|PERCENT_LIFETIME_USED/ { print $NF }
        /Wear_Leveling_Count|Media_Wearout_Indicator/ { print $NF }
    ' | head -1)"
    if [[ ${OUTPUT} == json ]]; then
        printf '  {"device":"%s","percentage_used":%s}' \
            "$(json_escape "${device}")" "${wear:-null}"
    else
        printf '%s%s%s  (SATA/SCSI)\n' "${C_BOLD}" "${device}" "${C_RESET}"
        printf '  percentage used: %s\n' "$(wear_paint "${wear:-?}")"
        while IFS= read -r line; do
            printf '  %s\n' "${line}"
        done < <(printf '%s' "${output}" | awk -F' *: *' 'NF == 2 { printf "%-28s %s", $1, $2 }')
    fi
}

main() {
    local nvme_tool=0
    command -v nvme >/dev/null 2>&1 && nvme_tool=1
    if (( ! nvme_tool )) && ! command -v smartctl >/dev/null 2>&1; then
        printf 'Neither `nvme` nor `smartctl` is installed.\n\n' >&2
        printf '  Void Linux:  sudo xbps-install nvme-cli\n' >&2
        printf '  Arch Linux:  sudo pacman -S nvme-cli\n' >&2
        printf '  Fedora:      sudo dnf install nvme-cli\n\n' >&2
        printf 'Both tools need read access to the device, which normally means root.\n' >&2
        exit 1
    fi

    # Reading a SMART log is a privileged operation; say so once, clearly.
    if [[ ! -r /dev/nvme0 && ! -r /dev/sda ]]; then
        printf 'note: reading SMART data normally requires root; try: sudo %s\n\n' "$0" >&2
    fi

    [[ ${OUTPUT} == json ]] && printf '[\n'
    local first=1
    local device
    while IFS= read -r device; do
        [[ -e "${device}" ]] || { printf 'no such device: %s\n' "${device}" >&2; continue; }
        (( first )) || { [[ ${OUTPUT} == json ]] && printf ',\n'; }
        first=0
        case "$(basename "${device}")" in
            nvme*) (( nvme_tool )) && emit_nvme "${device}" || emit_sata "${device}" ;;
            *) command -v smartctl >/dev/null 2>&1 && emit_sata "${device}" ;;
        esac
    done < <(discover)
    [[ ${OUTPUT} == json ]] && printf '\n]\n'
}

main "$@"
