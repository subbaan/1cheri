use crate::comments::{parse_comment, unescape_entities};
use serde::Deserialize;

/// One post as returned by 4chan's thread API (`a.4cdn.org/{board}/thread/{no}.json`).
/// Field names match the real API exactly so a locally captured thread JSON
/// can be dropped in unmodified.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiPost {
    pub no: u64,
    #[serde(default)]
    pub resto: u64,
    #[serde(default)]
    pub sub: Option<String>,
    #[serde(default)]
    pub com: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub ext: Option<String>,
    #[serde(default)]
    pub tim: Option<u64>,
    #[serde(default)]
    pub w: Option<u32>,
    #[serde(default)]
    pub h: Option<u32>,
    #[serde(default)]
    pub fsize: Option<u64>,
    #[serde(default)]
    pub replies: Option<u32>,
    #[serde(default)]
    pub images: Option<u32>,
    /// Not part of the real API: lets the bundled offline fixture point at a
    /// local file instead of `i.4cdn.org`. Absent on real API responses, in
    /// which case the media URL is derived from `tim`/`ext` as usual.
    #[serde(default)]
    pub local_media: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiThread {
    pub posts: Vec<ApiPost>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Video,
    AnimatedImage,
    StaticImage,
    Unsupported,
}

impl MediaType {
    pub fn from_ext(ext: &str) -> Self {
        match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "webm" | "mp4" | "m4v" => MediaType::Video,
            "gif" => MediaType::AnimatedImage,
            "jpg" | "jpeg" | "png" => MediaType::StaticImage,
            _ => MediaType::Unsupported,
        }
    }
}

/// A post that carries an attachment, flattened out of `ApiPost` for the UI.
#[derive(Debug, Clone)]
pub struct MediaPost {
    pub post_number: u64,
    pub original_filename: String,
    pub extension: String,
    pub media_type: MediaType,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: Option<u64>,
    /// Resolved location to hand to the player: a local filesystem path for
    /// this milestone's offline fixture.
    pub source_path: String,
    /// `None` for the offline fixture (no real thumbnail exists to fetch).
    pub thumbnail_url: Option<String>,
}

#[derive(Clone)]
pub struct Thread {
    pub board: String,
    pub op: ApiPost,
    pub posts: Vec<ApiPost>,
}

impl Thread {
    pub fn from_api(board: &str, api: ApiThread) -> Self {
        let op = api.posts.first().cloned().expect("thread has no posts");
        Thread {
            board: board.to_string(),
            op,
            posts: api.posts,
        }
    }

    pub fn subject(&self) -> String {
        self.op
            .sub
            .as_deref()
            .map(unescape_entities)
            .unwrap_or_else(|| format!("Thread #{}", self.op.no))
    }

    /// Every post that carries an attachment, in thread order.
    pub fn media_posts(&self) -> Vec<MediaPost> {
        self.posts
            .iter()
            .filter_map(|p| {
                let ext = p.ext.clone()?;
                let tim = p.tim?;
                let is_local = p.local_media.is_some();
                let source_path = p
                    .local_media
                    .clone()
                    .unwrap_or_else(|| format!("https://i.4cdn.org/{}/{}{}", self.board, tim, ext));
                let thumbnail_url =
                    (!is_local).then(|| format!("https://i.4cdn.org/{}/{}s.jpg", self.board, tim));
                Some(MediaPost {
                    post_number: p.no,
                    original_filename: p.filename.clone().unwrap_or_else(|| tim.to_string()),
                    extension: ext.clone(),
                    media_type: MediaType::from_ext(&ext),
                    width: p.w,
                    height: p.h,
                    size_bytes: p.fsize,
                    source_path,
                    thumbnail_url,
                })
            })
            .collect()
    }

    pub fn post_by_number(&self, no: u64) -> Option<&ApiPost> {
        self.posts.iter().find(|p| p.no == no)
    }

