use pragma_loc::offset_to_line_col;

#[test]
fn two_line_snippet() {
    assert_eq!(offset_to_line_col("ab\ncd", 0), (1, 1));
    assert_eq!(offset_to_line_col("ab\ncd", 3), (2, 1));
}

#[test]
fn utf8_multibyte_then_newline() {
    let src = "é\nx";
    let nl = src.find('\n').unwrap() as u32;
    assert_eq!(offset_to_line_col(src, nl), (1, 2));
    assert_eq!(offset_to_line_col(src, nl + 1), (2, 1));
}
