#!/usr/bin/env python3
# Benchmark engram binaries against the remote in ~/.engram/config.json. Usage: BENCH_ROUNDS=10 scripts/bench.py BIN [BIN...]
import json, os, random, shutil, statistics, subprocess, sys, tempfile, time

BINS = [os.path.abspath(b) for b in sys.argv[1:]] or sys.exit("usage: bench.py BIN [BIN...]")
ROUNDS = int(os.environ.get("BENCH_ROUNDS", "10"))
REAL = os.path.expanduser("~/.engram")
CFG = json.load(open(f"{REAL}/config.json"))
TAG = f"engram-bench-{int(time.time())}"
FILES, SIZE = 400, 5000
T = tempfile.mkdtemp(prefix="engram-bench-")
results = {}  # scenario -> bin -> [ms]


def run(bin_, args, home=None, stdin=None):
    env = dict(os.environ)
    if home:
        env["ENGRAM_HOME"] = home
    t = time.perf_counter()
    p = subprocess.run([bin_, *args], env=env, input=stdin, capture_output=True, text=True)
    ms = (time.perf_counter() - t) * 1000
    if p.returncode:
        sys.exit(f"{bin_} {args} failed: {p.stderr}")
    return ms


def rec(name, label, ms):
    results.setdefault(name, {}).setdefault(label, []).append(ms)


def fill(d, n, seed):
    os.makedirs(d, exist_ok=True)
    rnd = random.Random(seed)
    for i in range(n):
        with open(f"{d}/note-{i:04d}.md", "w") as f:
            f.write("".join(rnd.choice("abcdefgh ") for _ in range(SIZE)))


def touch(d, n, it):
    for i in range(n):
        with open(f"{d}/note-{i:04d}.md", "a") as f:
            f.write(f"\nedit {it}")


def home(label, dev, col, data):
    h = f"{T}/{label}-{dev}"
    os.makedirs(h, exist_ok=True)
    shutil.copy(f"{REAL}/identity.txt", h)
    c = dict(CFG, device=dev, collections=[{"name": col, "path": data, "exclude": []}])
    json.dump(c, open(f"{h}/config.json", "w"))
    return h


bins = {f"b{i}": b for i, b in enumerate(BINS)}


def order(r):
    # Rotate binaries each round so network drift does not favour one
    k = list(bins)
    return k[r % len(k):] + k[:r % len(k)]
try:
    # Read path on the real setup: per-prompt hook and SessionStart-style full pull (no-op, nothing written remotely)
    for r in range(ROUNDS):
        for label in order(r):
            b = bins[label]
            rec("prompt hook (pull memory)", label, run(b, ["hook", "userpromptsubmit"]))
            rec("full pull (5 collections)", label, run(b, ["pull"]))

    # Write path in an isolated home + private bench collection, removed afterwards
    for label, b in bins.items():
        col = f"{TAG}-{label}"
        a_data, b_data = f"{T}/{label}-a-data", f"{T}/{label}-b-data"
        fill(a_data, FILES, 1)
        ha, hb = home(label, "bench-a", col, a_data), home(label, "bench-b", col, b_data)
        rec(f"initial push ({FILES} files)", label, run(b, ["push", "--collection", col], ha))
        rec(f"initial pull ({FILES} files)", label, run(b, ["pull", "--collection", col], hb))
    for r in range(ROUNDS):
        for label in order(r):
            b, col = bins[label], f"{TAG}-{label}"
            a_data, b_data = f"{T}/{label}-a-data", f"{T}/{label}-b-data"
            ha, hb = f"{T}/{label}-bench-a", f"{T}/{label}-bench-b"
            rec("push no-op", label, run(b, ["push", "--collection", col], ha))
            touch(a_data, 1, r)
            rec("push 1 file", label, run(b, ["push", "--collection", col], ha))
            rec("pull 1 file", label, run(b, ["pull", "--collection", col], hb))
            touch(a_data, 20, r)
            rec("push 20 files", label, run(b, ["push", "--collection", col], ha))
            rec("pull 20 files", label, run(b, ["pull", "--collection", col], hb))
finally:
    env = dict(os.environ, AWS_ACCESS_KEY_ID=CFG["access_key_id"], AWS_SECRET_ACCESS_KEY=CFG["secret_access_key"],
               AWS_DEFAULT_REGION="auto", AWS_REQUEST_CHECKSUM_CALCULATION="when_required")
    bucket = CFG["remote"].removeprefix("s3://").rstrip("/")
    for label in bins:
        subprocess.run(["aws", "s3", "rm", f"s3://{bucket}/{TAG}-{label}/", "--recursive", "--quiet",
                        "--endpoint-url", CFG["endpoint"]], env=env)
    shutil.rmtree(T, ignore_errors=True)


def pct(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))]


for label, b in bins.items():
    print(f"{label} = {b}")
head = "".join(f"{l + ' p50':>10}{l + ' p90':>10}" for l in bins)
print(f"\n{'scenario (ms)':<28}{head}   ({ROUNDS} rounds)")
for name, by in results.items():
    cells = "".join(f"{statistics.median(by[l]):>10.0f}{pct(by[l], 0.9):>10.0f}" for l in bins)
    print(f"{name:<28}{cells}")
