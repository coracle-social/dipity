//! Where the core's `log` records go on a device.
//!
//! `core/Cargo.toml` takes `log` as the facade only, so every `log::error!` in
//! the core is inert until something installs a logger. Nothing in the core
//! picks one — os_log and logcat are platform APIs, and choosing between them
//! is the shell's job — so the facade is bridged here instead: the shell
//! implements [`Logger`] over whichever it has and calls [`init_logging`] once
//! at startup.
//!
//! Records are pushed, never pulled. A shell that has not called
//! [`init_logging`] gets nothing, which is the behaviour on the host too.

use std::sync::Arc;

use log::{Level, LevelFilter, Log, Metadata, Record};

/// How severe a record is.
///
/// The `log` crate's own levels, redeclared because a foreign enum cannot be
/// generated for a type this crate does not own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LogLevel {
    /// Something failed and the subsystem could not carry on.
    Error,
    /// Something failed and the subsystem carried on regardless.
    Warn,
    /// A milestone worth seeing in a shipped build.
    Info,
    /// Detail for reproducing a fault.
    Debug,
    /// Everything.
    Trace,
}

impl From<Level> for LogLevel {
    fn from(level: Level) -> Self {
        match level {
            Level::Error => Self::Error,
            Level::Warn => Self::Warn,
            Level::Info => Self::Info,
            Level::Debug => Self::Debug,
            Level::Trace => Self::Trace,
        }
    }
}

impl From<LogLevel> for LevelFilter {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Error => Self::Error,
            LogLevel::Warn => Self::Warn,
            LogLevel::Info => Self::Info,
            LogLevel::Debug => Self::Debug,
            LogLevel::Trace => Self::Trace,
        }
    }
}

/// The platform log, as the core reaches it.
///
/// Implemented in Swift over `os_log` and in Kotlin over `android.util.Log`.
/// `target` is the Rust module path the record came from, which is what makes
/// a subsystem filterable once it is a category or a tag.
#[uniffi::export(with_foreign)]
pub trait Logger: Send + Sync {
    /// Write one record. Called on whichever thread logged it.
    fn log(&self, level: LogLevel, target: String, message: String);
}

/// The `log` facade's end of a [`Logger`].
struct Forwarder(Arc<dyn Logger>);

impl Log for Forwarder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        self.0.log(
            record.level().into(),
            record.target().to_owned(),
            record.args().to_string(),
        );
    }

    fn flush(&self) {}
}

/// Route the core's logs to the shell, at or above `level`.
///
/// Answers whether it was installed: `log` takes one logger for the process and
/// refuses a second, so a second call is a no-op rather than an error, and a
/// test binary that has already installed its own keeps it.
#[uniffi::export]
pub fn init_logging(logger: Arc<dyn Logger>, level: LogLevel) -> bool {
    let installed = log::set_boxed_logger(Box::new(Forwarder(logger))).is_ok();

    if installed {
        log::set_max_level(level.into());
    }

    installed
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::{LogLevel, Logger};

    #[derive(Default)]
    struct Collected(Mutex<Vec<(LogLevel, String, String)>>);

    impl Logger for Collected {
        fn log(&self, level: LogLevel, target: String, message: String) {
            self.0.lock().unwrap().push((level, target, message));
        }
    }

    #[test]
    fn a_record_reaches_the_shell_with_its_level_and_target() {
        let logger = std::sync::Arc::new(Collected::default());
        let forwarder = super::Forwarder(logger.clone());

        log::Log::log(
            &forwarder,
            &log::Record::builder()
                .level(log::Level::Error)
                .target("dip::node")
                .args(format_args!("a link went down"))
                .build(),
        );

        assert_eq!(
            logger.0.lock().unwrap().as_slice(),
            [(
                LogLevel::Error,
                "dip::node".to_owned(),
                "a link went down".to_owned()
            )]
        );
    }

    #[test]
    fn the_facade_takes_one_logger_and_a_second_call_is_a_no_op() {
        let logger = std::sync::Arc::new(Collected::default());
        super::init_logging(logger.clone(), LogLevel::Debug);

        assert!(!super::init_logging(logger, LogLevel::Debug));
    }
}
