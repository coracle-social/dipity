//! The two UUIDs both platforms have to agree on.
//!
//! `docs/transport.md#link-layer` specifies one primary service and one
//! characteristic, and `docs/discovery.md` makes the service UUID the entire
//! advertisement — a device is found by scanning for this and nothing else.
//!
//! They live here rather than in Swift and Kotlin for the reason the framing
//! does: an iPhone and an Android phone have to name the same service, and two
//! hard-coded copies is one place for them to disagree. The shell reads them
//! through the FFI and hands them to CoreBluetooth or `android.bluetooth`.
//!
//! Neither is derived from anything and neither is a registered assignment.
//! They are random 128-bit UUIDs, chosen once, and changing either is a
//! protocol break: a device on the old one is invisible to a device on the new.

/// The primary service, and the whole advertisement.
pub const SERVICE_UUID: &str = "80E0688B-A648-4D1D-BB0D-BC58993CF926";

/// The one characteristic, supporting `notify`, `write` and
/// `writeWithoutResponse`.
pub const CHARACTERISTIC_UUID: &str = "5C020905-A8FC-4D19-AB51-1FFDB97FB90A";

#[cfg(test)]
mod tests {
    use super::{CHARACTERISTIC_UUID, SERVICE_UUID};

    #[test]
    fn both_uuids_are_the_form_both_platforms_parse() {
        for uuid in [SERVICE_UUID, CHARACTERISTIC_UUID] {
            assert_eq!(uuid.len(), 36);
            assert_eq!(
                uuid.split('-').map(str::len).collect::<Vec<_>>(),
                [8, 4, 4, 4, 12]
            );
            assert!(
                uuid.chars()
                    .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
            );
        }

        assert_ne!(SERVICE_UUID, CHARACTERISTIC_UUID);
    }
}
