#!/usr/bin/env python3
"""Headless QEMU smoke test for tinyOS.

Boots the OS headless, drives the userspace shell over QMP key events, and
asserts on real command output mirrored to the serial port (the kernel's
`opt/tinyos/smoke` fw_cfg flag turns that mirror on; see kernel/src/smoke.rs).

Why this exists: nearly every runtime bug this project has hit — idle-core
wakes, console back-pressure, `ps` truncating mid-column, `run`-from-shell
hangs, the Ctrl+C kill path — is invisible to host unit tests but wedges the
live shell. Each of those makes some step below either produce truncated
output or never return to a prompt, so the step times out and the run fails.

Usage:
    python3 tools/smoke/smoke.py [--boot-timeout S] [--step-timeout S]
                                 [--overall-timeout S] -- QEMU ARG...

Everything after `--` is the QEMU command (the Makefile passes the same argv
as `make run`, plus `-display none` and the smoke fw_cfg flag). The harness
appends its own `-qmp` socket; serial is expected on QEMU's stdout
(`-serial stdio`).

Exit status 0 = PASS, non-zero = FAIL (with the captured serial log dumped).
"""

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time

# ---- character -> QEMU QKeyCode ("qcode") -----------------------------------
# Only the keys the smoke script actually types. Everything here is a single
# unmodified key; Ctrl+C is handled separately as a chord.
_QCODE = {
    " ": "spc", "\n": "ret", "/": "slash", "-": "minus", ".": "dot",
}
for _c in "abcdefghijklmnopqrstuvwxyz0123456789":
    _QCODE[_c] = _c

# Characters that require Shift, as (shift + base-qcode) chords.
_SHIFTED = {
    "&": "7", "|": "backslash", ":": "semicolon", "_": "minus", "*": "8",
}


