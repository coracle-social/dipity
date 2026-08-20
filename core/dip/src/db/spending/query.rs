//! Reads over `spending`.

use anyhow::{Context, Result};
use coracle_lib::keys::PublicKey;
use rusqlite::params;

use crate::db::Tx;

/// The accepted events and their bytes from `pubkey` at or after `cutoff`.
/// The rolling quota asks this of the last 24 hours.
pub fn since(tx: &Tx<'_>, pubkey: &PublicKey, cutoff: i64) -> Result<(u32, u64)> {
    let (events, bytes) = tx
        .prepare_cached(
            "SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM spending
             WHERE pubkey = ?1 AND at >= ?2",
        )?
        .query_row(params![pubkey.to_hex(), cutoff], |row| {
            let bytes: i64 = row.get(1)?;

            Ok((row.get::<_, u32>(0)?, bytes as u64))
        })
        .with_context(|| format!("summing spending from {pubkey}"))?;

    Ok((events, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::Db;
    use crate::db::spending::command;
    use crate::fixtures::author;

    #[test]
    fn spending_sums_events_and_bytes_since_a_cutoff() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        command::record(&tx, &author(1), 100, 40).unwrap();
        command::record(&tx, &author(1), 200, 60).unwrap();
        command::record(&tx, &author(2), 150, 1).unwrap();

        assert_eq!(since(&tx, &author(1), 0).unwrap(), (2, 100));
        assert_eq!(since(&tx, &author(1), 150).unwrap(), (1, 60));
        assert_eq!(since(&tx, &author(2), 0).unwrap(), (1, 1));
    }
}
