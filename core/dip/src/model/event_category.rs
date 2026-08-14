//! Which visibility setting governs an event the user wrote.

use crate::model::{KIND_MUTE, KIND_PROFILE};

/// Which visibility setting governs an event the user wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventCategory {
    /// The user's profile.
    Profile,
    /// The user's trust, block and mute lists.
    Metadata,
    /// Everything else the user writes.
    Content,
}

impl EventCategory {
    /// Every category, so a caller can ask about each in turn.
    pub const ALL: [Self; 3] = [Self::Profile, Self::Metadata, Self::Content];

    /// The kinds a category other than [`Content`](Self::Content) names.
    const NAMED: [(Self, u16); 2] = [(Self::Profile, KIND_PROFILE), (Self::Metadata, KIND_MUTE)];

    /// Which category governs an event of this kind.
    #[must_use]
    pub fn of(kind: u16) -> Self {
        Self::NAMED
            .iter()
            .find(|(_, named)| *named == kind)
            .map_or(Self::Content, |(category, _)| *category)
    }

    /// Every kind some category other than [`Content`](Self::Content) names.
    #[must_use]
    pub fn named_kinds() -> Vec<u16> {
        Self::NAMED.iter().map(|(_, kind)| *kind).collect()
    }

    /// The kinds this category names.
    #[must_use]
    pub fn kinds(self) -> Vec<u16> {
        Self::NAMED
            .iter()
            .filter(|(category, _)| *category == self)
            .map(|(_, kind)| *kind)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_category_is_the_kinds_it_names() {
        assert_eq!(EventCategory::of(KIND_PROFILE), EventCategory::Profile);
        assert_eq!(EventCategory::of(KIND_MUTE), EventCategory::Metadata);
        assert_eq!(EventCategory::of(1), EventCategory::Content);

        assert_eq!(EventCategory::Profile.kinds(), [KIND_PROFILE]);
        assert_eq!(EventCategory::Metadata.kinds(), [KIND_MUTE]);
        assert!(EventCategory::Content.kinds().is_empty());
        assert_eq!(EventCategory::named_kinds(), [KIND_PROFILE, KIND_MUTE]);
    }
}
