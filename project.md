# 1cheri

**Project document version:** 0.3.2 (revised to match the actual implementation as of application version 0.4.5; §18.5 additionally re-curated by the user for 0.5.0 planning)  
**Initial application version:** 0.1.0  
**Target platform:** Linux desktop  
**Primary target environment:** Manjaro Linux with XFCE  
**Primary implementation:** Rust, GTK4, libmpv, SQLite

> This document originally described the intended design before implementation began. Several areas were deliberately simplified or changed once real use (and real `/gif/` data) showed what actually mattered; most of this revision updates those sections to describe what was built. §18.5 and the non-goals in §4 are the exception: those describe future/deferred work rather than what's been built, so they're not "corrected" the same way -- §18.5 was separately re-curated by the user to reflect what's actually still wanted for 0.5.0, trimming several items that were never revisited.

## 1. Project summary

1cheri is a keyboard-driven desktop application for browsing selected 4chan boards, opening threads, and viewing their attached media.

The application is intended primarily as a video browser. Board navigation, thread information, comments, filtering, and downloading exist to support a fast and comfortable media-viewing workflow. (The original design also named a full watched-state tracking system as a supporting pillar; it was deliberately not built -- see §12.)

The first target board is `/gif/`, although users must be able to configure and switch between any boards they choose.

The application is Linux-only. Cross-platform compatibility is not a project goal.

## 2. Core product identity

The application should feel like a desktop video gallery built around 4chan threads, rather than a conventional web browser or a complete clone of the 4chan website.

The central workflow is:

1. Select a configured board.
2. Browse the board's current threads.
3. Filter, sort, or hide unwanted threads.
4. Open a thread.
5. Browse its image and video posts through a thumbnail strip.
6. View the selected image or video in a large media area.
7. Display the selected post's comment and direct replies referencing that post.
8. Move rapidly between media items using the keyboard.
9. Save selected media.
10. Preserve mute, filtering, and navigation preferences between runs. (Not watched-state -- see §12.)

## 3. Primary goals

The application must prioritise:

- Reliable WebM and video playback.
- Fast keyboard navigation.
- Strong user-controlled filtering.
- Clear thread and media metadata.
- Persistent mute and volume settings.
- Current-post context with useful replies.
- Efficient caching and prefetching.
- A clean native Linux interface.
- A maintainable Rust codebase suitable for continued AI-assisted development.

## 4. Non-goals

The initial application will not include:

- Posting or replying to threads.
- Captcha handling.
- User accounts.
- Cross-platform support.
- Mobile support.
- Support for unrelated imageboard engines.
- Cloud synchronisation.
- Remote classification services.
- Automatic mass-downloading of every opened thread.

These may be reconsidered later, but they must not complicate the initial architecture.

## 5. Technology choice

### 5.1 Rust

Rust will be used for:

- Application logic.
- Network requests.
- API models.
- Filtering.
- Cache management.
- SQLite access.
- Download management.
- Media metadata probing.
- GTK integration.
- libmpv integration.

The project should use a normal Cargo workspace or a small, clean Cargo package. Avoid excessive crate fragmentation during the early stages.

### 5.2 GTK4

GTK4 will provide:

- Main application window.
- Board navigation.
- Thread catalogue.
- Thumbnail strip.
- Media viewport container.
- Comment panel.
- Filter editor.
- Settings windows.
- Keyboard input.
- Native file dialogs.
- Notifications and status messages.

The interface should be designed for XFCE and ordinary Linux window managers. It does not need to mimic GNOME design conventions closely.

### 5.3 libmpv

libmpv will be the media playback engine.

It must handle:

- WebM playback.
- Other supported video formats.
- Hardware decoding where available.
- Audio mute and volume.
- Playback position.
- Seeking.
- Looping.
- Playback speed.
- Fullscreen or maximised viewing.
- Playback events.
- End-of-file detection.
- Media duration.
- Audio-track detection.

Images may be displayed using GTK image facilities or mpv. The implementation should choose the simplest reliable route while presenting a consistent viewer.

Of the list above, two capabilities mpv itself provides were never actually wired up to any application feature: **playback speed** (§9) and **audio-track detection/switching** (no multi-track UI exists; mpv just plays whatever track it auto-selects). Everything else in the list is used.

### 5.4 SQLite

**As built, this is narrower than originally planned.** There is no per-media (per-post-attachment) tracking at all -- no watched state, no per-item playback position, no per-item hidden/saved flags. Everything is tracked at the thread level instead:

- Cached thread/catalogue metadata (subject, comment preview, thumbnail, reply/image/video counts, last-modified).
- Per-thread resume position (the post number of the last media item viewed).
- Per-thread video count (only knowable once the full thread has been fetched).
- Per-thread "new media since last visit" count, for the catalogue's "+N new" badge (§12).
- Per-thread temporary pin/hide flag with an expiry timestamp (§11).
- Last selected board and thread (a small key/value table).

There is no filter-hit history table -- "explain why hidden" (§11.6) is computed live by re-running the word match, not looked up from a log, since matching is cheap and this avoids keeping a second source of truth in sync.

Configuration that users may edit manually remains in a human-readable configuration file (§5.5).

#### 5.4.1 Actual schema

