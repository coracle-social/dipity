//! The store, opened once and held by both halves of the app.
//!
//! `docs/overview.md#architecture` has the view addressing the store directly
//! and gossip going through a [`Node`](crate::node::Node), and both are the same
//! SQLite file: one `Db`, two readers. So the shell opens this first and hands
//! it to the node, rather than the node owning a store the view cannot see.
//!
//! Nothing to read here yet — the query surface is #45.

use std::sync::Arc;

use dip::db::Db;

/// Why the store could not be opened or read.
#[derive(Debug, uniffi::Error)]
pub enum StoreError {
    /// SQLite refused, or the migrations did not apply.
    Failed {
        /// What went wrong, for the log.
        reason: String,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed { reason } => write!(f, "the store failed: {reason}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<anyhow::Error> for StoreError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed {
            reason: format!("{error:#}"),
        }
    }
}

/// One open store.
#[derive(uniffi::Object)]
pub struct Store {
    /// The connection, its migrations, and the channels its writes announce on.
    pub(crate) db: Arc<Db>,
}

#[uniffi::export]
impl Store {
    /// Open the store at `path`, migrating it to the current schema.
    ///
    /// The shell picks the path; the core has no opinion about where an app's
    /// data lives on either platform. `docs/storage.md`.
    #[uniffi::constructor]
    pub fn open(path: String) -> Result<Arc<Self>, StoreError> {
        Ok(Arc::new(Self {
            db: Arc::new(Db::open(path)?),
        }))
    }
}
