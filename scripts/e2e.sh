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

# 1. both devices converge to one index
a pull >/dev/null; a push >/dev/null; b pull >/dev/null
cmp -s "$T/a/memory/MEMORY.md" "$T/b/memory/MEMORY.md" || fail "index differs after convergence"
[ "$(grep -c '^- ' "$T/b/memory/MEMORY.md")" = 4 ] || fail "expected 4 index entries"

# 2. PostToolUse hook on b → UserPromptSubmit hook on a sees the new memory
echo "z body" > "$T/b/memory/new-z.md"
echo '- [new Z](new-z.md) — b z' >> "$T/b/memory/MEMORY.md"
echo "{\"tool_name\":\"Write\",\"tool_input\":{\"file_path\":\"$T/b/memory/new-z.md\"}}" | b hook posttooluse
a hook userpromptsubmit | grep -q 'new Z' || fail "prompt hook did not surface new index line"
[ -z "$(a hook userpromptsubmit)" ] || fail "second prompt hook should be silent"
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

# 6. wrong identity fails cleanly
mkdir -p "$T/x"; sed 's/dev-a/intruder/' "$T/a/config.json" > "$T/x/config.json"
{ ENGRAM_HOME="$T/x" "$BIN" pull 2>&1 || true; } | grep -q "wrong identity" || fail "wrong identity should be rejected"

echo "e2e OK"
