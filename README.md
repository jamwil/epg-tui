# epg — a TUI EPG (XMLTV) viewer

A terminal Electronic Programme Guide viewer for XMLTV data, with mechanics
inspired by [csvlens](https://github.com/chenjiating/csvlens) /
[jless](https://github.com/PaulJuliusMartinez/jless): vim-style bindings,
`/` to search, `n`/`N` to jump between matches, filter mode, etc.

Large XMLTV feeds (tens of thousands of channels / hundreds of thousands of
programmes) are cached locally in a SQLite database so the TUI stays snappy and
re-opening is instant — the source is only re-fetched when you ask.

## Features

- **Vim-style navigation**: `j`/`k`, `g g`/`G`, `Ctrl-d`/`Ctrl-u`, `Ctrl-f`/`Ctrl-b`,
  `H`/`M`/`L` (top/middle/bottom of viewport), `PageUp`/`PageDown`.
- **Search**: `/` enters search; type a query and press `Enter` to confirm.
  `n`/`N` cycle through matches. The active query stays visible in the title
  bar, and matches are highlighted inline in the channel list, schedule, and
  detail views. In the channel list, it searches channel names plus only the
  currently-airing and next programme's title **and** description; in the
  schedule view, it searches programme titles and descriptions.
- **Filter**: `f` toggles "filter mode" — hide everything that doesn't match the
  current search query.
- **Channel list** shows each channel's programme count plus a live
  **Now** / **Next** column (currently-airing and next-up programmes).
- **Schedule view** (`Enter` on a channel) lists all programmes for that
  channel by start time, with the programme airing *now* highlighted in green
  and past programmes dimmed. `t` jumps to the current/next airing.
- **Detail view** (`Enter` on a programme) shows title, channel, time range,
  duration, and a description wrapped to the terminal width; scroll with
  `j`/`k`/`Ctrl-d`/`Ctrl-u`.
- **Consistent navigation**: `Enter`/`l` drills in (channels → schedule →
  detail) and `Esc` backs out everywhere. `Esc` first dismisses an active
  search/filter, then returns to the previous screen; `h`/`Backspace` go
  straight back, keeping any active search.
- **SQLite cache**: `~/.cache/epg-viewer/cache.db` (override with `EPG_DB`).
  WAL mode, pre-aggregated per-channel counts, indexed by channel + start time.
- **Streaming XMLTV parser** (quick-xml) — the 53MB / 140k-programme test feed
  imports in ~15s without holding the whole file in memory.
- **Refresh**: `r` re-downloads and re-imports from the configured source.
- `s` cycles sort mode (name ⇄ programme count).

## Install / build

```sh
cargo build --release
# binary: target/release/epg
```

## Usage

```sh
# Launch the TUI. The first run requires -u, EPG_URL, or -f.
# Subsequent launches open the cache instantly and remember a URL source.
epg

# Point at your own XMLTV URL (remembered as the source for refreshes):
epg -u 'https://your-provider/xmltv.php?...'
# or via env: EPG_URL='https://...' epg

# Import a local XMLTV file instead of fetching:
epg -f path/to/epg.xmltv

# Force a re-download before launching:
epg -r

# Use a custom cache DB:
epg --db /tmp/epg.db

# Non-interactive subcommands:
epg refresh              # download + import, no TUI
epg refresh -u 'https://...'
epg refresh -f local.xmltv
epg info                 # show cached source + counts
epg cache-path           # print the cache DB path
epg self-test            # headless render smoke test (no TTY needed)
```

## Keybindings

| Key | Channels view | Schedule view | Detail |
|---|---|---|---|
| `j` `k` `↓` `↑` | move down/up | move down/up | scroll down/up |
| `Enter` `l` `→` | open schedule | open detail | |
| `Esc` | clear search/filter | clear search/filter, else back | back to schedule |
| `h` `Backspace` `←` | | straight back to channels | straight back to schedule |
| `g g` | top | top | top (single `g`) |
| `G` | bottom | bottom | bottom |
| `t` | | jump to now/next airing | |
| `Ctrl-d` `Ctrl-u` | half page down/up | half page down/up | 10 lines down/up |
| `Ctrl-f` `Ctrl-b` `PgDn` `PgUp` | full page | full page | 20 lines |
| `H` `M` `L` | top/mid/bot of viewport | top/mid/bot of viewport | |
| `/` | search | search | |
| `n` `N` | next/prev match | next/prev match | |
| `f` | toggle filter | toggle filter | |
| `s` | cycle sort | | |
| `r` | refresh data | refresh data | |
| `?` | toggle help | toggle help | toggle help |
| `q` | quit | quit | quit |

## Data model

The XMLTV `<channel>` and `<programme>` elements are stored in two tables:

```
channels(channel_id PK, display_name, icon)
programmes(rowid PK, channel_id, start_ts, stop_ts, start_text, stop_text, title, desc)
_counts(channel_id, c)   -- pre-aggregated programme counts, rebuilt on open/import
meta(key PK, value)       -- source_url, imported_at
```

Imports are atomic: malformed or interrupted refreshes leave the previous cached
schedule intact.

Programme timestamps come from the feed's `start_timestamp`/`stop_timestamp`
attributes when present, else parsed from the `start`/`stop` XMLTV date strings
(`YYYYMMDDHHMMSS ±ZZZZ`).

## Tested against

An XMLTV provider URL (must be supplied via `EPG_URL` / `-u` — no credentials are bundled).
→ The reference feed: 4109 channel entries / 3273 unique channels / 140360 programmes, ~53MB.
