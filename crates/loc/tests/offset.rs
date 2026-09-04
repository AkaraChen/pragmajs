use pragma_loc::{byte_offset_to_line_col, utf16_offset_to_line_col};

#[test]
fn two_line_snippet() {
    assert_eq!(byte_offset_to_line_col("ab\ncd", 0), (1, 1));
    assert_eq!(byte_offset_to_line_col("ab\ncd", 3), (2, 1));
}

#[test]
fn utf8_multibyte_then_newline() {
    let src = "é\nx";
    let nl = src.find('\n').unwrap() as u32;
    assert_eq!(byte_offset_to_line_col(src, nl), (1, 2));
    assert_eq!(byte_offset_to_line_col(src, nl + 1), (2, 1));
}

#[test]
fn byte_and_utf16_offsets_report_unicode_scalar_columns() {
    let src = "a😀b\nc";
    let b_byte = src.find('b').unwrap() as u32;
    let b_utf16 = src[..src.find('b').unwrap()].encode_utf16().count() as u32;
    assert_eq!(byte_offset_to_line_col(src, b_byte), (1, 3));
    assert_eq!(utf16_offset_to_line_col(src, b_utf16), (1, 3));

    let c_byte = src.find('c').unwrap() as u32;
    let c_utf16 = src[..src.find('c').unwrap()].encode_utf16().count() as u32;
    assert_eq!(byte_offset_to_line_col(src, c_byte), (2, 1));
    assert_eq!(utf16_offset_to_line_col(src, c_utf16), (2, 1));
}
