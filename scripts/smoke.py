#!/usr/bin/env python3
"""Drive the syswatch binary inside a real PTY and dump the rendered screen.

This is a smoke test of the whole stack (crossterm input, ratatui rendering, the
collector thread) rather than of any single function, so it lives outside the Rust
test suite. Usage: scripts/smoke.py [seconds] [keys...]
"""
import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time

BINARY = os.environ.get("SYSWATCH_BIN", "target/release/syswatch")
COLUMNS = 140
ROWS = 45


def set_winsize(fd: int) -> None:
    """A PTY starts with a 0x0 window, which ratatui renders as an empty frame."""
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLUMNS, 0, 0))


def render(data: bytes) -> str:
    """Very small ANSI screen builder: enough to see the text syswatch painted."""
    grid = [[" "] * COLUMNS for _ in range(ROWS)]
    row = col = 0
    text = data.decode("utf-8", "replace")
    i = 0
    while i < len(text):
        ch = text[i]
        if ch == "\x1b":
            j = i + 1
            while j < len(text) and text[j] not in "@ABCDEFGHJKLMPSTfmnsulh":
                j += 1
            if j < len(text):
                final = text[j]
                if final == "H":
                    nums = text[i + 2 : j].split(";")
                    row = int(nums[0]) - 1 if nums[0] else 0
                    col = int(nums[1]) - 1 if len(nums) > 1 and nums[1] else 0
                i = j + 1
                continue
        if ch == "\r":
            col = 0
        elif ch == "\n":
            row += 1
        elif 0 <= row < len(grid) and 0 <= col < len(grid[row]):
            grid[row][col] = ch
            col += 1
        i += 1
    return "\n".join("".join(r).rstrip() for r in grid)


def main() -> int:
    duration = float(sys.argv[1]) if len(sys.argv) > 1 else 4.0
    keys = sys.argv[2:] or ["3", "?", "1"]

    primary, secondary = pty.openpty()
    set_winsize(secondary)
    set_winsize(primary)
    env = dict(os.environ, TERM="xterm-256color")
    proc = subprocess.Popen(
        [BINARY, "--interval", "0.5"],
        stdin=secondary,
        stdout=secondary,
        stderr=secondary,
        env=env,
    )
    os.close(secondary)

    output = b""
    deadline = time.time() + duration
    while time.time() < deadline:
        ready, _, _ = select.select([primary], [], [], 0.2)
        if ready:
            try:
                output += os.read(primary, 65536)
            except OSError:
                break

    for key in keys:
        os.write(primary, key.encode())
        time.sleep(0.6)
        ready, _, _ = select.select([primary], [], [], 0.4)
        if ready:
            try:
                output += os.read(primary, 65536)
            except OSError:
                break

    # Any open dialog swallows the quit key, which is the intended behaviour, so
    # dismiss it first and then quit.
    os.write(primary, b"\x1b")
    time.sleep(0.4)
    os.write(primary, b"q")
    # Keep draining: an undrained PTY fills its buffer and blocks the child's writes,
    # which would look like a hang that is not there.
    end = time.time() + 2.0
    while time.time() < end and proc.poll() is None:
        ready, _, _ = select.select([primary], [], [], 0.2)
        if ready:
            try:
                output += os.read(primary, 65536)
            except OSError:
                break
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        print(render(output))
        print("FAIL: syswatch did not exit on 'q'", file=sys.stderr)
        return 1

    screen = render(output)
    print(screen)
    print(f"\n--- exit code: {proc.returncode} ---")
    if proc.returncode != 0:
        print("FAIL: non-zero exit", file=sys.stderr)
        return 1
    for expected in ("syswatch", "CPU", "Memory", "Processes"):
        if expected not in screen:
            print(f"FAIL: {expected!r} missing from the rendered screen", file=sys.stderr)
            return 1
    print("OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
