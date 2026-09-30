// Thread-level word/phrase filtering and pinning, per project.md §11 (plus
// pinning, which isn't in the doc but uses the same matching mechanism).
//
// Deliberately simpler than project.md §11.6's sketched generic rule engine
// (conditions, operators, any/all matching): what's needed is per-board
// lists of words/phrases for two actions (hide, pin), matched with
// whole-word (not substring) search, negation-aware.
//
// Negation awareness exists because plain matching has a real false-positive
// problem: hiding on "BBC" would also hide a thread whose OP explicitly says
// "no BBC" -- the opposite of what the word was meant to catch. This is a
// heuristic, not real language understanding (sarcasm and convoluted
// phrasing will still defeat it), but it handles the dominant real-world
// case well. That case was checked empirically against live `/gif/` OP text
// (2026-07-29): "no" is by far the most common exclusion marker, almost
// always as a comma-separated list -- e.g. "no BBW, BBC, rimming" -- which
// is why the lookback window below is wide enough to cover a short list
// after "no", not just a single adjacent word.
//
// Only OP subject/comment text is ever matched against: `ui/catalogue.rs`
// filters `ThreadSummary`, whose fields come from catalog.json, which is
// OP-only per thread (reply bodies aren't fetched until a thread is opened,
// which happens after filtering, not before) -- exactly where thread rules
// like exclusion lists actually get stated.
//
// Words match singular/plural-insensitively (see `words_match`), so one
// filter entry covers both "trap" and "traps" instead of needing two.

const NEGATION_MARKERS: &[&str] = &[
    "no", "not", "non", "never", "without", "anti", "isnt", "arent", "dont", "doesnt", "wont", "cant", "cannot",
];

/// How many words before a match to scan for a negation marker. Wide enough
/// to cover a short comma-separated exclusion list after "no" (tokenizing
/// drops the commas, so "no BBW, BBC, rimming" becomes contiguous words
/// ["no", "bbw", "bbc", "rimming"]), while staying a local, bounded check
/// rather than scanning the whole post.
const NEGATION_LOOKBACK: usize = 8;

fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// True if `b` is a regular English plural of `a`: a+"s" ("trap" -> "traps"),
/// a+"es" ("box" -> "boxes"), or y -> "ies" ("titty" -> "titties"). The base
/// must be at least two characters so that one-letter words don't pair up
/// with common short words ("i"/"is", "a"/"as", "u"/"us").
fn is_plural_of(a: &str, b: &str) -> bool {
    if a.chars().count() < 2 {
        return false;
    }
    if let Some(stem) = b.strip_prefix(a) {
        return stem == "s" || stem == "es";
    }
    match (a.strip_suffix('y'), b.strip_suffix("ies")) {
        (Some(a_stem), Some(b_stem)) => !a_stem.is_empty() && a_stem == b_stem,
        _ => false,
    }
}

/// Whole-word equality, tolerant of singular vs plural in either direction.
/// Symmetric because the filter entry may be either form.
fn words_match(a: &str, b: &str) -> bool {
    a == b || is_plural_of(a, b) || is_plural_of(b, a)
}

/// True if `phrase_words` appears as a contiguous run in `haystack_words` at
/// least once without a negation marker in the preceding lookback window.
fn has_unnegated_match(haystack_words: &[String], phrase_words: &[String]) -> bool {
    let n = phrase_words.len();
    if n == 0 || haystack_words.len() < n {
        return false;
    }
    for start in 0..=(haystack_words.len() - n) {
        let window = &haystack_words[start..start + n];
        if window.iter().zip(phrase_words).all(|(h, p)| words_match(h, p)) {
            let lookback_start = start.saturating_sub(NEGATION_LOOKBACK);
            let preceding = &haystack_words[lookback_start..start];
            if !preceding.iter().any(|w| NEGATION_MARKERS.contains(&w.as_str())) {
                return true;
            }
        }
    }
    false
}