class Qmp:
    """Minimal QMP client over a Unix socket (stdlib only)."""

    def __init__(self, path):
        self.path = path
        self.sock = None
        self.buf = b""

    def connect(self, timeout=15.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.connect(self.path)
                self.sock = s
                break
            except OSError:
                time.sleep(0.1)
        else:
            raise TimeoutError(f"QMP socket {self.path} never appeared")
        self._recv_json()                       # greeting banner
        self._cmd("qmp_capabilities")

    def _recv_json(self):
        # QMP is line-delimited JSON. Skip async 'event' messages.
        while True:
            while b"\n" not in self.buf:
                chunk = self.sock.recv(4096)
                if not chunk:
                    raise ConnectionError("QMP closed")
                self.buf += chunk
            line, self.buf = self.buf.split(b"\n", 1)
            line = line.strip()
            if not line:
                continue
            msg = json.loads(line)
            if "event" in msg:
                continue
            return msg

    def _cmd(self, execute, arguments=None):
        req = {"execute": execute}
        if arguments:
            req["arguments"] = arguments
        self.sock.sendall((json.dumps(req) + "\n").encode())
        return self._recv_json()

    def key(self, qcodes, hold_ms=80):
        keys = [{"type": "qcode", "data": q} for q in qcodes]
        self._cmd("send-key", {"keys": keys, "hold-time": hold_ms})

    def type_line(self, text, per_key=0.02):
        """Type `text` then Enter, one key at a time."""
        for ch in text:
            if ch in _SHIFTED:
                self.key(["shift", _SHIFTED[ch]])
            elif ch in _QCODE:
                self.key([_QCODE[ch]])
            else:
                raise ValueError(f"no qcode mapping for {ch!r} (extend _QCODE)")
            time.sleep(per_key)
        self.key(["ret"])


class Serial:
    """Background reader over QEMU's stdout; records and echoes every line."""

    PANIC = "*** KERNEL PANIC ***"

    def __init__(self, proc):
        self.proc = proc
        self.lines = []
        self.panic = False
        self._lock = threading.Lock()
        self._t = threading.Thread(target=self._pump, daemon=True)
        self._t.start()

    def _pump(self):
        for raw in self.proc.stdout:
            line = raw.rstrip("\n")
            with self._lock:
                self.lines.append(line)
                if self.PANIC in line:
                    self.panic = True
            print(f"  serial| {line}", flush=True)

    def wait_for(self, needle, timeout, start=0):
        """Wait until a line at index >= `start` contains `needle`.

        Returns the index just past the match (a cursor to resume from), so
        each step only scans output produced after the previous one.
        """
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self._lock:
                if self.panic:
                    raise AssertionError("kernel panic during: %r" % needle)
                for i in range(start, len(self.lines)):
                    if needle in self.lines[i]:
                        return i + 1
            if self.proc.poll() is not None:
                raise AssertionError(
                    "QEMU exited (code %s) before seeing %r"
                    % (self.proc.returncode, needle))
            time.sleep(0.05)
        raise AssertionError("timeout (%.0fs) waiting for %r" % (timeout, needle))

    def cursor(self):
        with self._lock:
            return len(self.lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--boot-timeout", type=float, default=60.0)
    ap.add_argument("--step-timeout", type=float, default=10.0)
    ap.add_argument("--overall-timeout", type=float, default=180.0)
    ap.add_argument("qemu", nargs=argparse.REMAINDER,
                    help="-- followed by the QEMU command")
    args = ap.parse_args()

    qemu = args.qemu
    if qemu and qemu[0] == "--":
        qemu = qemu[1:]
    if not qemu:
        sys.exit("error: no QEMU command given (put it after `--`)")

    tmp = tempfile.mkdtemp(prefix="tinyos-smoke-")
    qmp_path = os.path.join(tmp, "qmp.sock")
    qemu = qemu + ["-qmp", f"unix:{qmp_path},server=on,wait=off"]

    print("smoke: launching QEMU headless")
    print("smoke:   " + " ".join(qemu))
    proc = subprocess.Popen(
        qemu, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        stdin=subprocess.DEVNULL, text=True, bufsize=1)
    serial = Serial(proc)

    start = time.monotonic()
    ok = False
    try:
        # 1. Boot to a live userspace shell, with the mirror confirmed on.
        serial.wait_for("tinyos: smoke-test console mirror on", args.boot_timeout)
        serial.wait_for("tinyos: shell up", args.boot_timeout)
        # svcd: the service supervisor is boot-spawned and hosts the Nexus.
        # Assert the readiness-ordering sequence end-to-end: heartbeatd
        # publishes, and waiterd's blocking lookup only unblocks after it
        # (cursor chaining enforces the order).
        c = serial.wait_for("svcd: started", args.boot_timeout)
        c = serial.wait_for("heartbeatd: published heartbeat", args.boot_timeout, c)
        serial.wait_for("waiterd: heartbeat ready", args.boot_timeout, c)
        print("smoke: svcd supervisor + Nexus readiness ordering OK")
        cur = serial.wait_for("[out] tinyOS shell", args.boot_timeout)
        print("smoke: userspace shell is up")
        time.sleep(0.5)                          # let input settle

        qmp = Qmp(qmp_path)
        qmp.connect()
        print("smoke: QMP connected")

        def step(desc, line, *needles):
            nonlocal cur
            print(f"smoke: > {line}    ({desc})")
            qmp.type_line(line)
            for n in needles:
                cur = serial.wait_for(n, args.step_timeout, cur)

        # 2. stdin -> shell -> console -> serial round-trip.
        step("echo round-trip", "echo smokeok", "[out] smokeok")

        # 3. Long multi-line output must not truncate: the LAST help row only
        #    appears if the console back-pressure path stayed live to the end.
        step("help (no truncation)", "help", "[out] commands:", "about tinyOS")

        # 4. Directory listing reaches the filesystem and lists the apps.
        step("ls /system/apps", "ls /system/apps", " sh", " top")

        # 5. ps: the `pid` process rows print only AFTER the whole thread list.
        #    Truncation mid-thread-list (the historical bug) never reaches them.
        step("ps (full listing)", "ps", "ID  NAME", "pid ")

        # 6. Spawn + argv marshaling from userspace: `hello` echoes its args, so
        #    this exercises the SYS_PROCESS_SPAWN argument path end to end.
        pre_exec = cur
        step("run with args", "run hello alpha beta",
             "got 2 argument(s):", "[0] alpha", "[1] beta")

        # Kernel-attested identity: the process name the kernel logs on exec
        # comes from the /system/apps basename it resolved and loaded (not an argv[0]
        # claim) — see `kprintln!("tinyos: exec {name}")` in
        # kernel/src/obj/syscall.rs sys_process_exec. `hello` declares only
        # the `console` capability, so this run never opens a window and
        # needs no mouse click / focus juggling to verify. The kernel logs
        # this before the app produces any output, so scan from before the
        # step above rather than from its (later) cursor.
        serial.wait_for("tinyos: exec hello", args.step_timeout, pre_exec)
        print("smoke: kernel-attested exec name confirmed (hello)")

        # 7. Filesystem write path (only reads were covered above): write a file
        #    then read it back through cat.
        step("write a file", "write /smoke.txt persist42")
        step("read it back", "cat /smoke.txt", "[out] persist42")

        # 8. Background-job lifecycle: `&` spawns detached, and the shell reaps
        #    it on a later prompt (the reap runs one loop iteration after the
        #    child exits, hence the intervening command).
        step("background spawn", "run hello &", "] hello &")
        step("later prompt", "echo bgstep", "[out] bgstep")
        cur = serial.wait_for("hello done", args.step_timeout, cur)
        print("smoke: background job reaped")

        # Broker regression: a background child is alive (holding its OWN
        # broker-minted FS/PROC connection) while the foreground shell does FS
        # work on ITS connection. Pre-broker this shared one channel; now they
        # are isolated. Both must produce correct output.
        step("bg child + fg fs", "run hello &", "] hello &")
        step("fg write while child alive", "write /broker.txt isolated")
        step("fg read while child alive", "cat /broker.txt", "[out] isolated")

        # 9. Error path must report and return to a live prompt, not wedge.
        step("unknown app", "run nope", "run: nope: not found")

        # 10. Integration: launch a full-screen app from the shell (capability
        #     forwarding + surface), kill it with Ctrl+C (foreground kill +
        #     self-heal back to LINES), and prove the shell is alive after.
        print("smoke: > run top    (foreground full-screen app)")
        qmp.type_line("run top")
        time.sleep(1.5)                          # let top take the screen
        print("smoke: sending Ctrl+C to kill it")
        qmp.key(["ctrl", "c"])
        time.sleep(0.5)
        step("shell alive after Ctrl+C", "echo aftertop", "[out] aftertop")

        # 10b. Windowed-launch guard, two regressions in one step. `run pixels`
        #      (no `&`) execs a windowed app. It must:
        #      (a) auto-background — a windowed app never uses this console, so sh
        #          returns to the prompt instead of blocking on it (the `[N] &`
        #          job line proves sh didn't wait); and
        #      (b) not steal keyboard focus — the exec-minted window opens
        #          unfocused, so the NEXT command's keystrokes still reach sh.
        #      If either regressed, the follow-up `help` output never reaches
        #      serial (sh is blocked, or the keys went to the pixels window).
        print("smoke: > run pixels   (windowed app: auto-background, no focus steal)")
        qmp.type_line("run pixels")
        cur = serial.wait_for("] pixels &", args.step_timeout, cur)   # auto-backgrounded
        time.sleep(1.2)                          # let pixels actually open its window
        if serial.panic:
            raise AssertionError("panic running a windowed app from the shell")
        step("prompt live + focus kept after windowed launch", "help", "[out] commands:")

        # 11. Launch path: the command palette (Ctrl+K) -> `uterm` -> Enter
        #     spawns the userspace terminal. It renders to its own window
        #     (serial can't see that), so this only proves the launch path
        #     fired the kernel-side launch + spawn without panicking.
        print("smoke: > (Ctrl+K) uterm")
        qmp.key(["ctrl", "k"])
        time.sleep(0.4)
        qmp.type_line("uterm")
        cur = serial.wait_for("uterm launched", args.step_timeout, cur)
        time.sleep(0.6)   # let /system/apps/terminal spawn sh
        if serial.panic:
            raise AssertionError("panic after launching uterm")
        print("smoke: uterm launched cleanly")

        # Run a full-screen surface app (top) inside the userspace terminal and
        # quit it. Renders to uterm's window (not serial), so we only assert the
        # surface host path doesn't panic/wedge.
        print("smoke: > (in uterm) run top")
        qmp.type_line("run top")
        time.sleep(1.5)                 # let top open its surface + render frames
        qmp.key(["q"])                  # top quits on 'q' (apps/top/src/main.rs:110)
        time.sleep(0.6)
        if serial.panic:
            raise AssertionError("panic hosting a surface app in uterm")
        print("smoke: surface app hosted in uterm cleanly")

        # 12. Durability: the file must survive a real sync+reboot. `reboot`
        #     syncs then PSCI-resets; QEMU restarts the same process in place,
        #     so we wait for a second boot and read the file back.
        print("smoke: > reboot    (sync + cold reset; file must survive)")
        qmp.type_line("reboot")
        # Thread `cur` through every wait so each looks PAST the first boot —
        # otherwise these match the original boot's markers at index 0 and we'd
        # drive the shell while the guest is still resetting.
        cur = serial.wait_for("filesystem synced, rebooting", args.step_timeout, cur)
        cur = serial.wait_for("tinyos: shell up", args.boot_timeout, cur)
        cur = serial.wait_for("[out] tinyOS shell", args.boot_timeout, cur)
        print("smoke: rebooted, shell back up")
        time.sleep(0.5)
        step("file survived reboot", "cat /smoke.txt", "[out] persist42")

        # 13. Clean shutdown: sync + poweroff, then QEMU must exit on its own.
        print("smoke: > shutdown")
        qmp.type_line("shutdown")
        serial.wait_for("filesystem synced, going down", args.step_timeout, cur)
        try:
            proc.wait(timeout=10)
            print(f"smoke: QEMU exited cleanly (code {proc.returncode})")
        except subprocess.TimeoutExpired:
            raise AssertionError("guest asked to power off but QEMU never exited")

        if serial.panic:
            raise AssertionError("kernel panic somewhere in the run")
        ok = True

    except AssertionError as e:
        print(f"\nsmoke: FAIL — {e}", file=sys.stderr)
    except (TimeoutError, ConnectionError, OSError, ValueError) as e:
        print(f"\nsmoke: FAIL — harness/QMP error: {e}", file=sys.stderr)
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        elapsed = time.monotonic() - start
        print(f"smoke: {'PASS' if ok else 'FAIL'} in {elapsed:.1f}s")

    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
