use std::any::Any;
use std::fmt;

use serde::Serialize;

/// The code a failure is answered with, as the wire spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ErrorCode {
    /// A null handle or method, a method or params that are not UTF-8, null params with a length, or params that are
    /// not JSON.
    InvalidRequest,
    /// Params the method does not take, the message naming what: JSON that is neither an object nor `null`, a field
    /// that is unknown, missing or of the wrong type, a value the method refuses (a symbol kind it does not know, a
    /// malformed name in a build option's set, a refresh path that is absolute, starts with a drive, or holds a `\` or a `..` name), or a graph the
    /// build did not make.
    InvalidParams,
    /// No method of the name; the message lists the methods.
    UnknownMethod,
    /// `open` could not read the root's metadata, or the root is not a directory.
    InvalidRoot,
    /// No build has finished, or the last one failed, or a cancel stopped it and the method needs more than its index.
    NotBuilt,
    /// A build is in flight.
    Building,
    /// The build stopped on a failure, which leaves no index.
    BuildFailed,
    /// A cancel stopped the build, whose index the file queries answer from when it holds an entry, or that the cancel
    /// came too late to stop.
    BuildCancelled,
    /// The build kept no stamps, so a refresh is refused.
    NoStamps,
    /// The refresh stopped on a failure.
    RefreshFailed,
    /// A cancel stopped the refresh.
    RefreshCancelled,
    /// A glob the params name does not compile, answered by the call that takes it.
    InvalidGlob,
    /// A grep pattern does not compile.
    InvalidPattern,
    /// `list` cannot list the path, by the [`label`](kartta::ListError::label) of why.
    Listing(&'static str),
    /// The engine panicked, `kartta_call` could not start its thread, or an answer could not be encoded. After a call
    /// the handle stays usable, though a refresh that panicked partway leaves every call that reads the engine
    /// answering this code, naming `build`, until a build replaces the engine; an `open` that answers it made no
    /// handle.
    Internal,
}

impl ErrorCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidParams => "invalid_params",
            Self::UnknownMethod => "unknown_method",
            Self::InvalidRoot => "invalid_root",
            Self::NotBuilt => "not_built",
            Self::Building => "building",
            Self::BuildFailed => "build_failed",
            Self::BuildCancelled => "build_cancelled",
            Self::NoStamps => "no_stamps",
            Self::RefreshFailed => "refresh_failed",
            Self::RefreshCancelled => "refresh_cancelled",
            Self::InvalidGlob => "invalid_glob",
            Self::InvalidPattern => "invalid_pattern",
            Self::Listing(label) => label,
            Self::Internal => "internal",
        }
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// A failure the wire answers as `{"err": {"code": ..., "message": ...}}`: a code to branch on, and a message that
/// names what failed and, where the handle knows it, what the caller can ask instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Error {
    code: ErrorCode,
    message: String,
}

impl Error {
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn panicked(payload: &(dyn Any + Send)) -> Self {
        let reason = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("a panic without a message");
        Self::new(
            ErrorCode::Internal,
            format!("the engine panicked: {reason}"),
        )
    }

    /// The error as the wire's `{"code": ..., "message": ...}` object.
    #[must_use]
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_else(|_| INTERNAL_ENCODE_FAILURE.to_vec())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for Error {}

pub(crate) const INTERNAL_ENCODE_FAILURE: &[u8] =
    br#"{"code":"internal","message":"the answer could not be encoded as JSON"}"#;
