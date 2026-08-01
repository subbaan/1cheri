use crate::models::{ApiPost, Thread};

/// One display line out of a sanitised post comment.
#[derive(Debug, Clone)]
pub struct CommentLine {
    pub text: String,
    pub greentext: bool,
    pub references: Vec<u64>,
}

/// Converts a raw 4chan `com` field (HTML) into a safe internal
/// representation: line breaks, greentext, and quote-link references are
/// recognised; everything else is reduced to plain text. Per project.md §7,
/// this does not need to be a full HTML renderer.
pub fn parse_comment(raw: &str) -> Vec<CommentLine> {
    let normalized = raw.replace("<br>", "\n").replace("<br/>", "\n").replace("<br />", "\n");
    let stripped = strip_tags(&normalized);
    let unescaped = unescape_entities(&stripped);
    unescaped
        .lines()
        .map(|line| {
            let references = extract_references(line);
            let trimmed = line.trim_start();
            let greentext = trimmed.starts_with('>') && !trimmed.starts_with(">>");
            CommentLine {
                text: line.to_string(),
                greentext,
                references,
            }
        })
        .collect()
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Decodes the small set of HTML entities 4chan actually emits (subjects and
/// comments alike -- e.g. a subject like "Trials &amp; Tribulations" needs
/// this same treatment, not just comment bodies).
pub fn unescape_entities(s: &str) -> String {
    s.replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&amp;", "&")
}

/// Finds every `>>NNNN` reference in a line, in order. Handles multiple
/// references in one line (project.md §19.4).
fn extract_references(line: &str) -> Vec<u64> {
    let mut refs = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'>' && bytes[i + 1] == b'>' {
            let start = i + 2;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start {
                if let Ok(n) = line[start..j].parse::<u64>() {
                    refs.push(n);
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    refs
}

/// Finds byte ranges of bare http(s) URLs in already-detagged comment text,
/// so the UI layer can render them as clickable links.
pub fn find_urls(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &text[i..];
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let start = i;
            let mut j = i;
            while j < bytes.len() && !bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            while j > start
                && matches!(
                    bytes[j - 1],
                    b'.' | b',' | b'!' | b'?' | b';' | b':' | b'\'' | b'"' | b')' | b']' | b'>'
                )
            {
                j -= 1;
            }
            if j > start {
                spans.push((start, j));
                i = j;
                continue;
            }
        }
        i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
    }
    spans
}

/// All posts in thread order whose comment directly references `target_no`,
/// per project.md §7 (the default comment mode).
pub fn direct_replies<'a>(thread: &'a Thread, target_no: u64) -> Vec<&'a ApiPost> {
    thread
        .posts
        .iter()
        .filter(|p| {
            p.com.as_deref().is_some_and(|c| {
                parse_comment(c)
                    .iter()
                    .any(|line| line.references.contains(&target_no))
            })
        })
        .collect()
}
