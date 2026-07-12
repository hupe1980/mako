//! Text chunking utilities for RAG indexing.

/// Split `text` into overlapping chunks of approximately `chunk_size` characters.
///
/// Respects paragraph boundaries (double newlines) where possible.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.len() <= chunk_size {
        return vec![text.trim().to_owned()];
    }

    let paragraphs: Vec<&str> = text.split("\n\n").collect();
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
