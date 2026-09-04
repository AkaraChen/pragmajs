//! Source offsets → 1-based Unicode-scalar line/column.

/// Converts a UTF-8 byte offset to a 1-based line and Unicode-scalar column.
pub fn byte_offset_to_line_col(source: &str, offset: u32) -> (u32, u32) {
    let mut line = 1u32;
    let mut col = 1u32;
    for (i, ch) in source.char_indices() {
        if i as u32 >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Converts a UTF-16 code-unit offset to a 1-based line and Unicode-scalar column.
pub fn utf16_offset_to_line_col(source: &str, offset: u32) -> (u32, u32) {
    let mut consumed = 0u32;
    let mut line = 1u32;
    let mut col = 1u32;
    for ch in source.chars() {
        if consumed >= offset {
            break;
        }
        consumed = consumed.saturating_add(ch.len_utf16() as u32);
        if ch == '\n' {
            line = line.saturating_add(1);
            col = 1;
        } else {
            col = col.saturating_add(1);
        }
    }
    (line, col)
}
