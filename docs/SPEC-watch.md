# Spec: `brainjar watch` — Polling File Watcher

**Status:** Draft
**Date:** 2026-04-02
**Issue:** #1

## Overview

A simple polling watcher that periodically checks for file changes and runs sync. No filesystem events, no debouncing — just poll, diff, sync, sleep.

## CLI

```bash
brainjar watch                    # foreground, 5m default interval
brainjar watch --interval 60      # check every 60 seconds
brainjar watch --kb my-notes      # watch specific KB only
brainjar watch --daemon           # background, writes PID file
brainjar watch --stop             # kills running daemon
```

## Poll Cycle

1. For each auto_sync KB (or `--kb` if specified):
   a. Collect files from watch_paths
   b. Hash and diff against DB
   c. If changes detected: run full sync (upsert, chunk, embed, extract)
   d. Log: `[14:15:03] test-corpus: 3 files changed, synced in 2.1s`
2. Sleep for `interval` seconds
3. Repeat

## Config

```toml
[watch]
interval = 300    # seconds, default 5 minutes
```

CLI `--interval` overrides config value.

## Lock File

Location: `{data_dir}/{kb_name}.lock`

- Created at start of sync, deleted on completion
- Contains PID + timestamp
- If lock exists and PID is alive: skip sync, log warning
- If lock exists but PID is dead: remove stale lock, proceed
- Protects against: watcher + manual `brainjar sync` running simultaneously

## Daemon Mode

`--daemon`:
- Fork to background
- Write PID to `{data_dir}/brainjar-watch.pid`
- Redirect stdout/stderr to `{data_dir}/brainjar-watch.log`
- Exit parent process

`--stop`:
- Read PID from `{data_dir}/brainjar-watch.pid`
- Send SIGTERM
- Remove PID file
- If PID not running: clean up stale PID file

## Plugin Integration

The OpenClaw plugin spawns `brainjar watch` as a child process (not daemon mode). Plugin manages lifecycle:
- Start on gateway boot (if auto_sync KBs exist)
- Kill on gateway shutdown (child process dies with parent)
- No PID file needed in this mode

## Output

Foreground mode:
```
🔭 Watching 2 knowledge bases (interval: 5m)
   test-corpus: ./test-corpus (16 docs)
   glitch-memory: ~/.openclaw/homes/glitch (119 docs)

[14:15:03] Checking for changes...
[14:15:03] test-corpus: 3 files changed
[14:15:05] test-corpus: synced (3 docs, 18 chunks, 18 embeddings) in 2.1s
[14:15:05] glitch-memory: no changes
[14:20:03] Checking for changes...
[14:20:03] No changes detected
```

JSON mode (`--json`):
```json
{"timestamp":"2026-04-02T14:15:05Z","kb":"test-corpus","changes":3,"chunks":18,"embeddings":18,"duration_ms":2100}
```

## Signals

- SIGTERM/SIGINT: graceful shutdown (finish current sync, then exit)
- SIGHUP: reload config and re-scan watch paths

## Edge Cases

- Empty watch_paths: skip KB, log warning
- API key missing: sync fails for that KB, continue watching others
- All KBs fail: keep running (might recover if key is set later)
- Interval < 10s: warn but allow (user's choice)

## Testing

- Unit: lock file create/check/cleanup
- Unit: stale lock detection (dead PID)
- Integration: poll cycle detects file changes
- Integration: daemon start/stop/PID file lifecycle
