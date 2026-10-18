use std::any::Any;
use std::fmt;

use serde::Serialize;

/// What kind of failure an [`Error`] reports, spelled on the wire as [`ErrorCode::as_str`] gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ErrorCode {
    /// The request could not be read: a null pointer, bytes that are not UTF-8, or params that are not JSON.
    InvalidRequest,
    /// The params are JSON but not the method's: a missing or unknown field, a value of the wrong type, or a name no
    /// language or option carries.
    InvalidParams,
    /// No method has the name asked for.
    UnknownMethod,
    /// The root `open` was given is not a directory that can be read.
    InvalidRoot,
    /// The method needs a build, and the handle holds none: no build has finished, or the last one failed or was
    /// cancelled.
    NotBuilt,
    /// The method needs a build, and the handle holds none because a build is running.
    Building,
    /// The build stopped on a failure that leaves no index, such as a root that can no longer be listed.
    BuildFailed,
    /// A cancel stopped the build, which leaves no index.
    BuildCancelled,
    /// A glob the params gave does not parse; the message names it.
    InvalidGlob,
    /// A content search pattern does not compile; the message names it.
    InvalidPattern,
    /// Nothing is indexed at the path the params name, or a name on the way is one the build prunes.
    NotFound,
    /// The path the params name is indexed as something other than a directory.
    NotADirectory,
    /// The engine panicked. The handle stays usable, and the message carries the panic's.
    Internal,
}

impl ErrorCode {
    /// The code as the wire spells it, such as `not_built`.
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
            Self::InvalidGlob => "invalid_glob",
            Self::InvalidPattern => "invalid_pattern",
            Self::NotFound => "not_found",
            Self::NotADirectory => "not_a_directory",
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
