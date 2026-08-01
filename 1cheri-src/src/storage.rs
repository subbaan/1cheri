// SQLite-backed cache and small key-value state store, per project.md §5.4.
//
// Deliberate scope trade-off: these calls run on the GTK main thread rather
// than a background thread. project.md §21 says not to block the main
// thread with database work; this is a local SQLite file with at most a
// few hundred rows, and every call here is sub-millisecond, so the
// simplicity is judged worth it for this milestone. Revisit if the schema
// or row counts grow enough for that to stop being true.

use crate::models::ThreadSummary;
use rusqlite::{params, Connection};
use std::path::PathBuf;

fn db_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    PathBuf::from(home).join(".local/share/1cheri/state.sqlite3")
}

/// Adds `column` to `table` if it isn't already there. `CREATE TABLE IF NOT
/// EXISTS` only handles brand-new databases; existing ones (like anyone who
/// ran an earlier version of this app) need an explicit migration to pick up
/// columns added later, since SQLite errors on a duplicate `ADD COLUMN`.
fn ensure_column(conn: &Connection, table: &str, column: &str, ddl: &str) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == column);
    if !exists {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {ddl}"), [])?;
    }
    Ok(())
}

pub fn open() -> rusqlite::Result<Connection> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("failed to create data directory");
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS threads (
            board            TEXT NOT NULL,
            number           INTEGER NOT NULL,
            subject          TEXT,
            comment_preview  TEXT NOT NULL DEFAULT '',
            thumbnail_url    TEXT,
            replies          INTEGER NOT NULL DEFAULT 0,
            images           INTEGER NOT NULL DEFAULT 0,
            video_count      INTEGER,
            last_modified    INTEGER NOT NULL,
            last_opened_at   INTEGER,
            last_media_post  INTEGER,
            PRIMARY KEY (board, number)
        );

        CREATE TABLE IF NOT EXISTS app_state (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )?;
    // Manual per-thread hide override (project.md's own suggested schema
    // has this on `threads`; it was left out of the trimmed 0.2.0 schema
    // since word-based filtering didn't exist yet). Superseded by
    // temp_flag_kind/temp_flag_at below -- kept as unused dead columns
    // rather than a destructive drop, and migrated once (below).
    ensure_column(&conn, "threads", "hidden", "hidden INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(&conn, "threads", "hidden_reason", "hidden_reason TEXT")?;
    // How many media posts existed the last time this thread was actually
    // opened (not just seen in the catalogue) -- compared against the
    // catalogue's current `images` count to show a "+N new" badge. NULL
    // until the thread has been opened at least once.
    ensure_column(&conn, "threads", "last_seen_media_count", "last_seen_media_count INTEGER")?;
    // Temporary, auto-expiring per-thread pin/hide: the user right-clicks a
    // thread that doesn't match any permanent pin/hide word but still wants
    // flagged for a while, without adding a permanent rule. Deliberately
    // separate from config.toml's pin/hide *word* lists -- one flag per
    // thread, not a rule -- and `temp_flag_kind = 'hide'` takes precedence
    // over everything else, same as the old `hidden` override it replaces
    // (see ui/catalogue.rs's partitioning order). Cleared automatically by
    // `prune_expired_temp_flags` once `TEMP_FLAG_TTL_SECS` has passed.
    ensure_column(&conn, "threads", "temp_flag_kind", "temp_flag_kind TEXT")?;
    ensure_column(&conn, "threads", "temp_flag_at", "temp_flag_at INTEGER")?;

    // One-time migration: carry over any pre-existing `hidden = 1` threads
    // into the new system with a fresh TTL, so upgrading doesn't silently
    // lose (or instantly expire) what the user had force-hidden. Guarded by
    // an app_state marker since `hidden` is otherwise never cleared, which
    // would make this re-fire and stomp a later `clear_temp_flag` every
    // restart if it weren't gated.
    if get_app_state(&conn, "migrated_temp_flags")?.is_none() {
        conn.execute(
            "UPDATE threads SET temp_flag_kind = 'hide', temp_flag_at = ?1
             WHERE hidden = 1 AND temp_flag_kind IS NULL",
            params![now_unix()],
        )?;
        set_app_state(&conn, "migrated_temp_flags", "1")?;
    }
    Ok(conn)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Upserts catalogue rows without clobbering per-thread state a catalogue
/// refresh doesn't know about (video_count, last_opened_at, last_media_post).
pub fn upsert_thread_summaries(conn: &Connection, board: &str, summaries: &[ThreadSummary]) -> rusqlite::Result<()> {
    for s in summaries {
        conn.execute(
            "INSERT INTO threads (board, number, subject, comment_preview, thumbnail_url, replies, images, last_modified)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(board, number) DO UPDATE SET
                subject = excluded.subject,
                comment_preview = excluded.comment_preview,
                thumbnail_url = excluded.thumbnail_url,
                replies = excluded.replies,
                images = excluded.images,
                last_modified = excluded.last_modified",
            params![
                board,
                s.number as i64,
                s.subject,
                s.comment_preview,
                s.thumbnail_url,
                s.replies,
                s.images,
                s.last_modified,
            ],
        )?;
    }
    Ok(())
}

/// Removes cached rows for `board` whose thread number is no longer present
/// in a freshly fetched catalog. catalog.json is a complete snapshot of every
/// currently-live thread, so anything absent from it has 404'd or been
/// pruned by 4chan and is gone for good. Skips pruning if `live_numbers` is
/// empty -- an empty catalog for a board that has threads is more likely a
/// fetch/parse anomaly than every thread vanishing at once, and pruning on
/// that would wipe the whole cached list.
pub fn prune_vanished_threads(conn: &Connection, board: &str, live_numbers: &[u64]) -> rusqlite::Result<usize> {
    if live_numbers.is_empty() {
        return Ok(0);
    }
    let placeholders = live_numbers.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("DELETE FROM threads WHERE board = ? AND number NOT IN ({placeholders})");
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&board];
    let number_params: Vec<i64> = live_numbers.iter().map(|&n| n as i64).collect();
    for n in &number_params {
        params.push(n);
    }
    conn.execute(&sql, params.as_slice())
}