    /// How many of this thread's attachments are actually videos. Only
    /// knowable once the full thread has been fetched (catalog.json only
    /// gives a total attachment count, not a type breakdown) -- see
    /// `CatalogThread` doc comment.
    pub fn video_count(&self) -> u32 {
        self.media_posts()
            .iter()
            .filter(|m| m.media_type == MediaType::Video)
            .count() as u32
    }

    /// Media attachment count matching catalog.json's `images` field
    /// semantics: replies only, excluding the OP's own attachment. Needed so
    /// `last_seen_media_count` (derived from a full thread fetch) can be
    /// compared directly against `ThreadSummary.images` (derived from the
    /// catalog) for the "+N new" badge -- `media_posts()` alone includes the
    /// OP and would make every thread look permanently 1 short.
    pub fn reply_media_count(&self) -> u32 {
        self.media_posts()
            .iter()
            .filter(|m| m.post_number != self.op.no)
            .count() as u32
    }
}

/// One page of `a.4cdn.org/{board}/catalog.json`.
#[derive(Debug, Deserialize)]
pub struct CatalogPage {
    #[allow(dead_code)]
    pub page: u32,
    pub threads: Vec<CatalogThread>,
}

/// One thread entry from catalog.json (the OP post plus rollup counts).
///
/// Note: `images` is the *total* attachment count for the whole thread, not
/// an images-only count -- 4chan's catalog API doesn't break attachments
/// down by media type, so an accurate video count isn't available until the
/// full thread is fetched (project.md §19.5 flags this class of cost).
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogThread {
    pub no: u64,
    #[serde(default)]
    pub sub: Option<String>,
    #[serde(default)]
    pub com: Option<String>,
    #[serde(default)]
    pub replies: u32,
    #[serde(default)]
    pub images: u32,
    pub last_modified: i64,
    #[serde(default)]
    pub tim: Option<u64>,
    #[serde(default)]
    pub ext: Option<String>,
}

/// Flattened catalogue row for the UI and the cache, per project.md §17.
#[derive(Debug, Clone)]
pub struct ThreadSummary {
    pub board: String,
    pub number: u64,
    pub subject: Option<String>,
    pub comment_preview: String,
    pub thumbnail_url: Option<String>,
    pub replies: u32,
    pub images: u32,
    /// `None` until the thread has actually been opened once.
    pub video_count: Option<u32>,
    pub last_modified: i64,
    /// Temporary, auto-expiring per-thread pin/hide set via right-click,
    /// independent of (and, for `Hide`, overriding) pin/hide word matches.
    /// Only ever `Some` after a DB reload -- like `video_count`, a
    /// freshly-fetched catalogue entry doesn't know about it.
    pub temp_flag: Option<TempFlag>,
    /// Total media (images+videos) count as of the last time this thread was
    /// actually opened, not just seen in the catalogue -- compared against
    /// `images` to show a "+N new" badge. `None` until first opened.
    pub last_seen_media_count: Option<u32>,
}

impl ThreadSummary {
    pub fn from_catalog(board: &str, t: &CatalogThread) -> Self {
        let comment_preview = t
            .com
            .as_deref()
            .map(|c| {
                let text: String = parse_comment(c)
                    .into_iter()
                    .map(|line| line.text)
                    .collect::<Vec<_>>()
                    .join(" ");
                text.chars().take(160).collect::<String>()
            })
            .unwrap_or_default();
        let thumbnail_url = t
            .tim
            .map(|tim| format!("https://i.4cdn.org/{board}/{tim}s.jpg"));
        ThreadSummary {
            board: board.to_string(),
            number: t.no,
            subject: t.sub.as_deref().map(unescape_entities),
            comment_preview,
            thumbnail_url,
            replies: t.replies,
            images: t.images,
            video_count: None,
            last_modified: t.last_modified,
            temp_flag: None,
            last_seen_media_count: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempFlagKind {
    Pin,
    Hide,
}

#[derive(Debug, Clone)]
pub struct TempFlag {
    pub kind: TempFlagKind,
    pub flagged_at: i64,
}
