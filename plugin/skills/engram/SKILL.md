---
name: engram
description: Save, update or recall persistent memory shared across devices and agents (Claude Code, Codex) in ~/.engram/memory, and operate the engram sync (status, sync, setup, hook timing). Use when the user says "remember this", "save to memory", "forget X", corrects how you work, when you learn a durable fact worth keeping, or when memory looks stale or out of sync between machines.
---

# engram memory

Memory is a directory of Markdown files, `~/.engram/memory/` (or `$ENGRAM_HOME/memory`), synced
across devices by engram hooks. Claude Code and Codex read the same files. Do not edit the
remote bucket or `~/.engram/state.json` by hand.

## Layout

- `MEMORY.md`: the index, one line per memory: `- [Title](file.md) — short hook`. It is loaded at
  session start, so keep lines short and never put memory content in it.
- One file per fact, kebab-case name, with frontmatter:

```markdown
---
name: <kebab-case-slug>
description: <one-line summary used to judge relevance>
metadata:
  type: user | feedback | project | reference
---

<the fact. For feedback/project add **Why:** and **How to apply:** lines. Link related memories with [[their-name]].>
```

Types: `user` = who the user is and their preferences; `feedback` = how to work (corrections and
approaches that worked); `project` = ongoing work and constraints not in the code (use absolute
dates); `reference` = pointers to dashboards, tickets, URLs.

## Saving

1. Look for an existing file on the topic first (`grep -ril <word> ~/.engram/memory`). Update it
   rather than adding a duplicate; delete memories that turned out to be wrong.
2. Write the file, then add or adjust its line in `MEMORY.md`. Append new lines to the section
   they fit in.
3. Don't save secrets, what the repo already records (code, git history) or what only matters
   to this conversation.

Pushing is automatic: the hooks push after each edit and at the end of every turn. Run
`engram push` if you need the change on other devices right away.

## Recalling

Read the index first, then open only the topic files whose lines are relevant. Treat memories as
background that was true when written: check that a file, flag or host it names still exists
before acting on it.

## Conflicts

`MEMORY.md` is merged line by line. Other conflicts keep the newer file and save the other one as
`<name>.conflict-<device>.md`: merge the two by hand, delete the conflict file, then push.

## Operating engram

`engram` is on the Bash `PATH` while the plugin is enabled in Claude Code. If the command is not
found (for example in Codex), use `~/.engram/bin/engram-<version>`.

| command | does |
|---|---|
| `engram status` | devices, sequence numbers, local pending changes |
| `engram sync` | pull then push now |
| `engram hook-timing on\|off` | show or hide `engram pull 412ms · …` after each hook (default off) |
| `engram init --device <name> --remote r2://<bucket> …` | first-time setup on a device |
| `engram install-hooks --plugin` | set Claude Code `autoMemoryDirectory` and remove the old non-plugin hooks |

Errors from hooks never block the agent. They go to `~/.engram/engram.log`, so read it when sync
looks stuck.