/// Removes a single thread from the cache -- used when opening it confirms
/// it's actually gone (a real 404), rather than waiting for the next
/// catalogue refresh's `prune_vanished_threads` pass to catch it.
pub fn delete_thread(conn: &Connection, board: &str, number: u64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM threads WHERE board = ?1 AND number = ?2", params![board, number as i64])?;
    Ok(())
}

pub fn load_cached_threads(conn: &Connection, board: &str) -> rusqlite::Result<Vec<ThreadSummary>> {
    let mut stmt = conn.prepare(
        "SELECT number, subject, comment_preview, thumbnail_url, replies, images, video_count, last_modified, temp_flag_kind, temp_flag_at, last_seen_media_count
         FROM threads WHERE board = ?1 ORDER BY last_modified DESC",
    )?;
    let rows = stmt.query_map(params![board], |row| {
        let kind: Option<String> = row.get(8)?;
        let at: Option<i64> = row.get(9)?;
        let temp_flag = match (kind.as_deref(), at) {
            (Some("pin"), Some(at)) => Some(crate::models::TempFlag { kind: crate::models::TempFlagKind::Pin, flagged_at: at }),
            (Some("hide"), Some(at)) => Some(crate::models::TempFlag { kind: crate::models::TempFlagKind::Hide, flagged_at: at }),
            _ => None,
        };
        Ok(ThreadSummary {
            board: board.to_string(),
            number: row.get::<_, i64>(0)? as u64,
            subject: row.get(1)?,
            comment_preview: row.get(2)?,
            thumbnail_url: row.get(3)?,
            replies: row.get(4)?,
            images: row.get(5)?,
            video_count: row.get::<_, Option<i64>>(6)?.map(|v| v as u32),
            last_modified: row.get(7)?,
            temp_flag,
            last_seen_media_count: row.get::<_, Option<i64>>(10)?.map(|v| v as u32),
        })
    })?;
    rows.collect()
}

