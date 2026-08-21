//! The authorship registers of `docs/proofs.md`, and selecting on them.

use std::collections::BTreeSet;

use coracle_lib::keys::PublicKey;

/// Which authorship register an event sits in, from one device's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Register {
    /// Authored by this device.
    Own,
    /// Authored by someone else.
    Forwardable,
    /// Neither.
    Held,
}

/// A register constraint: which registers qualify, and the identities they are
/// measured from.
///
/// The identities are part of the constraint because both registers are
/// relative to them — "authored by us" and "signed to us" mean nothing until
/// "us" is named. A device acting as several is measured against any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registers {
    /// The identities asking. Never empty.
    pub identities: BTreeSet<PublicKey>,
    /// The registers admitted. An empty set admits nothing.
    pub registers: BTreeSet<Register>,
}

impl Registers {
    /// A constraint admitting exactly `registers`.
    #[must_use]
    pub fn new<'a>(
        identities: impl IntoIterator<Item = &'a PublicKey>,
        registers: impl IntoIterator<Item = Register>,
    ) -> Self {
        Self {
            identities: identities.into_iter().copied().collect(),
            registers: registers.into_iter().collect(),
        }
    }

    /// Everything this device is in a position to put on the wire at all: its
    /// own events, and those it holds the author's signature over.
    #[must_use]
    pub fn offerable<'a>(identities: impl IntoIterator<Item = &'a PublicKey>) -> Self {
        Self::new(identities, [Register::Own, Register::Forwardable])
    }

    /// Whether the constraint admits nothing.
    #[must_use]
    pub fn matches_nothing(&self) -> bool {
        self.registers.is_empty() || self.identities.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::author;

    #[test]
    fn the_offerable_registers_are_the_two_that_travel() {
        let registers = Registers::offerable(&[author(1)]);

        assert!(registers.registers.contains(&Register::Own));
        assert!(registers.registers.contains(&Register::Forwardable));
        assert!(!registers.registers.contains(&Register::Held));
        assert!(!registers.matches_nothing());
    }
}
