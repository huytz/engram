#!/usr/bin/env bash
# End-to-end check with two simulated devices and a file:// remote. Usage: scripts/e2e.sh [path/to/engram]
set -euo pipefail

BIN=${1:-target/release/engram}
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/remote" "$T/a" "$T/b" "$T/seed-a" "$T/seed-b"

printf '# Memory Index\n\n## Rules\n- [rule A](rule-a.md) — from a\n\n## Work\n- [work X](work-x.md) — a x\n' > "$T/seed-a/MEMORY.md"
echo "rule A body" > "$T/seed-a/rule-a.md"; echo "work x body" > "$T/seed-a/work-x.md"
printf '# Memory Index\n\n## Rules\n- [rule B](rule-b.md) — from b\n\n## Other\n- [thing Y](thing-y.md) — b y\n' > "$T/seed-b/MEMORY.md"
echo "rule B body" > "$T/seed-b/rule-b.md"; echo "thing y body" > "$T/seed-b/thing-y.md"

a() { ENGRAM_HOME="$T/a" "$BIN" "$@"; }
b() { ENGRAM_HOME="$T/b" "$BIN" "$@"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

a init --device dev-a --remote "file://$T/remote" --import "$T/seed-a" --no-skills >/dev/null
cp "$T/a/identity.txt" "$T/b/"
b init --device dev-b --remote "file://$T/remote" --import "$T/seed-b" --no-skills >/dev/null
# Cases 1-7 check the synchronous prompt hook; case 8 covers background pull
sync_mode() { sed -i.bak 's/"background_pull": *true/"background_pull": false/' "$1/config.json" && rm "$1/config.json.bak"; }
sync_mode "$T/a"; sync_mode "$T/b"

# 1. both devices converge to one index
a pull >/dev/null; a push >/dev/null; b pull >/dev/null
cmp -s "$T/a/memory/MEMORY.md" "$T/b/memory/MEMORY.md" || fail "index differs after convergence"
[ "$(grep -c '^- ' "$T/b/memory/MEMORY.md")" = 4 ] || fail "expected 4 index entries"

# 2. PostToolUse hook on b → UserPromptSubmit hook on a sees the new memory
echo "z body" > "$T/b/memory/new-z.md"
echo '- [new Z](new-z.md) — b z' >> "$T/b/memory/MEMORY.md"
echo "{\"tool_name\":\"Write\",\"tool_input\":{\"file_path\":\"$T/b/memory/new-z.md\"}}" | b hook posttooluse
a hook userpromptsubmit | grep -q 'new Z' || fail "prompt hook did not surface new index line"
a hook userpromptsubmit | grep -q 'new Z' && fail "second prompt hook should not repeat index lines"
a hook userpromptsubmit | grep -q '"systemMessage":"engram pull [0-9]*ms' || fail "prompt hook timing missing"
echo '{"tool_input":{"file_path":"/etc/hosts"}}' | b hook posttooluse || fail "unrelated path must be a no-op"

# 3. index line removal propagates
sed -i.bak '/work-x/d' "$T/a/memory/MEMORY.md" && rm "$T/a/memory/MEMORY.md.bak"
a push >/dev/null; b pull >/dev/null
grep -q work-x "$T/b/memory/MEMORY.md" && fail "removed index line resurrected"

# 4. conflicting edits: newer wins, loser kept
echo "a edit" > "$T/a/memory/rule-a.md"; sleep 1.1; echo "b edit" > "$T/b/memory/rule-a.md"
a push >/dev/null; b pull | grep -q CONFLICT || fail "conflict not reported"
[ -f "$T/b/memory/rule-a.conflict-dev-a.md" ] || fail "loser copy missing"
[ "$(cat "$T/b/memory/rule-a.md")" = "b edit" ] || fail "newer edit did not win"

# 5. deletion propagates and stays deleted
rm "$T/b/memory/thing-y.md"; b push >/dev/null; a pull >/dev/null
[ ! -e "$T/a/memory/thing-y.md" ] || fail "delete did not propagate"
a push >/dev/null; b pull >/dev/null
[ ! -e "$T/b/memory/thing-y.md" ] || fail "deleted file resurrected"

# 6. prompt hook reuses device list; session start and expired TTL re-LIST
mkdir -p "$T/c" "$T/seed-c"; cp "$T/a/identity.txt" "$T/c/"
printf '# Memory Index\n\n## Rules\n- [rule C](rule-c.md) — from c\n' > "$T/seed-c/MEMORY.md"; echo "rule C body" > "$T/seed-c/rule-c.md"
ENGRAM_HOME="$T/c" "$BIN" init --device dev-c --remote "file://$T/remote" --import "$T/seed-c" --no-skills >/dev/null
a hook userpromptsubmit >/dev/null
[ -e "$T/a/memory/rule-c.md" ] && fail "prompt hook should reuse cached device list"
a hook sessionstart >/dev/null
[ -e "$T/a/memory/rule-c.md" ] || fail "session start must LIST and see new device"
echo "rule C2" > "$T/seed-c/rule-c2.md"; cp "$T/seed-c/rule-c2.md" "$T/c/memory/"; ENGRAM_HOME="$T/c" "$BIN" push >/dev/null
a hook userpromptsubmit >/dev/null
[ -e "$T/a/memory/rule-c2.md" ] || fail "known device must still be fetched on prompt"
mkdir -p "$T/d"; cp "$T/a/identity.txt" "$T/d/"
ENGRAM_HOME="$T/d" "$BIN" init --device dev-d --remote "file://$T/remote" --import "$T/seed-c" --no-skills >/dev/null
echo "rule D" > "$T/d/memory/rule-d.md"; ENGRAM_HOME="$T/d" "$BIN" push >/dev/null
sed -i.bak 's/"device_list_ttl": *[0-9]*/"device_list_ttl": 0/' "$T/a/config.json" && rm "$T/a/config.json.bak"
a hook userpromptsubmit >/dev/null
[ -e "$T/a/memory/rule-d.md" ] || fail "expired TTL must LIST and see new device"

# 7. new blobs ride inline; a reader two pushes behind uses blob GETs
echo "inline body" > "$T/b/memory/inl.md"; b push >/dev/null
rm "$T/remote/memory/dev-b/blobs/$(shasum -a 256 "$T/b/memory/inl.md" | cut -d' ' -f1)"
a pull >/dev/null; [ "$(cat "$T/a/memory/inl.md")" = "inline body" ] || fail "inline blob not applied"
echo "lag 1" > "$T/b/memory/lag1.md"; b push >/dev/null
echo "lag 2" > "$T/b/memory/lag2.md"; b push >/dev/null
a pull >/dev/null; [ "$(cat "$T/a/memory/lag1.md")" = "lag 1" ] || fail "lagging reader missed blob"
grep -q '"inline"' "$T/b/state.json" && fail "inline content leaked into state"

# 8. background pull: hook returns at once, results on the next prompt
sed -i.bak 's/"background_pull": *false/"background_pull": true/' "$T/a/config.json" && rm "$T/a/config.json.bak"
echo "bg body" > "$T/b/memory/bg.md"; echo '- [bg W](bg.md) — b w' >> "$T/b/memory/MEMORY.md"; b push >/dev/null
a hook userpromptsubmit | grep -q 'bg W' && fail "bg mode must not block on the pull"
for _ in $(seq 50); do [ -n "$(ls "$T/a/inbox/" 2>/dev/null | grep json)" ] && break; sleep 0.1; done
[ "$(cat "$T/a/memory/bg.md")" = "bg body" ] || fail "background pull did not land the file"
a hook userpromptsubmit | grep -q 'bg W' || fail "next prompt did not surface bg index line"
for _ in $(seq 50); do [ ! -e "$T/a/lock" ] && break; sleep 0.1; done

# 9. wrong identity fails cleanly
mkdir -p "$T/x"; sed 's/dev-a/intruder/' "$T/a/config.json" > "$T/x/config.json"
{ ENGRAM_HOME="$T/x" "$BIN" pull 2>&1 || true; } | grep -q "wrong identity" || fail "wrong identity should be rejected"

echo "e2e OK"