/// Returns the phrases (from `phrases`) that appear, as whole words, in
/// `subject` or `comment_preview`, without a preceding negation marker. A
/// phrase may be multiple words.
pub fn find_matches<'a>(subject: Option<&str>, comment_preview: &str, phrases: &'a [String]) -> Vec<&'a str> {
    let haystack_words = tokenize(&format!("{} {}", subject.unwrap_or(""), comment_preview));
    phrases
        .iter()
        .filter(|phrase| {
            let phrase_words = tokenize(phrase);
            !phrase_words.is_empty() && has_unnegated_match(&haystack_words, &phrase_words)
        })
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phrases(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn plain_match() {
        let words = phrases(&["BBC"]);
        let matches = find_matches(Some("Some thread"), "I love BBC content", &words);
        assert_eq!(matches, vec!["BBC"]);
    }

    #[test]
    fn negated_single_word_does_not_match() {
        let words = phrases(&["BBC"]);
        let matches = find_matches(Some("Some thread"), "no BBC allowed here", &words);
        assert!(matches.is_empty());
    }

    #[test]
    fn negated_comma_list_does_not_match() {
        // The exact real-world pattern found on live /gif/ OP text.
        let words = phrases(&["BBC"]);
        let matches = find_matches(Some("Discussion thread"), "no BBW, BBC, rimming", &words);
        assert!(matches.is_empty());
    }

    #[test]
    fn one_negated_one_plain_occurrence_still_matches() {
        let words = phrases(&["BBC"]);
        let matches = find_matches(
            Some("thread"),
            "no BBC in the first half, but the second half is all BBC",
            &words,
        );
        assert_eq!(matches, vec!["BBC"]);
    }

    #[test]
    fn whole_word_boundary_not_substring() {
        let words = phrases(&["BBC"]);
        let matches = find_matches(Some("thread"), "check out bbcable.com for streams", &words);
        assert!(matches.is_empty());
    }

    #[test]
    fn multi_word_phrase_matches_as_contiguous_words() {
        let words = phrases(&["Local Models"]);
        let matches = find_matches(Some("Local Models General"), "", &words);
        assert_eq!(matches, vec!["Local Models"]);
    }

    #[test]
    fn singular_filter_matches_plural_text() {
        let words = phrases(&["trap", "box", "titty", "glass"]);
        let matches = find_matches(Some("traps and boxes"), "titties behind glasses", &words);
        assert_eq!(matches, vec!["trap", "box", "titty", "glass"]);
    }

    #[test]
    fn plural_filter_matches_singular_text() {
        let words = phrases(&["traps", "movies"]);
        let matches = find_matches(Some("a trap"), "one movie", &words);
        assert_eq!(matches, vec!["traps", "movies"]);
    }

    #[test]
    fn plural_matching_applies_per_word_in_phrases() {
        let words = phrases(&["local model"]);
        let matches = find_matches(Some("Local Models General"), "", &words);
        assert_eq!(matches, vec!["local model"]);
    }

    #[test]
    fn plural_matching_is_not_substring_matching() {
        let words = phrases(&["cat", "i"]);
        let matches = find_matches(Some("catalog"), "this is it, cats aside", &words);
        assert_eq!(matches, vec!["cat"]);
        let matches = find_matches(Some("catalog"), "this is it", &words);
        assert!(matches.is_empty());
    }

    #[test]
    fn negated_plural_does_not_match() {
        let words = phrases(&["trap"]);
        let matches = find_matches(Some("thread"), "no traps", &words);
        assert!(matches.is_empty());
    }

    // The three cases below are three real threads pulled live from /gif/
    // during verification of this feature (2026-07-29), covering exactly
    // the ambiguity a plain "BBC" hide word needs to get right.
    #[test]
    fn real_gif_thread_plainly_about_word_is_hidden() {
        let words = phrases(&["BBC"]);
        let matches = find_matches(
            Some("Proud BBC Cuckolds"),
            "Off-topic / Seethe = Your mom dies in her sleep tonight.",
            &words,
        );
        assert_eq!(matches, vec!["BBC"]);
    }

    #[test]
    fn real_gif_thread_excluding_word_is_not_hidden() {
        let words = phrases(&["BBC"]);
        let matches = find_matches(
            Some("Girls With Glasses Thread"),
            "the girls have to be cute, all races acceptable if they aren't fat or ugly. no BBW, BBC, rimming, or tranny bullshit, if it's a weird fetish, no one wants to se",
            &words,
        );
        assert!(matches.is_empty());
    }

    #[test]
    fn real_gif_thread_mentioning_word_in_body_is_hidden() {
        let words = phrases(&["BBC"]);
        let matches = find_matches(Some("BMwm"), "White little dicklets learning their place serving BBC", &words);
        assert_eq!(matches, vec!["BBC"]);
    }
}
