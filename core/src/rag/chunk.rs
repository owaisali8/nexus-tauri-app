//! Splitting documents into retrievable pieces.
//!
//! Chunks overlap so a passage spanning a boundary is still findable, and
//! splits prefer paragraph then sentence breaks over cutting mid-word.

/// Target chunk size in characters.
///
/// Characters rather than tokens: counting tokens needs the model's
/// tokenizer, which differs per provider. ~4 chars/token puts this near 250
/// tokens, comfortably inside every embedding model's window.
pub const TARGET_CHARS: usize = 1000;

/// How much consecutive chunks share.
///
/// Without overlap, a sentence split across a boundary matches neither side
/// well enough to retrieve.
pub const OVERLAP_CHARS: usize = 150;

/// A piece of a document, ready to embed.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub text: String,
    /// Position in the source document, for ordering and citation.
    pub index: usize,
    /// Character offset in the original text.
    pub offset: usize,
}

/// Split `text` into overlapping chunks.
///
/// Returns an empty vec for blank input rather than one empty chunk, which
/// would otherwise be embedded and pollute results.
pub fn split(text: &str) -> Vec<Chunk> {
    split_with(text, TARGET_CHARS, OVERLAP_CHARS)
}

pub fn split_with(text: &str, target: usize, overlap: usize) -> Vec<Chunk> {
    let trimmed = text.trim();
    if trimmed.is_empty() || target == 0 {
        return Vec::new();
    }

    // Work in chars, not bytes: slicing a &str mid-codepoint panics, and
    // documents are frequently not ASCII.
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= target {
        return vec![Chunk {
            text: trimmed.to_string(),
            index: 0,
            offset: 0,
        }];
    }

    // Overlap must leave forward progress, or this loops forever.
    let overlap = overlap.min(target.saturating_sub(1));

    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < chars.len() {
        let hard_end = (start + target).min(chars.len());
        let end = if hard_end == chars.len() {
            hard_end
        } else {
            find_break(&chars, start, hard_end)
        };

        let text: String = chars[start..end].iter().collect();
        let text = text.trim().to_string();

        if !text.is_empty() {
            chunks.push(Chunk {
                text,
                index: chunks.len(),
                offset: start,
            });
        }

        if end >= chars.len() {
            break;
        }
        start = end.saturating_sub(overlap).max(start + 1);
    }

    chunks
}

/// Find a natural break at or before `hard_end`.
///
/// Prefers a paragraph break, then a sentence end, then whitespace. Falls back
/// to the hard limit when the text has none — a minified file or a language
/// without spaces still has to be split somewhere.
fn find_break(chars: &[char], start: usize, hard_end: usize) -> usize {
    // Only look in the last third; breaking too early wastes the chunk.
    let earliest = start + (hard_end - start) * 2 / 3;

    let window = &chars[earliest..hard_end];

    // Paragraph break.
    if let Some(position) = window
        .windows(2)
        .rposition(|pair| pair[0] == '\n' && pair[1] == '\n')
    {
        return earliest + position + 2;
    }

    // Sentence end followed by whitespace.
    if let Some(position) = window.windows(2).rposition(|pair| {
        matches!(pair[0], '.' | '!' | '?' | '。' | '！' | '？') && pair[1].is_whitespace()
    }) {
        return earliest + position + 2;
    }

    // Any whitespace.
    if let Some(position) = window.iter().rposition(|c| c.is_whitespace()) {
        return earliest + position + 1;
    }

    hard_end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_input_yields_nothing() {
        assert!(split("").is_empty());
        assert!(split("   \n\t ").is_empty());
    }

    #[test]
    fn short_text_is_a_single_chunk() {
        let chunks = split("a short document");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "a short document");
        assert_eq!(chunks[0].index, 0);
    }

    #[test]
    fn long_text_is_split_and_indexed() {
        let text = "word ".repeat(1000);
        let chunks = split(&text);

        assert!(chunks.len() > 1);
        for (position, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index, position, "indices must be sequential");
            assert!(!chunk.text.is_empty());
        }
    }

    #[test]
    fn chunks_overlap_so_boundaries_stay_findable() {
        let text: String = (0..400).map(|n| format!("sentence {n}. ")).collect();
        let chunks = split(&text);

        assert!(chunks.len() > 2);
        // Each chunk must start before the previous one ended.
        for pair in chunks.windows(2) {
            assert!(
                pair[1].offset < pair[0].offset + pair[0].text.chars().count(),
                "chunk {} does not overlap its predecessor",
                pair[1].index
            );
        }
    }

    #[test]
    fn splitting_prefers_a_paragraph_break() {
        let first = "a".repeat(700);
        let second = "b".repeat(700);
        let text = format!("{first}\n\n{second}");

        let chunks = split(&text);
        assert!(chunks.len() >= 2);
        // The break should land at the blank line, so no chunk mixes them.
        assert!(
            chunks[0].text.chars().all(|c| c == 'a'),
            "the first chunk should not cross the paragraph break"
        );
    }

    /// Text with no whitespace at all — minified JSON, or CJK — must still
    /// split rather than loop or return one enormous chunk.
    #[test]
    fn text_without_breaks_still_splits() {
        let text = "x".repeat(5000);
        let chunks = split(&text);

        assert!(chunks.len() > 1);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.text.chars().count() <= TARGET_CHARS)
        );
    }

    /// Slicing on byte offsets would panic here.
    #[test]
    fn multibyte_text_does_not_panic() {
        let text = "日本語のテキストです。".repeat(300);
        let chunks = split(&text);

        assert!(chunks.len() > 1);
        let rejoined: String = chunks.iter().map(|chunk| chunk.text.as_str()).collect();
        assert!(rejoined.contains("日本語"));
    }

    /// A pathological config must not spin.
    #[test]
    fn overlap_larger_than_target_terminates() {
        let chunks = split_with(&"word ".repeat(500), 100, 500);
        assert!(chunks.len() > 1);
    }

    #[test]
    fn every_chunk_is_within_the_target_size() {
        let text = "lorem ipsum dolor sit amet ".repeat(400);
        for chunk in split(&text) {
            assert!(
                chunk.text.chars().count() <= TARGET_CHARS,
                "chunk {} is {} chars",
                chunk.index,
                chunk.text.chars().count()
            );
        }
    }
}
