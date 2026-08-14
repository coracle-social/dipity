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

/// A register constraint: which registers qualify, and the device they are
/// measured from.
///
/// The identity is part of the constraint because both registers are relative
/// to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registers {
    /// The device asking.
    pub identity: PublicKey,
    /// The registers admitted. An empty set admits nothing.
    pub registers: BTreeSet<Register>,
}

impl Registers {
    /// A constraint admitting exactly `registers`.
    #[must_use]
    pub fn new(identity: PublicKey, registers: impl IntoIterator<Item = Register>) -> Self {
        Self {
            identity,
            registers: registers.into_iter().collect(),
        }
    }

    /// Everything this device is in a position to put on the wire at all: its
    /// own events, and those it holds the author's signature over.
    #[must_use]
    pub fn offerable(identity: PublicKey) -> Self {
        Self::new(identity, [Register::Own, Register::Forwardable])
    }

    /// Whether this constraint admits `register`.
    #[must_use]
    pub fn admits(&self, register: Register) -> bool {
        self.registers.contains(&register)
    }

    /// Whether the constraint admits nothing.
    #[must_use]
    pub fn matches_nothing(&self) -> bool {
        self.registers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::author;

    #[test]
    fn the_offerable_registers_are_the_two_that_travel() {
        let registers = Registers::offerable(author(1));

        assert!(registers.admits(Register::Own));
        assert!(registers.admits(Register::Forwardable));
        assert!(!registers.admits(Register::Held));
        assert!(!registers.matches_nothing());
    }
}
