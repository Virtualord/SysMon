# syswatch

A fast, lightweight native terminal system monitor for Linux.

Six dashboards — CPU, memory, processes, storage, network and system — in a single
full-screen TUI, written in Rust with [ratatui](https://ratatui.rs) and
[crossterm](https://github.com/crossterm-rs/crossterm). It needs no runtime
dependencies, works entirely unprivileged, and never runs a shell command.

```
 syswatch 0.1.0  host Asus  Void Linux  kernel 6.18.54_1  up 15h 11m  load 1.69 1.97 1.80
 1 CPU │ 2 Memory │ 3 Processes │ 4 Storage │ 5 Network │ 6 System
╭───────────────────────────CPU────────────────────────────╮╭────────────────Per core─────────────────────╮
│████▉                    CPU   8%                         ││  1 █·······   8.3
│Model        Intel(R) Core(TM) i5-9300H CPU @ 2.40GHz     ││  3 ········   4.1
│Cores        4 physical / 8 logical                       ││  5 ██······  26.0
│Frequency    N/A                                          ││  7 ········   2.0
│Temperature  57.0 °C                                      ││  2 █·······  10.2
│Load         8.3%                                         ││  4 ········   6.0
╰──────────────────────────────────────────────────────────╯╰────────────────────────────────────────────╯
╭─────────CPU times──────────╮╭────────────History (last samples)────────────╮
│user                   4.0% ││100│percent                                    │
│system                 3.5% ││   │                                            │
│interrupt              0.8% ││   │                            ⠉⠉⠉⠒⠒⠒⠢⠤⠤⣀⣀⣀    │
│steal                  0.0% ││   │  ⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠁        │
│iowait                 1.0% ││   │                                            │
│idle      ▇▇▇▇▇▇▇▇▇   90.7% ││   │                                            │
╰────────────────────────────╯╰────────────────────────────────────────────╯
 CPU  │  ?:help  q:quit  Tab:view  /:search  r:refresh
```

## Contents

- [Features](#features)
- [Keyboard shortcuts](#keyboard-shortcuts)
- [Build and install](#build-and-install)
- [Distribution packages](#distribution-packages)
- [Command line](#command-line)
- [Configuration](#configuration)
- [How the numbers are calculated](#how-the-numbers-are-calculated)
- [Architecture](#architecture)
- [Development](#development)
- [Known limitations](#known-limitations)
- [License](#license)

## Features

**CPU** — total and per-core utilization, model, physical and logical core counts,
average frequency, temperature when a sensor exists, a live scrolling graph, and the
user / system / interrupt / steal / I/O wait / idle breakdown for the current
interval.

**Memory** — total, used, available, free, buffers, page cache, reclaimable slab and
shared memory, plus swap, a utilization gauge and a scrolling history. The `used`
figure deliberately excludes reclaimable cache; see
[the explanation below](#how-the-numbers-are-calculated).

**Processes** — a sortable, searchable, filterable table with PID, name, CPU%,
memory, user and state. Arrow keys and `PageUp`/`PageDown` move the selection,
`Enter` opens a details panel with the executable path, command line, parent PID and
state, and `k` opens a confirmation dialog that can send `SIGTERM` or `SIGKILL`.
Processes that exit mid-refresh are reported, never crashed on.

**Storage** — the physical disk inventory (model, firmware, bus, capacity, and each
drive's temperature against its critical threshold), then every mount from
`/proc/mounts` with capacity from `statvfs`, classified as physical, virtual, network
or pseudo so a full `tmpfs` is not mistaken for a full disk.

**Network** — per-interface receive and transmit throughput, cumulative totals, MAC
address, link state and error counters, with live graphs. Throughput is a rate
derived from two samples; totals are the counters since the link came up, and the
two are always labelled separately.

**System** — distribution from `/etc/os-release`, kernel, architecture, hostname,
uptime, load averages, task counts, and every temperature the kernel exposes.

Throughout: dark, light and monochrome themes, Unicode or ASCII graphs, colour-coded
usage, a help overlay, a bounded memory footprint, and a hard minimum terminal size
with a clear message instead of a garbled screen.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `q` | Quit |
| `Ctrl+C` | Quit |
| `?` | Toggle the help overlay |
| `Tab` / `Shift+Tab` | Next / previous dashboard |
| `1` … `6` | CPU, memory, processes, storage, network, system |
| `↑` `↓` | Move the selection |
| `PageUp` / `PageDown` | Move by a screen |
| `Home` / `End` | First / last row |
| `Enter` | Details of the selected process |
| `/` | Search processes |
| `Esc` | Close a dialog, or cancel the search |
| `c` | Sort by CPU |
| `m` | Sort by memory |
| `p` | Sort by PID |
| `n` | Sort by name |
| `s` | Reverse the sort order |
| `f` | Cycle the process filter: all → mine → tasks |
| `i` | Cycle the search field: name → pid → all |
| `k` | Open the termination dialog |
| `y` / `n` | Confirm / cancel a termination |
| `k` (in the dialog) | Switch between `SIGTERM` and `SIGKILL` |
| `r` | Force an immediate refresh |
| `g` | Toggle Unicode / ASCII graphs |
| Mouse wheel | Scroll the tables |

Pressing the key of the column that is already active reverses the sort order;
pressing a different column switches to it with its natural order (usage columns
largest first, identifiers ascending).

## Build and install

### Requirements

- Rust stable, edition 2024 (1.85 or newer; `rust-toolchain.toml` pins stable)
- A C toolchain at build time only, which `libc` and `nix` link against. No C compiler
  is needed at runtime.

Install Rust from <https://rustup.rs>, then:

```sh
git clone https://github.com/Rayuga/VirSys.git
cd VirSys
./install.sh
```

`install.sh` checks for Cargo, builds `--release`, installs the binary to
`~/.local/bin/syswatch`, and seeds `~/.config/syswatch/config.toml` **only if it does
not already exist**. It prints the `export PATH` line you need if `~/.local/bin` is
not on your `PATH`.

Useful flags:

```sh
./install.sh --prefix /usr/local   # install somewhere else
./install.sh --no-config           # do not seed a configuration
./install.sh --offline             # never touch the network
./install.sh --uninstall           # delegate to uninstall.sh
```

`make` is a thin wrapper around the same commands:

```sh
make            # debug build
make release    # optimized build
make test       # run the test suite
make all-checks # fmt --check, check, clippy -D warnings, test
make install    # ./install.sh
```

### Uninstalling

```sh
./uninstall.sh
```

It removes the binary and asks before deleting the configuration. `./uninstall.sh
--yes --keep-config` is fully non-interactive. Only files that `install.sh` created
are touched.

### Offline builds

Once the crates are in the cargo registry, the build needs no network:

```sh
cargo build --release --offline
# or
./install.sh --offline
```

`cargo vendor` and `cargo build --offline` with a `.cargo/config.toml` source
replacement work as well.

## Distribution packages

### Void Linux

```sh
xbps-install -S rust cargo        # build dependencies
git clone https://github.com/Rayuga/VirSys.git
cd VirSys && ./install.sh
```

The `xbps-src` recipe should build with the default profile; no patches are needed
because the only build-time dependencies are pure Rust plus `libc`.

### Arch Linux

```sh
sudo pacman -S --needed base-devel
git clone https://github.com/Rayuga/VirSys.git
cd VirSys && ./install.sh
```

An AUR package would declare `makedepends = ("rust" "cargo")` and no
`depends`, since syswatch has no runtime library dependencies.

### Fedora

```sh
sudo dnf install rust cargo
git clone https://github.com/Rayuga/VirSys.git
cd VirSys && ./install.sh
```

### Any other distribution

The only hard requirement is a Rust toolchain new enough for edition 2024 and a
C compiler for the build.

## Command line

```
syswatch [OPTIONS]

  -c, --config <PATH>     Path to the configuration file
  -i, --interval <SEC>    Seconds between samples (0.1 - 3600)
  -n, --no-color          Disable all colours
  -t, --theme <THEME>     dark | light | monochrome
      --view <VIEW>       cpu | memory | processes | storage | network | system
      --dump-config       Print the resolved configuration and exit
      --write-config <P>  Write the resolved configuration to P and exit
  -l, --log-level...      Verbosity; only active when SYSWATCH_LOG is set
  -h, --help              Print help
  -V, --version           Print version
```

Command line options win over the configuration file. A broken configuration is not
fatal: the defaults are used and the reason is printed.

To enable diagnostics without corrupting the display, set a filter and they go to
stderr:

```sh
SYSWATCH_LOG=syswatch=debug syswatch -ll
```

## Configuration

`~/.config/syswatch/config.toml`, honouring `XDG_CONFIG_HOME`. Every option is
documented in [`config/default.toml`](config/default.toml). An empty file is valid, a
missing file is not an error, and an unknown key is reported rather than ignored, so a
typo cannot silently disable a setting.

```toml
refresh_interval = 1.0          # seconds, clamped to 0.1 ..= 3600
theme = "dark"                  # dark | light | monochrome
default_view = "cpu"            # cpu | memory | processes | storage | network | system
graph_history = 120             # samples per graph, clamped to 16 ..= 4096
unicode_graphs = true           # false for terminals without block characters
process_filter = "all"          # all | user | tasks
network_interfaces = []         # empty means every interface except loopback
show_temperatures = true
show_disk_health = true         # physical disk inventory on the storage view
show_pseudo_filesystems = false
show_load_average = true

[process_sort]
key = "cpu"                     # cpu | memory | pid | name
descending = true
```

Out-of-range values are clamped and reported in the status bar rather than refused:
a slightly odd interval should still give you a working dashboard.

## How the numbers are calculated

A monitor that reports cumulative counters as percentages is worse than useless, so
every figure here is derived the same way.

**CPU utilization is a difference between two samples.** The counters in
`/proc/stat` are monotonic jiffies since boot. Utilization is
`delta(busy) / delta(total) * 100` over the sampling interval. The first sample after
start-up reports zero rather than a 100% spike, because there is nothing to
difference it against. The user / system / idle / I/O wait breakdown comes from
parsing the aggregate `cpu` line directly, since `sysinfo` no longer exposes those
fields.

**Memory usage excludes reclaimable cache.** Linux does not track "used" RAM, it
tracks the page cache, and a cached page occupies RAM while remaining available at
any moment. So:

```
used  = MemTotal - MemAvailable
used% = used / MemTotal * 100
```

`MemAvailable` is the kernel's own estimate of what a new workload could obtain
without swapping. On kernels older than 3.14 it does not exist and the collector
falls back to `MemFree + Buffers + Cached`. Swap uses the same idea:
`SwapTotal - SwapFree`.

**Network throughput is a rate; totals are counters.** The `RX/s` and `TX/s` columns
are `(counter_now - counter_previous) / elapsed_seconds`, using real elapsed wall
clock time rather than the number of samples. The `RX TOTAL` and `TX TOTAL` columns
are the raw counters since the interface came up. A counter that moves backwards — a
32-bit wrap, a reset, or an interface that was re-created — reports zero for that
interval instead of a negative spike.

**Filesystem capacity comes from `statvfs`.** Total is `f_blocks * f_frsize`;
available is `f_bavail * f_frsize`, which is the space an unprivileged process may
still use, so the root reserve is not counted as free. Used is the difference.

**Temperatures are optional.** They are read from `/sys/class/hwmon` through
`sysinfo`. A machine without sensors, or one where `lm-sensors` has not been run,
shows `N/A` rather than a fabricated value.

**Disk temperatures are attributed to drives, not just listed.** `/sys/class/hwmon`
reports a sensor as `nvme` with a label of `Composite`; on a machine with two identical
drives that is ambiguous. syswatch canonicalises both `/sys/block/<dev>/device` and
`/sys/class/hwmon/hwmonN/device` to the same controller node and matches on that, so
each drive shows its own temperature. Wear percentage is deliberately absent; see
[Disk health](#disk-health-and-what-it-deliberately-does-not-show).

## Disk health, and what it deliberately does not show

The storage view lists each physical drive with its model, firmware, bus, capacity and
temperature, where the temperature is shown against the drive's own critical threshold:

```
╭Disks (2)──────────────────────────────────────────────────────────────────────────────╮
│DEVICE      TYPE     MODEL                   FIRMWARE  BUS   CAPACITY    TEMP         │
│nvme0n1     SSD      INTEL HBRPEKNX0202A     G001      nvme  953.9 GiB    30.9°/80°    │
│nvme1n1     SSD      INTEL HBRPEKNX0202AO    K4110430  nvme  54.5 GiB     41.8°        │
╰──────────────────────────────────────────────────────────────────────────────────────╯
```

**What you get for free.** The kernel publishes a surprising amount about a drive
through world-readable sysfs, and syswatch reads all of it: `queue/rotational` (so SSD
and HDD are distinguished rather than guessed), capacity, model, firmware, serial,
NVMe controller state, and the temperature channels from `/sys/class/hwmon`. Each
sensor is attributed to its own drive by canonicalising both the block device's and the
hwmon device's symlinks to the same controller node, so two identical drives never
show each other's temperature.

**What it does not show, and why.** The numbers people usually want — wear percentage,
media errors, power-on hours, data units written — are in the **NVMe SMART log page**,
which is not a file. It is an NVMe admin command (`NVME_IOCTL_ADMIN_CMD`) issued
through a raw ioctl on `/dev/nvme*`, and opening that device needs root. There is no
sysfs file for it, and no crate in the dependency set exposes it.

Getting it would mean one of three things, all of which cost more than they are worth
for a dashboard:

| Option | Why not |
| --- | --- |
| Shell out to `nvme smart-log` | Needs root or a setuid helper, and breaks the promise that syswatch never runs an external command |
| Raw `ioctl` via `nix` | Adds `unsafe` code wrapping a 512-byte protocol buffer, and still needs root |
| Link `libsmartctl` | Adds a C dependency and a build-time toolchain requirement |

So the TUI stays unprivileged and dependency-free, and the full SMART log is one
command away when you actually want it:

```sh
scripts/disk-health.sh              # every NVMe device
sudo scripts/disk-health.sh         # needed if the device is not world readable
scripts/disk-health.sh --json       # machine readable
```

It wraps `nvme-cli` (NVMe) or `smartctl` (SATA/SCSI) and reports percentage used,
power-on hours, power cycles, media errors, error log entries, unsafe shutdowns,
temperature and available spare. Install the tool first:

```sh
sudo xbps-install nvme-cli     # Void
sudo pacman -S nvme-cli        # Arch
sudo dnf install nvme-cli      # Fedora
```

The critical temperature that syswatch *does* show is the one number here that
genuinely predicts a failure: it is the point at which the drive throttles or shuts
itself down to protect the data. Wear percentage is a better long-run indicator, but
it changes slowly and no display needs to refresh for it.

## Architecture

```
src/
├── main.rs             CLI parsing, bootstrap, the event loop
├── lib.rs              the library the binary and the tests share
├── app.rs              application state, actions, view routing
├── config.rs           TOML model, defaults, loading, validation
├── events.rs           key mapping, the monitoring thread, the channel
├── terminal.rs         RAII terminal guard, panic hook, signal handling
├── format.rs           human readable formatting
├── history.rs          bounded ring buffers for the graphs
├── collector/          all metric collection
│   ├── cpu.rs          /proc/stat deltas, per-core, sysinfo
│   ├── memory.rs       /proc/meminfo plus sysinfo
│   ├── processes.rs    the process table, search, sort, signals
│   ├── storage.rs      /proc/mounts plus statvfs, classification
│   ├── disks.rs        /sys/block inventory plus hwmon temperatures
│   ├── network.rs      /proc/net/dev deltas and rates
│   └── system.rs       /etc/os-release, uptime, load, sensors
└── ui/                 rendering; a pure function of the state
    ├── mod.rs          frame dispatch, small-terminal fallback
    ├── dashboard.rs    header, tab bar, status bar, search prompt
    ├── cpu.rs  memory.rs  processes.rs  storage.rs  network.rs  system.rs
    ├── dialogs.rs      help, process details, kill confirmation
    ├── widgets.rs      shared panels, gauges, graphs, helpers
    └── theme.rs        the three palettes
```

**Threading.** The main thread owns the terminal and does nothing but render and
read input. One worker thread owns the `sysinfo` handles and pushes immutable
`Snapshot` values over a bounded channel of four. The channel is written with
`try_send`: when the UI falls behind, samples are dropped rather than queued, because
stale metrics are worthless and blocking the collector would make the refresh
interval meaningless. Only the newest queued snapshot is rendered. The process table
records how many rows fit during rendering so `PageUp`/`PageDown` move by a screen.

**Refresh cadence.** Volatile metrics — CPU, memory, processes, network — are
collected every interval. Slowly changing data — mounted filesystems, sensors, static
system information — every fifth sample, which is enough for a UI and saves a
`statvfs` call per mount per second.

**Terminal safety.** A `TerminalGuard` enables raw mode and the alternate screen and
restores both on drop. A panic hook restores the terminal before the previous hook
runs. `SIGINT`, `SIGTERM` and `SIGHUP` handlers only set an atomic flag, which the
event loop polls, so shutdown always happens through the normal path and the
terminal is restored. Ratatui's damage tracking means a redraw only writes the cells
that changed, so there is no flicker.

**Dependencies.** `ratatui` and `crossterm` for the terminal, `sysinfo` for the
cross-platform metrics, `serde` and `toml` for configuration, `clap` for arguments,
`nix` for signals and `statvfs`, `thiserror` for typed errors, `tracing` for optional
diagnostics, `anyhow` for the bootstrap path, and `unicode-width` so truncation is
correct for CJK text. Standard library threads and channels; no async runtime.

## Development

```sh
make all-checks   # cargo fmt --check, cargo check, clippy -D warnings, cargo test
make run          # debug build and run
make smoke        # drive the TUI in a pseudo terminal and print the screen
```

`scripts/smoke.py` runs the release binary inside a PTY, sends real keystrokes and
renders the screen back as text. It is the only test that exercises the full stack
including crossterm and the collector thread.

The test suite is 298 tests: unit tests next to the code they cover, plus
integration tests in `tests/` that drive the public API only.

```
tests/config_tests.rs   loading, validation, round trips, the shipped default
tests/parser_tests.rs   /proc and /etc parsers, against real and malformed input
tests/network_tests.rs  rates, counter resets, interfaces appearing and vanishing
```

Every parser is tested twice: once with an exact copy of a real kernel file, and
once with malformed input, because a monitor that panics on an unexpected format is
worse than one that displays `N/A`.

## Known limitations

- **Linux only.** The parsers, signals and mount handling are Linux specific, as the
  brief requires. The code is structured so the platform layer is isolated in
  `collector/`, but other platforms are untested.
- **CPU frequency needs kernel support.** `cpu.frequency()` returns nothing on
  kernels or VMs that do not expose `cpufreq`; the field shows `N/A`.
- **Temperatures need hwmon or `lm-sensors`.** Without them the sensor section says
  so rather than guessing. Package and core temperatures are reported separately when
  the kernel exposes them.
- **No SMART wear data.** Percentage used, media errors and power-on hours are not in
  sysfs; they need an NVMe admin ioctl that requires root. Use
  `scripts/disk-health.sh`. This is deliberate, explained under
  [Disk health](#disk-health-and-what-it-deliberately-does-not-show).
- **The disk inventory is `/sys/block` only**, which lists whole disks and not
  partitions, so a drive appears once rather than once per partition. Virtual devices
  (loop, zram, device-mapper, md) are filtered out.
- **Process CPU is relative to one core.** A multi-threaded process can exceed 100%
  on a single core. The table normalises by the logical core count, so the column
  reads as a share of the whole machine; a process pinned to four cores on an
  eight-core host shows about 50%.
- **The process list is a snapshot, not a stream.** PIDs can be reused between
  samples, and a process can exit between being selected and being signalled. The
  termination dialog reports `no longer running` rather than claiming success.
- **`SIGKILL` cannot be undone** and the dialog says so.
- **Local time needs libc.** The header clock goes through `localtime_r`, the one
  documented `unsafe` block in the project. Timezone names like `Europe/Berlin` are
  resolved by libc; no timezone database is embedded.
- **Overmounts collapse.** When several filesystems are mounted on the same mount
  point, only the first entry in `/proc/mounts` is shown, matching what the user
  actually sees.
- **Refresh intervals below 200 ms are not useful.** `sysinfo` needs at least
  `MINIMUM_CPU_UPDATE_INTERVAL` between CPU refreshes; values below that behave like
  200 ms.

## License

MIT. See [LICENSE](LICENSE).
