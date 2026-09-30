# 1cheri

A keyboard-driven desktop viewer for browsing 4chan boards, opening threads, and watching their attached images and videos — built around a fast, native Linux workflow rather than a browser tab.

1cheri is an unofficial, independently-developed client. It is not affiliated with, endorsed by, or officially connected to [4chan](https://www.4chan.org/) in any way. All content it displays is fetched live from 4chan and remains the property of the original posters and/or 4chan, as applicable — this project doesn't claim any ownership over it. The same disclosure, plus a couple of links, is available in-app from the "⋮" menu's "About" entry.

**This is a read-only viewer. There is no ability to post, reply, or otherwise write to any thread or board — no captcha handling, no posting UI, no API calls that create content. This is a deliberate, permanent design decision, not a missing feature: posting is not something this program does or will ever do.**

The default configured boards (`/wsg/`, `/g/`) are worksafe — nothing here is filtered or restricted, though, and any board can be added in Settings, including 18+ ones (`/gif/` is the primary target board this app was actually built around, just not something a fresh install adds for you).

## Features

- Board catalogue with thread sorting (most videos, most images, most replies, newest, most recent activity) and live search.
- Thread filtering: hide unwanted threads by word or phrase, per-board or global (applies across every board), with negation-aware matching so "no politics" doesn't trigger a hide on "politics", and singular/plural-insensitive so one entry like "trap" also catches "traps".
- Thread pinning: the same word/phrase system surfaces favorites at the top of the catalogue instead of hiding them, plus one-off temporary pin/hide flags (right-click a thread) that auto-expire after a week with no manual cleanup needed.
- Thumbnail-strip media browser with fast keyboard navigation between images and videos.
- Native video playback via libmpv — hardware decoding, mute/volume/loop persisted between runs, fixed-step seeking.
- Current post + direct replies referencing it, shown alongside the media.
- Save the current file, or every file in a thread at once ("Save all media"), with live progress feedback.
- Resume-on-startup is opt-in (off by default) — reopening the last-viewed thread only happens if you enable it in Settings, and only if you quit while still inside a thread rather than after backing out to the catalogue.
- Board directory: a "?" next to Settings' Add-a-board box opens a searchable list of every real board 4chan has (title, description, worksafe/18+ marked), fetched live — click one to add it.

## Building (Arch / Manjaro)

```sh
sudo pacman -S rust gtk4 mpv pkgconf base-devel
git clone https://github.com/subbaan/1cheri.git
cd 1cheri
cargo build --release
./target/release/1cheri
```

`rusqlite` is built with the `bundled` feature, so no separate `sqlite` package is needed — SQLite is compiled in.

## Usage

```
1cheri [OPTIONS]

  -h, --help     Print help and exit
  -v, --version  Print version and exit

```

### Keybindings

```
H/L or Up/Down  Previous / next media
Left/Right      Seek current video -5s / +5s
Space           Play or pause
M               Toggle mute
R               Restart current media
F or F11        Toggle fullscreen
S               Save current media
B or Escape     Return to the thread catalogue
/               Focus the catalogue's search box
Ctrl+Q          Quit (works from either view)
```

The viewer's top bar also has clickable Back, Save, "Save all media", and a Replies-panel visibility toggle.

### Data locations

```
~/.config/1cheri/config.toml       settings, pin/hide word lists
~/.local/share/1cheri/state.sqlite3 thread cache, resume state
~/.cache/1cheri/                    downloaded media + thumbnail cache (grows unbounded; clear manually if needed)
~/.local/state/1cheri/1cheri.log    this session's log (truncated fresh on each launch); also openable via Settings
```

## Status

Actively developed, currently at the 0.4.x "viewer refinement" stage of the roadmap described in [`project.md`](project.md) — that document is the full design history and the most accurate reference for exactly what's implemented, what was deliberately left out, and why.

## License

MIT — see [LICENSE](LICENSE).
