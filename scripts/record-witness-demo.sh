#!/usr/bin/env bash
# Record the witness-debugger demo as an asciinema v2 cast
# (docs/demo/witness-debugger.cast), fully offline: the debugger runs in a
# local PTY over the checked-in canonical fixtures, a scripted key
# schedule walks the RC001 hang and the RC002 mask mismatch, and the
# captured output becomes the cast. Play it with `asciinema play` (or any
# v2-compatible player); re-run this script after an intentional UI change.
#
# This is a demo recorder, not a test: timing here is sleep-based on
# purpose (a demo needs pacing); the §9 no-sleep policy applies to tests.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/docs/demo/witness-debugger.cast"

cargo build -q -p reconverge-tui
mkdir -p "$ROOT/docs/demo"

TUI="$ROOT/target/debug/reconverge-tui" \
FIXTURES="$ROOT/fixtures/witness" \
CAST_OUT="$OUT" \
python3 - <<'EOF'
import fcntl, json, os, pty, select, struct, sys, termios, time

tui = os.environ["TUI"]
fixtures = os.environ["FIXTURES"]
out_path = os.environ["CAST_OUT"]
width, height = 80, 24

# The walkthrough: step to the RC001 barrier, watch the hang verdict land,
# rewind, jump to the divergence, then the RC002 witness and its mask
# panel. Pacing is for human eyes.
#
# The step counts come from the fixture rather than being written here. A
# hard-coded "l" x4 "to the barrier" outlived the hand-written five-step
# fixtures it was counted against: after the fixtures were recorded from a
# real run (three steps) the cast still showed "step 0/5" and lib.rs:20,
# lines no current artifact has.
def load(name):
    with open(os.path.join(fixtures, name)) as f:
        return json.load(f)

rc001 = load("rc001-divergent-barrier.json")
rc002 = load("rc002-partial-mask.json")
steps = len(rc001["steps"])
# Position p shows step p-1 (position 0 is the launch), so the barrier at
# step index b is b+1 presses away.
barrier = next(i for i, st in enumerate(rc001["steps"])
               if st["statement"].startswith("sync_threads()"))
to_barrier = [(1.5 if i == 0 else 0.9, "l") for i in range(barrier + 1)]
to_verdict = [(2.2, "l")] * (steps - (barrier + 1))
schedule = to_barrier + to_verdict + [
    (2.8, "g"),                                        # rewind
    (1.2, "d"),                                        # jump to the split
    (2.0, "n"),                                        # the RC002 witness
    (1.2, "v"),                                        # its verdict
    (1.6, "h"),                                        # the mask panel
    (3.0, "q"),
]

master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
pid = os.fork()
if pid == 0:
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    os.dup2(slave, 0)
    os.dup2(slave, 1)
    os.dup2(slave, 2)
    os.close(master)
    os.environ["TERM"] = "xterm-256color"
    os.chdir(fixtures)
    os.execv(tui, [tui, "witness",
                   "rc001-divergent-barrier.json", "rc002-partial-mask.json"])
os.close(slave)

start = time.monotonic()
events = []
deadline = start + 60
keys = list(schedule)
next_key = start + keys[0][0] if keys else None

while True:
    now = time.monotonic()
    if now > deadline:
        os.kill(pid, 9)
        sys.exit("demo recording ran away")
    timeout = max(0.0, (next_key - now)) if next_key else 0.25
    ready, _, _ = select.select([master], [], [], min(timeout, 0.25))
    if ready:
        try:
            data = os.read(master, 65536)
        except OSError:
            break
        if not data:
            break
        events.append([round(time.monotonic() - start, 4), "o",
                       data.decode("utf-8", "replace")])
    if next_key and time.monotonic() >= next_key:
        _, key = keys.pop(0)
        os.write(master, key.encode())
        next_key = (time.monotonic() + keys[0][0]) if keys else None

os.waitpid(pid, 0)

# Hold the recording to the fixtures before it replaces the checked-in
# cast: every step counter on screen must name a fixture's length, and the
# walk must have reached the RC001 verdict. The stream is a diff — after
# the first frame only changed cells are written — so the counter is read
# off a replayed grid, not searched for in the bytes. Cursor positioning
# and text are all the views emit; every other sequence is skipped.
import re

def frames(events):
    grid = [[" "] * width for _ in range(height)]
    row = col = 0
    for _, _, data in events:
        i = 0
        while i < len(data):
            ch = data[i]
            if ch == "\x1b":
                m = re.match(r"\x1b\[([0-9;?]*)([@-~])", data[i:])
                if m:
                    if m.group(2) == "H":
                        parts = (m.group(1) or "1;1").split(";")
                        row = int(parts[0] or 1) - 1
                        col = int(parts[1] or 1) - 1 if len(parts) > 1 else 0
                    i += m.end()
                else:
                    i += 2
                continue
            if ch == "\r":
                col = 0
            elif ch == "\n":
                row = min(row + 1, height - 1)
            elif 0 <= row < height and 0 <= col < width:
                grid[row][col] = ch
                col += 1
            i += 1
        yield "\n".join("".join(r) for r in grid)

seen = set()
for frame in frames(events):
    seen.update((int(a), int(b)) for a, b in re.findall(r"step (\d+)/(\d+)", frame))
lengths = {len(rc001["steps"]), len(rc002["steps"])}
totals = {b for _, b in seen}
if not totals or not totals <= lengths:
    sys.exit(f"cast shows step totals {sorted(totals)}; the fixtures have {sorted(lengths)}")
if (steps, steps) not in seen:
    sys.exit(f"the walk never reached step {steps}/{steps}, the RC001 verdict")
header = {
    "version": 2,
    "width": width,
    "height": height,
    "title": "reconverge witness — replaying a divergent barrier, lane by lane",
    "env": {"TERM": "xterm-256color", "SHELL": "/bin/bash"},
}
with open(out_path, "w") as f:
    f.write(json.dumps(header) + "\n")
    for event in events:
        f.write(json.dumps(event) + "\n")
print(f"wrote {out_path} ({len(events)} events)")
EOF
