//! Error types and exit-code mapping.
//!
//! skilldigest follows the convention used by most Unix linters:
//!
//! - **0** — scan completed, no issues at error severity
//! - **1** — issues found at error severity
//! - **2** — operational error (bad args, I/O, parse that halted)
//!
//! The [`ExitCode`] enum is a small plain-old-data type so it can be matched
//! at the top of `main` without pulling in `std::process::ExitCode` which has
//! a slightly noisier API.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Top-level error type. All fallible library functions return [`Result<T>`].
#[derive(Debug, Error)]
pub enum Error {
    /// File or directory could not be read or written.
    #[error("I/O error on {path}: {source}")]
    Io {
        /// Path that triggered the error.
        path: PathBuf,
        #[source]
        /// Underlying error.
        source: io::Error,
    },

    /// Input argument was malformed (e.g. tokenizer name unknown, budget < 0).
    #[error("invalid argument: {0}")]
    BadArg(String),

    /// Config file (`.skilldigest.toml`) could not be parsed.
    #[error("config error ({path}): {message}")]
    Config {
        /// Path to the config file.
        path: PathBuf,
        /// Diagnostic message.
        message: String,
    },

    /// Tokenizer could not be constructed.
    #[error("tokenizer '{0}' is not supported; known: cl100k, o200k, llama3")]
    UnknownTokenizer(String),

    /// Output format not recognised.
    #[error("output format '{0}' is not supported; known: text, json, sarif, markdown, dot")]
    UnknownFormat(String),

    /// Scan root does not exist or is not a directory.
    #[error("scan root is not a directory: {0}")]
    BadRoot(PathBuf),

    /// UTF-8 validation failed in a context where strict decoding is required.
    #[error("non-UTF-8 content in {path}")]
    NonUtf8 {
        /// Path to the offending file.
        path: PathBuf,
    },

    /// Catch-all for wrapped `anyhow::Error` in CLI glue code.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Canonical process exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitCode {
    /// Scan clean.
    Clean = 0,
    /// One or more error-severity issues were found.
    IssuesFound = 1,
    /// Operational failure — bad args, IO, malformed config, etc.
    OperationalError = 2,
}

impl ExitCode {
    /// Raw integer representation, suitable for [`std::process::exit`].
    #[must_use]
    pub fn as_i32(self) -> i32 {
        self as u8 as i32
    }
}

impl From<ExitCode> for i32 {
    fn from(code: ExitCode) -> Self {
        code.as_i32()
    }
}

/// Convenience `Result` alias.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Construct an I/O error with a path context.
    #[must_use]
    pub fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Construct a BadArg error.
    #[must_use]
    pub fn bad_arg(msg: impl Into<String>) -> Self {
        Self::BadArg(msg.into())
    }

    /// Map an error to its canonical exit code.
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Io { .. }
            | Self::BadArg(_)
            | Self::Config { .. }
            | Self::UnknownTokenizer(_)
            | Self::UnknownFormat(_)
            | Self::BadRoot(_)
            | Self::NonUtf8 { .. }
            | Self::Other(_) => ExitCode::OperationalError,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_canonical() {
        assert_eq!(ExitCode::Clean.as_i32(), 0);
        assert_eq!(ExitCode::IssuesFound.as_i32(), 1);
        assert_eq!(ExitCode::OperationalError.as_i32(), 2);
    }

    #[test]
    fn bad_arg_factory() {
        let e = Error::bad_arg("no good");
        assert!(matches!(e, Error::BadArg(_)));
        assert!(format!("{e}").contains("no good"));
    }

    #[test]
    fn io_error_includes_path() {
        let e = Error::io(
            "/tmp/missing",
            io::Error::new(io::ErrorKind::NotFound, "oh no"),
        );
        let msg = format!("{e}");
        assert!(msg.contains("/tmp/missing"), "msg = {msg}");
    }

    #[test]
    fn unknown_tokenizer_is_operational() {
        let e = Error::UnknownTokenizer("foo".into());
        assert_eq!(e.exit_code(), ExitCode::OperationalError);
    }

    #[test]
    fn exit_code_converts_to_i32() {
        let n: i32 = ExitCode::IssuesFound.into();
        assert_eq!(n, 1);
    }
}