/// A week's generous margin over even a slow board's real thread lifetime --
/// long enough that a temporary flag never expires while still "current" to
/// the user, short enough that stale flags don't linger forever.
pub const TEMP_FLAG_TTL_SECS: i64 = 7 * 24 * 3600;

/// One row per currently temp-flagged thread, for the "Temporary" list panel.
pub struct TempFlagEntry {
    pub number: u64,
    pub subject: String,
    pub kind: String,
    pub flagged_at: i64,
}

/// Temporarily pins or hides a specific thread, independent of (and, for
/// `"hide"`, overriding) pin/hide word matches -- see ui/catalogue.rs's
/// partitioning order. `kind` is `"pin"` or `"hide"`; setting one replaces
/// whichever was previously set, since a thread can only carry one flag.
pub fn set_temp_flag(conn: &Connection, board: &str, number: u64, kind: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE threads SET temp_flag_kind = ?1, temp_flag_at = ?2 WHERE board = ?3 AND number = ?4",
        params![kind, now_unix(), board, number as i64],
    )?;
    Ok(())
}

pub fn clear_temp_flag(conn: &Connection, board: &str, number: u64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE threads SET temp_flag_kind = NULL, temp_flag_at = NULL WHERE board = ?1 AND number = ?2",
        params![board, number as i64],
    )?;
    Ok(())
}

/// Clears any temp flag older than `TEMP_FLAG_TTL_SECS`. Returns how many
/// rows were cleared. Called once at startup and hourly thereafter.
pub fn prune_expired_temp_flags(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE threads SET temp_flag_kind = NULL, temp_flag_at = NULL
         WHERE temp_flag_at IS NOT NULL AND temp_flag_at < ?1",
        params![now_unix() - TEMP_FLAG_TTL_SECS],
    )
}

pub fn load_temp_flags(conn: &Connection, board: &str) -> rusqlite::Result<Vec<TempFlagEntry>> {
    let mut stmt = conn.prepare(
        "SELECT number, subject, temp_flag_kind, temp_flag_at FROM threads
         WHERE board = ?1 AND temp_flag_kind IS NOT NULL ORDER BY temp_flag_at DESC",
    )?;
    let rows = stmt.query_map(params![board], |row| {
        let number: i64 = row.get(0)?;
        let subject: Option<String> = row.get(1)?;
        Ok(TempFlagEntry {
            number: number as u64,
            subject: subject.unwrap_or_else(|| format!("Thread #{number}")),
            kind: row.get(2)?,
            flagged_at: row.get(3)?,
        })
    })?;
    rows.collect()
}

pub fn update_video_count(conn: &Connection, board: &str, number: u64, count: u32) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE threads SET video_count = ?1 WHERE board = ?2 AND number = ?3",
        params![count, board, number as i64],
    )?;
    Ok(())
}

pub fn update_last_seen_media_count(conn: &Connection, board: &str, number: u64, count: u32) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE threads SET last_seen_media_count = ?1 WHERE board = ?2 AND number = ?3",
        params![count, board, number as i64],
    )?;
    Ok(())
}

pub fn set_last_opened(conn: &Connection, board: &str, number: u64, timestamp: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE threads SET last_opened_at = ?1 WHERE board = ?2 AND number = ?3",
        params![timestamp, board, number as i64],
    )?;
    Ok(())
}

pub fn set_last_media_post(conn: &Connection, board: &str, number: u64, post: u64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE threads SET last_media_post = ?1 WHERE board = ?2 AND number = ?3",
        params![post as i64, board, number as i64],
    )?;
    Ok(())
}

pub fn get_last_media_post(conn: &Connection, board: &str, number: u64) -> rusqlite::Result<Option<u64>> {
    let result: rusqlite::Result<Option<i64>> = conn.query_row(
        "SELECT last_media_post FROM threads WHERE board = ?1 AND number = ?2",
        params![board, number as i64],
        |row| row.get(0),
    );
    match result {
        Ok(v) => Ok(v.map(|v| v as u64)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn get_app_state(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    let result: rusqlite::Result<String> =
        conn.query_row("SELECT value FROM app_state WHERE key = ?1", params![key], |row| row.get(0));
    match result {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn set_app_state(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_state (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}
