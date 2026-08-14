//! What the store makes of an event's tags.

/// Whether a tag is one a NIP-01 filter can name, which is the set worth
/// indexing. Everything else is read back from the event's tags.
#[must_use]
pub fn is_indexed_tag(name: &str) -> bool {
    name.len() == 1 && name.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_single_letter_tags_are_indexed() {
        assert!(is_indexed_tag("e"));
        assert!(is_indexed_tag("A"));
        assert!(!is_indexed_tag("imeta"));
        assert!(!is_indexed_tag("-"));
        assert!(!is_indexed_tag(""));
    }
}
