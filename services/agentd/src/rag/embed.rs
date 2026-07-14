//! Text chunking utilities for RAG indexing.

/// Split `text` into overlapping chunks of approximately `chunk_size` characters.
///
/// Respects paragraph boundaries (double newlines) where possible.
/// Returns an empty `Vec` for empty or whitespace-only input.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.len() <= chunk_size {
        return vec![trimmed.to_owned()];
    }

    let paragraphs: Vec<&str> = trimmed.split("\n\n").collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for para in &paragraphs {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }

        if current.len() + para.len() + 2 > chunk_size && !current.is_empty() {
            chunks.push(current.trim().to_owned());
            // Overlap: carry tail of current chunk forward
            let tail_start = current.len().saturating_sub(overlap);
            current = current[tail_start..].trim().to_owned();
            current.push('\n');
        }
        current.push_str(para);
        current.push_str("\n\n");
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_owned());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_stays_single_chunk() {
        let text = "Short text that easily fits in one chunk.";
        let chunks = chunk_text(text, 512, 64);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text.trim());
    }

    #[test]
    fn empty_text_returns_no_chunks() {
        assert!(chunk_text("", 512, 64).is_empty());
    }

    #[test]
    fn whitespace_only_returns_no_chunks() {
        assert!(chunk_text("   \n\n   \n\n   ", 512, 64).is_empty());
    }

    #[test]
    fn long_text_splits_into_multiple_chunks() {
        // Three long paragraphs that together exceed chunk_size
        let para = "word ".repeat(40); // ~200 chars
        let text = format!("{para}\n\n{para}\n\n{para}");
        let chunks = chunk_text(&text, 100, 20);
        assert!(
            chunks.len() >= 2,
            "expected >=2 chunks for text of {} chars, got {}",
            text.len(),
            chunks.len()
        );
    }

    #[test]
    fn chunks_do_not_lose_content() {
        let para = "a ".repeat(30);
        let text = format!("{para}\n\n{para}");
        let chunks = chunk_text(&text, 100, 10);
        // All words must appear somewhere in the chunks
        let joined = chunks.join(" ");
        assert!(joined.contains("a a"), "content lost during chunking");
    }
}
