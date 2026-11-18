/// Convert a character index into a byte index for safe string slicing.
pub fn char_to_byte_index(text: &str, char_index: usize) -> Option<usize> {
    if char_index == 0 {
        return Some(0);
    }
    text.char_indices()
        .nth(char_index)
        .map(|(byte_index, _)| byte_index)
        .or_else(|| {
            if text.chars().count() == char_index {
                Some(text.len())
            } else {
                None
            }
        })
}

/// Return the suffix starting at `char_index`.
pub fn suffix_from_char(text: &str, char_index: usize) -> Option<&str> {
    char_to_byte_index(text, char_index).map(|byte_index| &text[byte_index..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_to_byte_index_respects_multibyte_boundaries() {
        let text = "ab┃cd";

        assert_eq!(char_to_byte_index(text, 0), Some(0));
        assert_eq!(char_to_byte_index(text, 2), Some(2));
        assert_eq!(char_to_byte_index(text, 3), Some(5));
        assert_eq!(char_to_byte_index(text, 5), Some(7));
        assert_eq!(char_to_byte_index(text, 6), None);
    }
}