```sql
CREATE TABLE threads (
    board                  TEXT NOT NULL,
    number                 INTEGER NOT NULL,
    subject                TEXT,
    comment_preview        TEXT NOT NULL DEFAULT '',
    thumbnail_url          TEXT,
    replies                INTEGER NOT NULL DEFAULT 0,
    images                 INTEGER NOT NULL DEFAULT 0,
    video_count            INTEGER,          -- NULL until the thread has been opened once
    last_modified          INTEGER NOT NULL,
    last_opened_at         INTEGER,
    last_media_post        INTEGER,          -- resume position within the thread
    last_seen_media_count  INTEGER,          -- media count as of the last open, for the "+N new" badge
    temp_flag_kind         TEXT,             -- 'pin' | 'hide' | NULL
    temp_flag_at           INTEGER,          -- unix timestamp the flag was set, for auto-expiry
    PRIMARY KEY (board, number)
);

CREATE TABLE app_state (
    key   TEXT PRIMARY KEY,       -- "active_board", "active_thread", ...
    value TEXT NOT NULL
);
```

No `boards`, `media`, `filters`, `filter_conditions`, or `filter_hits` tables exist. Boards are just a `Vec<String>` in `config.toml` (§5.5); pin/hide word lists are also in `config.toml` (§11), not the database -- only the *temporary*, auto-expiring per-thread flags live in SQLite, since those need a timestamp and don't belong in a file the user hand-edits.

### 5.5 Configuration format

Use TOML for configuration.

Suggested path:

```text
~/.config/1cheri/config.toml
```

Suggested data path:

```text
~/.local/share/1cheri/
```

Suggested cache path:

```text
~/.cache/1cheri/
```

Suggested database path:

```text
~/.local/share/1cheri/state.sqlite3
```

## 6. Main application views

## 6.1 Board and thread catalogue

The user must be able to configure a list of boards.

Example board navigation:

```text
/gif/   /wsg/   /wg/   /g/   +
```

The user must be able to:

- Add a board.
- Remove a board.
- Reorder boards.
- Select a board.
- Refresh the current board.
- Remember the last active board.

Threads should be displayed as compact cards or rows.

Each thread entry displays:

