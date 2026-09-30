//! The single-device key backup: one text file holding prose instructions and
//! the identity key, written by the core.
//!
//! Multi-device is the happy path — see `docs/keys.md#login-with-device` — and
//! this is what a user with one phone gets instead. The whole point of writing
//! it here is that the key never crosses the bridge: the core encodes,
//! optionally encrypts, and writes the file, and the shell presents the share
//! sheet over a path the view never sees. `docs/keys.md#backup`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use coracle_lib::keys::SecretKey;

/// The shortest password the backup will encrypt under.
///
/// There is no recovery for a forgotten one, so a short password is a lost key
/// rather than a weak one.
pub const MINIMUM_PASSWORD_LENGTH: usize = 12;

/// What the file is called, which is what the share sheet offers.
const FILE_NAME: &str = "Nostr Secret Key.txt";

/// The scrypt work factor NIP-49 encryption runs at, within the 16–22 the spec
/// recommends. The low end: this runs on a phone, and the user is waiting.
const LOG_N: u8 = 16;

/// NIP-49's "the key has been handled securely" byte, which is the claim this
/// app can make — the key lives in platform secure storage and is read out only
/// to sign. `docs/keys.md#key-custody`.
const SECURITY_BYTE: u8 = 0x01;

/// Write the backup file into `cache`, encrypted under `password` when there is
/// one, and answer where it landed.
///
/// The path is for the shell's share sheet. It is not for the view:
/// `Capacitor.convertFileSrc` would let the view fetch the file back, which is
/// the same as handing it the key.
pub fn write(identity: &SecretKey, cache: &Path, password: Option<&str>) -> Result<PathBuf> {
    let contents = match password {
        Some(password) => {
            if password.chars().count() < MINIMUM_PASSWORD_LENGTH {
                bail!("a backup password is at least {MINIMUM_PASSWORD_LENGTH} characters");
            }

            let ncryptsec = identity
                .to_ncryptsec(password, LOG_N, SECURITY_BYTE)
                .map_err(|error| anyhow::anyhow!("encrypting the backup failed: {error}"))?;

            instructions(&ncryptsec, true)
        }
        None => instructions(&identity.to_nsec(), false),
    };

    fs::create_dir_all(cache)
        .with_context(|| format!("creating the cache directory {}", cache.display()))?;

    let path = cache.join(FILE_NAME);

    fs::write(&path, contents).with_context(|| format!("writing the backup {}", path.display()))?;

    Ok(path)
}

/// Remove a backup this device wrote, whether the share sheet was used or
/// dismissed.
///
/// A plaintext `nsec` sitting in the cache directory outlives the flow that
/// needed it, and the cache is the one place the platform may hand to a
/// file picker.
pub fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result.with_context(|| format!("removing the backup {}", path.display())),
    }
}

/// The prose the file wraps the key in, following Flotilla's
/// `KeyDownload.svelte` with the signer paragraph rewritten.
fn instructions(key: &str, encrypted: bool) -> String {
    let opening = if encrypted {
        "This file contains a backup of your Nostr secret key, encrypted using the password you chose."
    } else {
        "This file contains a backup of your Nostr secret key."
    };

    let label = if encrypted {
        "Your encrypted private key is:"
    } else {
        "Your private key is:"
    };

    format!(
        "{opening}

Most online services keep track of users by giving them a username and password. This gives the \
service total control over their users, allowing them to ban them at any time, or sell their \
activity.

On Nostr, you control your own identity and social data, using cryptography. The \
basic idea is that you have a public key, which acts as your user ID, and a private key which \
allows you to prove your identity.

It's very important to keep your private key secret because it grants permanent and complete \
access to your account.

{label}

{key}

Keep this file somewhere safe, like a password manager. It restores this app if you lose this \
device. It is also your identity everywhere else on Nostr, so other apps accept it if you want to \
post from one.
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::TempDir;

    #[test]
    fn a_plain_backup_carries_the_nsec_and_the_instructions() {
        let cache = TempDir::new("backup");
        let identity = SecretKey::generate();
        let path = write(&identity, &cache.0, None).unwrap();
        let contents = fs::read_to_string(&path).unwrap();

        assert!(contents.contains(&identity.to_nsec()));
        assert!(contents.contains("private key"));
        assert_eq!(path.file_name().unwrap(), FILE_NAME);
    }

    #[test]
    fn an_encrypted_backup_carries_no_nsec_and_decrypts_to_the_same_key() {
        let cache = TempDir::new("backup");
        let identity = SecretKey::generate();
        let password = "correct horse battery staple";
        let path = write(&identity, &cache.0, Some(password)).unwrap();
        let contents = fs::read_to_string(&path).unwrap();

        assert!(!contents.contains(&identity.to_nsec()));

        let ncryptsec = contents
            .split_whitespace()
            .find(|word| word.starts_with("ncryptsec1"))
            .unwrap();

        assert_eq!(
            SecretKey::from_ncryptsec(ncryptsec, password)
                .unwrap()
                .to_hex(),
            identity.to_hex()
        );
    }

    #[test]
    fn a_short_password_is_refused_before_anything_is_written() {
        let cache = TempDir::new("backup");

        assert!(write(&SecretKey::generate(), &cache.0, Some("hunter2")).is_err());
        assert!(!cache.0.join(FILE_NAME).exists());
    }

    #[test]
    fn removing_a_backup_twice_is_not_an_error() {
        let cache = TempDir::new("backup");
        let path = write(&SecretKey::generate(), &cache.0, None).unwrap();

        remove(&path).unwrap();
        remove(&path).unwrap();

        assert!(!path.exists());
    }
}