- OP thumbnail.
- Subject.
- Beginning of OP comment.
- Thread number.
- Reply count.
- Image count.
- Video count (once the thread has been opened at least once; unknown before that, since the catalogue API doesn't break attachment counts down by type).
- New-media-since-last-visit count, as a "+N new" badge (§12) -- there is no true per-item unseen/watched count.
- Last activity time (relative, e.g. "6m ago").
- Pinned badge and highlight, or hidden reason, depending on filter match (§11) -- there is no separate "watched" state to display.

The thread catalogue supports these sorting modes:

- Most videos (default).
- Most images.
- Most replies.
- Newest thread.
- Most recent activity.

"Bump order," "most unseen media," and "unwatched first" were dropped: bump order isn't exposed by the catalogue API without extra per-thread fetches, and both unseen-based modes depended on the full per-item watched-state system in §12, which was deliberately not built. Most-videos is the default and remains the important mode for the primary `/gif/` workflow.

## 6.2 Thread media viewer

The viewer should contain:

- A large media display area.
- A scrollable thumbnail strip.
- Thread summary information.
- Current media information.
- Current post comment.
- Direct replies referencing the current post.
- Playback controls.
- Save status.
- Filter status.

Suggested layout:

```text
┌──────────────────────────────────────────────────────────────────────┐
│ /gif/  Thread title                31 videos · 46 files · 83 posts   │
├────────────────┬─────────────────────────────────────────────────────┤
│ [thumbnail 01] │                                                     │
│ [thumbnail 02] │                                                     │
│ [thumbnail 03] │                 VIDEO OR IMAGE                      │
│ [thumbnail 04] │                                                     │
│ [thumbnail 05] │                                                     │
│ [thumbnail 06] │                                                     │
│                ├─────────────────────────────────────────────────────┤
│                │ Post details, comment, and referenced replies      │
├────────────────┴─────────────────────────────────────────────────────┤
│ Previous · Pause · Next · Mute · Loop · Save · Comments · Filter    │
└──────────────────────────────────────────────────────────────────────┘
```

The default thumbnail position is on the left. It is not configurable (no right/bottom placement option was built).

The selected thumbnail remains visible as the user moves through media, positioned a little above vertical centre (not dead centre) -- this leaves more of the strip visible below the current item to preview upcoming media, while keeping the last item or two above still in view.

"Filter status" and "Save status" in the layout above are less prominent than originally sketched: there is no persistent filter indicator in the viewer itself (filtering is thread-level only, decided before a thread is opened -- see §11), and save status is a transient status-bar message (e.g. "Saved to ...") rather than a persistent element. The bottom bar is primarily a keybinding hint strip, doubling as the "help overlay" mentioned in §10.

## 7. Comment behaviour

The default comment mode must be:

> Current post with direct replies that reference it.

This is required because the original comment may contain questions such as "source?", while the useful answer commonly appears in replies.

For selected post number `123456789`, the application should:

1. Display the selected post's comment.
2. Scan the thread for posts containing a reference to `>>123456789`.
3. Display those direct replies below the selected post.
4. Preserve the chronological order of replies.
5. Allow references inside displayed replies to be opened or previewed.

Recursive reply expansion should be user-triggered rather than automatic during the first implementation.

**Only the default mode was built.** There is no mode switching (§10's C/Shift+C were never implemented) and no way to hide the comment panel or expand to the entire thread -- current post with direct referenced replies is the only behaviour, always on. This covered the actual need well enough that the other three modes were never revisited.

Post HTML must be sanitised and converted into a safe internal representation. Support should include:

- Line breaks.
- Quote links.
- Greentext.
- Spoilers.
- Ordinary links.
- Basic emphasis where present.

## 8. Media handling

The application should classify attachments as:

- Video.
- Animated image.
- Static image.
- Unsupported media.

**Not implemented as originally scoped.** The thumbnail strip always shows every media item in the thread, in thread order; there is no live media-type filter switcher (videos only / images only / unseen only / saved only / OP media only). Sorting the *catalogue* by most-videos (§6.1) covers the main practical need this was meant to address.

For each media item, the application stores or derives:

- Post number.
- Original filename.
- Extension.
- Media URL (derived from the post's `tim` + extension).
- Thumbnail URL.
- Width.
- Height.
- File size.

Not tracked, unlike the original plan: duration, audio presence (neither is probed -- no media-inspection job exists, per the §19.5 risk note that flagged this as expensive), watched state, per-item playback position, per-item saved state, per-item hidden state, or per-item filter matches. Filtering only ever applies to whole threads (§11); there is no individual-media hide/pin.

## 9. Video playback behaviour

The video player supports:

- Autoplay when a media item is selected (configurable).
- Persistent mute state.
- Persistent volume.
- Pause and resume.
- Seeking (fixed ±5 second jumps via Left/Right; no drag-scrub keybinding, though the on-screen scrub bar is directly draggable with the mouse).
- Restarting.
- Looping (configurable, applies to every video; not a per-item or live-toggle setting).
- Fullscreen media.
- Hardware decoding when available (`hwdec=auto`).

Not implemented, and not currently planned:

- **Playback-speed adjustment.** Explicitly descoped during 0.4.0 planning as unnecessary for this application's actual use.
- **Automatic advance.** There is no "play next when this one ends" option; `keep-open` is enabled so mpv holds on the last frame instead.
- **Resume-from-position as a toggle.** Resuming is not a video-level feature at all -- what's remembered and restored is which *thread* and which *media item within it* you were last on, not a mid-video timestamp. This is not configurable; it always happens.

Mute is remembered between application runs, alongside volume, loop, and autoplay.

Actual default settings (see §15 for the full current config shape):

```toml
muted = true
volume = 70
loop_video = true
autoplay = true
```

## 10. Keyboard controls

The entire primary workflow is usable from the keyboard. The actual bindings ended up meaningfully different from the original sketch below -- most notably, **Left/Right seek within the current video rather than changing media**, which is the reverse of the original design:

```text
H or L          Previous / next media (moves the thumbnail-strip selection)
Up or Down      Previous / next media (native list navigation -- same effect as H/L)
Page Up/Down    Jump 10 media items back / forward, clamped to the ends (not wrapping)
Home or End     Jump to the first / last media item
Left or Right   Seek current video -5s / +5s
Space           Play or pause
M               Toggle mute
R               Restart current media
F or F11        Toggle fullscreen media
S               Save current media
B or Escape     Return to thread catalogue
/               Focus the catalogue's search box (catalogue only)
Ctrl+Q          Quit (works from either view)
```

The catalogue's own thread list additionally supports Page Up/Down (jump 10 threads) and Home/End (jump to the first/last thread) the same way -- Up/Down there is native GtkListBox keynav (no code needed), but paging and jump-to-edge aren't part of GtkListBox's native keynav, so those two are wired explicitly, the same as the viewer's.

Revised after 0.4.x use: quit moved from a bare `Q` to `Ctrl+Q`, and made global (works from the catalogue too, not just the viewer -- previously the app's only key controller was viewer-session-scoped, so `Q` silently did nothing from the catalogue). Bare `Q` was also a latent conflict with the catalogue's search box, where `q` is an ordinary character to type. `/`-to-focus-search was added the same pass, a common convention on Linux (browsers, mutt, less...). The viewer also has a persistent top bar now (§6.2) with clickable Back, Save, and "Save all media" controls duplicating the B/Esc and S bindings, plus a Replies visibility toggle with no keybinding equivalent.

Bindings that did not make it into the implementation: Shift+M (volume mode), a dedicated loop-toggle key (looping is a config/settings-window setting only), `[`/`]` (previous/next thread from the viewer), Shift+S (save as), C/Shift+C (comment panel is always shown, in its one default mode -- see §7), I (media info is always shown, not toggleable), X/Shift+X/U (hide/undo -- see §11.5 for what replaced this), and O (open original thread in browser).

There is no configurable keybinding system; bindings are centralised in one match statement in the viewer's key controller, per the "at minimum" fallback this section originally allowed.

There is no dedicated help overlay. The bottom status bar in the viewer permanently shows the active bindings instead (see §6.2), which covers the same need without a separate screen.

## 11. Filtering system

**This section describes a substantially simpler system than originally planned**, built and refined through actual `/gif/` use rather than the rule-engine design below. The core diagnosis held up: the primary pain point on `/gif/` is unwanted categories of threads cluttering the catalogue, and the first priority was letting the user hide entire threads quickly, based on subject and OP comment text. What changed is *how*: instead of a general condition/operator rule engine, filtering is two flat per-board word/phrase lists (pin and hide) plus, later, a separate temporary per-thread flag system. Filtering based on reply bodies, and media-level (individual attachment) filtering, were never built at all -- not even the single-item "hide this media" quick action originally planned as the minimal secondary feature.

**Pinning was added and is not in the original design at all.** It turned out to matter as much as hiding: some threads are worth surfacing at the top of the catalogue (highlighted), not just excluded. Pin and hide are symmetric, board-scoped word/phrase lists, with pin taking priority when both would otherwise match.

## 11.1 Filter scopes

Filtering only ever applies to whole threads. There is no media-level (per-attachment) filtering -- the "secondary" scope from the original design was never built, including the minimal single-item hide action. Filtering by poster ID, filename pattern, or reply-comment text remains out of scope; these were on the original §18.5 wish-list but were dropped from it during the 0.5.0 planning revision, not merely deferred further.

## 11.2 Thread filters

In practice, thread filters only ever inspect **subject and OP comment text** -- not reply count, image count, video count, or thread age as originally sketched; those turned out not to be needed once word-based subject/comment filtering existed, and adding them would mean building the general condition engine described in the original §11.6, which was deliberately avoided.

Matching is word/phrase based, with one important refinement discovered through real use: **negation-awareness**. A naive substring match on a hide word like "BBC" would also hide a thread that says "NO BBC" -- an explicit exclusion, the opposite of what the hide word is for. The matcher tokenizes the text and checks a short lookback window before each match for negation markers (e.g. "no", "not", "without"); a negated occurrence doesn't count as a match unless a separate, unnegated occurrence exists elsewhere in the same text. This is thread-catalogue-specific: only the OP's subject and first post are scanned, not every reply, matching the original design's reasoning that replies aren't scanned in bulk.

Two word lists exist per board, both user-editable through a Filters window (word/phrase entries, add/remove, rendered as removable chips -- there is no condition/operator/field picker):

- **Pin words**: a match highlights the thread and sorts it to the top of the catalogue.
- **Hide words**: a match hides the thread (moved into a separate "Hidden (N)" toggle view rather than removed outright).

A second pair of lists, **global pin/hide words** (added after 0.4.x use), apply across every board rather than one -- shown in the same Filters window, above the current board's own lists -- and are unioned into the board-specific lists before matching, rather than getting their own precedence tier. This means a board-specific pin word naturally overrides a global hide word for the same term (see the precedence rule below), with no separate global-vs-board override logic needed.

Typing multiple words/phrases separated by `|` or `,` when adding (e.g. `foo | bar, baz`) adds each as its own independent entry in one step -- a pure input convenience, not a grouping concept; there's no relationship between entries added this way and ones added individually.

Precedence, highest to lowest: a temporary hide flag (§11.5) always wins; then any pin signal (pin word match -- global or board-specific -- or a temporary pin flag) wins over hide; then a hide word match (global or board-specific); otherwise the thread is shown normally.

## 11.3 Media filters

**Not built.** Not even the minimal single "hide this media" quick action originally planned as sufficient for the first release. Rule-based media filters (duration, resolution, aspect ratio, audio presence, duplicate hash) were originally deferred to §18.5 for a good reason that still holds -- several of those fields need the media file probed first, which needs a job queue that doesn't exist (§19.5) -- but were dropped from §18.5's list entirely during the 0.5.0 planning revision, not merely deferred further.

## 11.4 Filter actions

Implemented:

- **Hide** (word-based, permanent until the word is removed; or temporary, see §11.5).
- **Pin** (word-based, permanent; or temporary, see §11.5) -- not part of the original design.

Not implemented: "skip during navigation," since there's no media-level hide for Next/Previous to need to skip past. Collapsing, dimming, marking, priority adjustment, and auto-mute remain unbuilt, as originally planned.

## 11.5 Quick filtering actions

There is no `X`/`Shift+X`/`U` keyboard scheme. Instead, right-clicking a thread row in the catalogue opens a context menu with four actions:

```text
Pin matching this subject      Adds the thread's subject as a permanent pin word (§11.2)
Hide matching this subject     Adds the thread's subject as a permanent hide word (§11.2)
Temporarily pin thread         Flags just this thread as pinned, no word involved
Temporarily hide thread        Flags just this thread as hidden, no word involved
```

**Temporary pin/hide is a feature not in the original design.** It covers the gap between "this thread happens to match a word" and "I want this specific thread pinned or hidden right now, without adding a permanent rule." A temporary flag is a single tag on one thread (mutually exclusive: pinned or hidden, not both), stored with a timestamp, and auto-expires after seven days (a wide margin over any real thread's lifetime) via a startup sweep and an hourly background check -- no manual cleanup needed, though a "Temporary" toolbar button opens a panel listing everything currently flagged, with a manual Remove option per entry. Temporary hide replaces what earlier design notes called "force hide": a manual override that beats even a pin-word match, for the "no, I want *this* thread hidden regardless" case -- now on a timer instead of being permanent-until-manually-undone.

There is no "undo last hide" action (`U`). Reversing a permanent word is done via the Filters editor (remove the word); reversing a temporary flag is done via the Temporary panel or by re-toggling it from the same right-click menu.

Poster-ID and filename-pattern filtering, and any cross-thread poster-following notion, remain out of scope (§11.3) -- these were dropped from §18.5's wish-list during the 0.5.0 planning revision rather than remaining on it as deferred work.

## 11.6 Filter rule representation

There is no rule engine, condition list, operator set, or `[[filters]]` TOML structure. Pin/hide words are stored per board directly in `config.toml`, alongside the flat top-level global lists:

```toml
global_pin_words = []
global_hide_words = ["example hide"]

[[board_filters]]
board = "gif"
pin_words = ["example pin one", "example pin two"]
hide_words = ["example hide one", "example hide two", "example hide three"]
```

Words or phrases are matched as whole-word/whole-phrase, case-insensitive, negation-aware substrings (§11.2) -- there is no separate "contains" vs. "equals" operator, and no any/all condition-combining, since each list is just a flat set of alternatives (a match on *any* word in the list matches).

Every hidden thread explains why, computed live rather than logged to a database table (§5.4):

```text
Hidden by: example hide one
```

or, for a temporary hide:

```text
Hidden by: temporarily hidden · expires in 6d
```

## 12. Watched-state system

**Deliberately not built, by explicit decision during 0.4.0 planning.** There is no per-media state machine (unseen / started / watched / saved / hidden / failed), no 80%-played threshold, no explicit "mark watched," and no minimum-viewing-period logic. When 0.4.0 reached this milestone, the actual need turned out to be much narrower than the full system above: a way to tell, from the catalogue, whether a previously-opened thread has gained new media worth looking at again -- not per-item tracking of what's been watched.

What was built instead: each thread remembers how much media it had the last time it was actually opened (not just seen in the catalogue). The catalogue compares that against the thread's current media count and shows a badge when it's grown:

```text
#123456789 · 302 replies · 69 files · 1 videos · 1h ago  +8 new
```

This is thread-level and count-only -- it says "8 new items have appeared," not which items, and carries no information about which media the user has actually looked at.

Opening a previously visited thread restores its last media position (the last item the user had selected). This is not optional or configurable (§9) -- it always happens. There is no "jump to first unseen" alternative, since there is no unseen state to jump to.

## 13. Saving and downloads

The user must be able to save the currently selected file immediately.

Default save action:

```text
S
```

Suggested directory structure:

```text
~/Downloads/1cheri/
├── gif/
│   └── 123456789-thread-title/
│       ├── 123456790_original-name.webm
│       └── 123456802_another-file.jpg
└── wsg/
```

The application:

- Preserves the original filename where possible.
- Prefixes filenames with the post number by default.
- Avoids overwriting existing files (appends `_1`, `_2`, ... on a collision, rather than skipping or warning).
- Displays a non-blocking save confirmation (a status-bar message, auto-clearing after a few seconds).
- Allows the save directory and filename pattern to be configured (Settings window).

Not implemented: "detect previously saved media" in the sense of recognising *this exact post was already saved* and surfacing that in the UI (e.g. a saved-state badge). Pressing Save again for an already-saved item just writes another numbered copy, per the overwrite-avoidance behaviour above.

If the item's file is already present in the local media cache (§14.3) -- because it was played or prefetched -- saving copies it directly instead of re-downloading.

**Bulk thread download**, added after 0.4.x use (§18.5): a "Save all media" action in the viewer's `⋮` overflow menu saves every item in the thread's thumbnail strip, not just the currently selected one. Manual and user-initiated per thread you're already viewing, per §4's non-goal against automatically mass-downloading every opened thread -- there is no "always download everything" setting. Reuses the same per-item save logic above (path/filename resolution, collision-avoidance, cache-hit-copies-instead-of-redownloading) for each item. Runs up to 2 downloads concurrently, each new dispatch staggered by 400ms, rather than one at a time or all-at-once -- media downloads are otherwise unthrottled by design (§14) for the normal one-or-two-at-a-time case (current item + prefetch), but a thread can have dozens of items, and 4chan doesn't document a rate limit for `i.4cdn.org` media the way it does for the `a.4cdn.org` API, so this errs conservative rather than assuming a burst of connections is fine. Progress is reported the same way as single-item save (a non-blocking, auto-clearing status-bar message), counting up as items complete (`Saving media: N/M...`) and finishing with a summary (`Saved all N files.`, or `Saved N/M files (K failed).` if any item failed).

## 14. Caching and network behaviour

All network requests are centralised through one module (`net.rs`), as planned, but it is simpler than originally specified: every request runs on a spawned thread and results are delivered back to the GTK main loop through a channel, but there is **no cancellation, no conditional (`If-Modified-Since`) requests, and no retry/backoff**. This is a known, documented simplification rather than an oversight -- the request volume this application generates doesn't currently justify the added complexity, but it's worth revisiting if that changes.

Actually implemented, matching the original budget:

- No more than one request per second, enforced globally, to `a.4cdn.org` (catalogue and thread JSON) -- **not** to media/thumbnail downloads from `i.4cdn.org`, which are deliberately left unthrottled (throttling them too caused thread-opens to visibly stall behind queued thumbnail downloads).
- No more than one catalogue/thread refresh per ten seconds per board/thread -- enforced as a plain time-based debounce, not via conditional requests (since those aren't implemented).
- A distinct `User-Agent` identifying the application and version.
- Local caching of thumbnails and media (§14.2, §14.3) so the same file is not re-fetched once cached.

Confirm current limits against the live API documentation before release, since these terms can change independently of this document.

Use three cache levels:

### 14.1 Catalogue cache

Store:

- Thread summaries.
- OP thumbnails.
- Last-modified information.
- Board refresh times.

### 14.2 Thumbnail cache

Store downloaded or generated thumbnails for media items.

### 14.3 Playback cache

Implemented in 0.4.1, close to the original strategy with one simplification:

- Current media plays from the local cache if already present, otherwise streams directly from the network as before caching existed.
- The *next* media item (one item, not the whole thread) begins prefetching in the background as soon as the current one is selected.
- Previously played or prefetched media is retained -- but **indefinitely, not "temporarily."** There is no eviction policy: nothing ever deletes a cached file once written. In practice this means the cache directory (`~/.cache/1cheri/media/`) only grows, the way many application caches do, until the user clears it manually.
- Other media remains thumbnail-only until selected, as planned.

Downloads to the cache are written to a temporary file first and only renamed into place once complete, so a reader (mpv, primarily) never sees a partial file mid-download -- an early version of this cache didn't do this and could hand mpv a truncated file if the user navigated quickly, which looked like playback silently hanging.

An entire thread is never downloaded automatically, as planned.

**Cache size limits are not configurable, and no `cache_size_mb` setting exists.** A real byte-budget eviction policy remains a reasonable future addition, not built.

### 14.4 Error and edge-case handling

- **Board/catalogue fetch fails** (offline, timeout, server error): matches the plan. The cached catalogue is shown immediately regardless, a fetch is attempted in the background, and a clear non-blocking status message reports failure ("Fetch failed (...); showing cached threads if available.") rather than a blank screen.
- **Thumbnail fails to load**: matches the plan. Falls back to a placeholder icon rather than a blank slot.
- **A thread fails to open** (404, network error): partially matches the plan as of 0.4.5. There is still no fallback to cached content and no "mark the thread as gone" state, but the failure is no longer silent -- the catalogue's status bar now shows "Couldn't open thread #N: ..." rather than leaving the UI looking like nothing happened. A thread that *later* 404s while already open (as opposed to failing to open in the first place) still isn't specifically detected.
- **Individual media fails to download or play** (corrupt file, unsupported codec, dead link): partially matches the plan as of 0.4.5. There is still no per-item "failed" state (§12 has no per-item states at all) and no automatic skip, but a genuine mpv load/playback error is no longer silent -- it now shows "Playback error: ..." in the viewer's status bar (auto-clearing after a few seconds), sourced from mpv's own end-of-file-with-error event. A closely related bug (a prefetched file could be partially downloaded and handed to mpv, which then just sat there with no error at all, since nothing had actually gone wrong from mpv's point of view yet) was separately fixed in 0.4.3 by making cache downloads atomic.

## 15. Actual configuration

The real `config.toml` is considerably smaller than originally sketched -- there is no `version` field in the file itself (the binary's own `--version` output is authoritative instead), and every setting below not present has no equivalent: no `auto_advance`, `show_comments`/`comment_mode` (comments are always shown, in one mode, §7), `media_filter` (§8), `thumbnail_position`/`thumbnail_size` (§6.2), `unwatched_first` (§6.1/§12), or `cache_size_mb`/`prefetch_next` (prefetch always happens, §14.3).

`resume_last_thread` is a real setting, added after 0.4.x use showed the original unconditional behaviour (auto-reopening whatever thread was last open, on every startup) was surprising if you'd deliberately backed out to the catalogue before quitting -- it's off by default (including for configs written before this setting existed). The active *board* is still always restored regardless of this setting; only auto-reopening a thread on startup is conditional. Returning to the catalogue (`B`/`Escape`/the viewer's Back button) clears the remembered thread, so even with the setting on, only quitting while still inside a thread resumes it next launch. Resuming a thread's last-viewed *media item* once you're actually in it (§12) is a separate, still-unconditional mechanism, untouched by this setting.

```toml
muted = true
volume = 70
loop_video = true
autoplay = true
resume_last_thread = false
save_directory = "~/Downloads/1cheri"
filename_template = "{post}_{original_name}"
boards = ["gif", "wsg", "g"]
active_board = "gif"
sort_threads_by = "most_videos"
global_pin_words = []
global_hide_words = ["example hide"]

[[board_filters]]
board = "gif"
pin_words = ["example pin one", "example pin two"]
hide_words = ["example hide one", "example hide two", "example hide three"]
```

`board_filters` (§11) is the one addition with no equivalent in the original sketch; `global_pin_words`/`global_hide_words` (§11.6, added after 0.4.x use) apply across every board rather than one, unioned into each board's own list at match time. Session state that changes on every navigation (active board, active thread) lives in SQLite (§5.4), not here, so it isn't rewritten to disk on every click.

## 16. Actual Rust module structure

The real structure stayed much flatter than the original nested sketch -- each area got one file (or one small `ui/` submodule) rather than a directory, since the separation those directories implied wasn't earning its cost at this project's size. This matches the closing guidance in the original sketch ("modules should only be split when the separation is useful") more literally than the sketch's own example did:

```text
src/
├── main.rs
├── comments.rs      -- comment HTML parsing + direct-reply lookup (§7)
├── config.rs         -- Config struct, load/save, pin/hide word helpers (§5.5, §11.6)
├── filters.rs         -- negation-aware word/phrase matching (§11.2)
├── media_cache.rs      -- local media cache + prefetch (§14.3)
├── models.rs           -- API/catalogue/thread/media types (§17)
├── net.rs               -- HTTP requests, rate limiting, downloads (§14)
├── player.rs             -- libmpv/GTK4 GLArea integration (§5.3, §19.1)
├── storage.rs             -- SQLite access (§5.4)
├── thumbnails.rs           -- thumbnail disk cache + async loading (§14.2)
└── ui/
    ├── mod.rs               -- Shell (shared state), window chrome, settings hook-in
    ├── catalogue.rs           -- board tabs, thread list, filters UI, temp-flags panel (§6.1, §11)
    ├── viewer.rs               -- media viewer, keybindings, comments panel (§6.2, §7, §10)
    └── settings.rs              -- settings window
```

There is no `app.rs`, `error.rs` (errors are mostly `String`/`eprintln!`-based rather than a typed error hierarchy -- a simplification, not a deliberate rejection of §21's "use typed error handling" guidance), `api/` directory (folded into `net.rs` + `models.rs`), `downloads/` directory (folded into `net.rs` + the viewer's save logic), or `shortcuts.rs` (bindings are one match statement in `ui/viewer.rs`, per §10).

## 17. Actual core models

There is no standalone `Board` struct (a board is just a `String` id in `Config.boards`) and no `Post`/`Media` split -- attachments are flattened out of the raw API post shape on demand rather than modelled as a always-present `media: Option<Media>` field. `unseen_media`, `watched_state`, `poster_id`, `duration_seconds`, `has_audio`, `saved`, and per-item `hidden` do not exist anywhere in the model layer, consistent with §8's and §12's notes above.

```rust
// One post exactly as the 4chan API returns it -- field names match the API
// so a captured thread JSON can be used directly.
pub struct ApiPost {
    pub no: u64,
    pub resto: u64,
    pub sub: Option<String>,
    pub com: Option<String>,
    pub filename: Option<String>,
    pub ext: Option<String>,
    pub tim: Option<u64>,
    pub w: Option<u32>,
    pub h: Option<u32>,
    pub fsize: Option<u64>,
}

pub struct ApiThread {
    pub posts: Vec<ApiPost>,
}

pub enum MediaType { Video, AnimatedImage, StaticImage, Unsupported }

// An ApiPost that carries an attachment, flattened for the UI. This is the
// closest equivalent to the original sketch's `Media`.
pub struct MediaPost {
    pub post_number: u64,
    pub original_filename: String,
    pub extension: String,
    pub media_type: MediaType,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: Option<u64>,
    pub source_path: String,      // resolved playable URL or local path
    pub thumbnail_url: Option<String>,
}

pub struct Thread {
    pub board: String,
    pub op: ApiPost,
    pub posts: Vec<ApiPost>,
}

// One catalogue row (§6.1). Closest equivalent to the original sketch's
// `ThreadSummary`, with `video_count`/`temp_flag`/`last_seen_media_count`
// added to support §6.1/§11/§12 and no `unseen_media`.
pub struct ThreadSummary {
    pub board: String,
    pub number: u64,
    pub subject: Option<String>,
    pub comment_preview: String,
    pub thumbnail_url: Option<String>,
    pub replies: u32,
    pub images: u32,
    pub video_count: Option<u32>,       // None until the thread has been opened
    pub last_modified: i64,
    pub temp_flag: Option<TempFlag>,    // §11.5
    pub last_seen_media_count: Option<u32>, // §12
}
```

## 18. Development milestones

## 18.1 Version 0.1.0. Media proof

Goal: prove that the chosen stack can deliver the core viewing experience.

### 18.1.1 Step zero: render spike

Before anything else, confirm libmpv can render reliably inside a GTK4 widget on the target system (§19.1). This should be a throwaway program, not part of the real application:

- Bare GTK4 window with a single video area.
- Load and play one local WebM file via libmpv.
- Resize the window during playback.
- Toggle mute.
- Close the player cleanly.

If this is not smooth and reliable, resolve the rendering approach (see the GStreamer fallback note in §19.1) before writing any catalogue, filtering, or application UI code. Do not proceed to §18.1.2 until the spike works.

### 18.1.2 Remaining milestone work

Required:

- GTK4 application window.
- Embedded libmpv video area.
- One hard-coded or locally supplied thread JSON file.
- Extract image and video posts.
- Scrollable thumbnail strip.
- Select a media item.
- Play WebM video.
- Display images.
- Next and previous media controls.
- Keyboard controls.
- Persistent mute.
- Save current media.
- Display current post and direct referenced replies.

This milestone must be completed before substantial catalogue or settings work.

## 18.2 Version 0.2.0. Board browser

Required:

- Configurable board list.
- Board catalogue fetching.
- Thread cards or rows.
- Thread sorting.
- Thread refresh.
- Thread opening.
- Basic cache.
- Resume positions (per-thread last-viewed media item, §12 -- not the full watched-state system that name might suggest, which was never built).
- Restore last board and thread.

## 18.3 Version 0.3.0. Filtering

Delivered, via the simpler system described in §11 rather than the rule engine originally sketched:

- Thread subject and OP-comment text filters (pin words and hide words, negation-aware).
- Quick actions via right-click context menu, not the originally planned `X`/`Shift+X` keybindings.
- Filter editor (word lists, add/remove) for thread filters.
- Explain why hidden.

Not delivered, and not currently planned: "undo last hide" as a dedicated action (reversal is done via the Filters editor or the Temporary panel instead, §11.5), and "skip hidden media during navigation" (there is no media-level hide to skip, §11.3). Filename, poster-ID, duration, dimension, and file-size filters were dropped from §18.5's wish-list entirely during the 0.5.0 planning revision, rather than remaining on it as deferred work.

## 18.4 Version 0.4.0. Viewer refinement

Delivered:

- Prefetch next media (§14.3).
- Improved caching, including the atomic-download fix for a partial-file race under rapid navigation.
- Fullscreen.
- Loop controls (as a persistent setting, not a live in-viewer toggle).
- Resume playback (thread + last media item, unconditional -- §12).
- Thumbnail-strip scroll positioning refinements (centred, then adjusted to sit above centre after real use showed centre still felt too low).
- **Temporary, auto-expiring per-thread pin/hide** (§11.5) and the **catalogue's "+N new" badge** (§12) -- both added mid-milestone, replacing two originally-planned items below after use showed they covered the actual need better.

Explicitly not delivered, by decision rather than oversight:

- **Playback-speed control.** Judged unnecessary for this application's actual use.
- **Seen and unseen state refinement**, in the sense of the full per-item watched-state system in §12. Replaced by the much narrower "+N new" thread-level count above.

"Improved reply previews" and "better download handling" were not tracked as distinct items; the comment/reply system (§7) and save/cache behaviour (§13, §14.3) received the improvements described in their own sections above as part of this milestone's general polish, rather than as a separate planned feature.

## 18.5 Version 0.5.0. Daily-use release

Possible requirements, as revised by the user after the 0.4.x milestones
shipped -- narrower than the original list, dropping several items nobody
had revisited (rule-based media filters, filename-pattern and poster-ID
filters, cross-thread poster filtering, stable database migrations,
thread following, perceptual/near-duplicate hashing, and packaged Arch/AUR
build instructions):

- Keyboard remapping.
- Filter import and export.
- ~~Bulk downloads (whole thread)~~ -- delivered, see §13.
- Duplicate detection (exact-file-match only; near-duplicate/perceptual
  matching was considered and judged not worth the added complexity).
- ~~Better diagnostics~~ -- delivered: process-wide stdout/stderr redirect to
  `~/.local/state/1cheri/1cheri.log` (see `logging.rs`), truncated fresh each
  launch, with an "Open log file" button in Settings. This is a
  file-descriptor-level redirect, not just wrapping the app's own
  `eprintln!` calls -- it's the only thing that actually catches mpv/
  ffmpeg's hardware-decoder-probing messages, which write straight to the
  real stderr fd and bypass mpv's own logging/terminal settings entirely.
- ~~Create a README.md for the project~~ -- delivered, see `README.md`.

## 19. Initial technical risks

The coding agent should investigate these risks early:

### 19.1 libmpv embedding

Confirm that libmpv can render reliably inside a GTK4 widget on the target system.

Test:

- Window resizing.
- Hardware decoding.
- Multiple sequential WebM files.
- Fullscreen transitions.
- Audio mute.
- Player shutdown.
- Rapid media switching.

If reliable embedding cannot be achieved with libmpv's render API on this system, evaluate GStreamer (`gtk4paintablesink`) as a fallback before committing further work to the viewer UI. This decision should be made during the render spike (§18.1.1), not discovered later.

### 19.2 GTK4 and player event integration

Playback events must be safely passed into GTK's main event loop.

### 19.3 Thumbnail performance

Large threads may contain many media posts. Thumbnail loading must remain incremental and avoid blocking the UI.

### 19.4 Thread parsing

Comment references must be parsed correctly, including multiple references in one post.

### 19.5 Filtering cost

Basic filters should run on catalogue and thread metadata immediately. Expensive media inspection such as duration or audio detection should be lazy and cached.

## 20. Acceptance criteria for the first useful release

Met, as originally written:

1. Launch it on Manjaro XFCE.
2. Select `/gif/` from a configured board list.
3. See the board's current threads.
4. Sort threads by video count.
5. Hide unwanted threads using text filters.
6. Open a thread.
7. Browse video thumbnails.
8. Play WebM files inside the application window.
9. Move through videos using the keyboard.
10. Keep audio muted between runs.
11. See the current post and replies referencing it.
12. Save the current video.
13. Resume at the previous point in the thread.

**Not met, because individual-media filtering was never built (§11.3):**

- "Hide unwanted individual media" -- only whole threads can be hidden or pinned, not a single attachment within an open thread.
- "Skip hidden items automatically" -- there is nothing hidden at the media level for Next/Previous to skip.

Given the actual `/gif/` workflow, thread-level filtering covered the real need well enough that these two were never revisited; they remain open if a concrete need for them shows up later.

## 21. Guidance for the coding agent

- Build the media proof before the full application.
- Keep the project runnable at every milestone.
- Add or update version numbers with every meaningful edit.
- Prefer small, testable modules.
- Avoid speculative abstractions.
- Use typed error handling.
- Keep network, player, database, and GTK state separated.
- Do not block the GTK main thread with network, file, probe, or database work.
- Add structured logging early.
- Include a sample configuration file.
- Include database migrations in source control.
- Provide clear build and dependency instructions for Manjaro and Arch Linux.
- Treat thread-level filtering as a central product feature; media-level and reply-level filtering were never built and are no longer on the §18.5 wish-list either -- not just "minimal for now."
- Treat comments as supporting context for the selected media, not a feature to build out further.
- Keep video playback as the primary measure of success.

## 22. Project name

`Chan Media Viewer` was a working name only, used throughout early development. The chosen final name is `1cheri`, and the executable, window title, `--version`/`--help` output, User-Agent string, and config/data/cache paths (§5.4, §5.5) all use it consistently.

```text
1cheri
```

The Cargo package itself is internally named `cheri` rather than `1cheri` -- Cargo package names cannot start with a digit, but this restriction doesn't apply to a `[[bin]]` target name, so the compiled executable is still literally `1cheri`. This is purely a build-tooling detail; it's invisible anywhere the application is actually run or referenced.
