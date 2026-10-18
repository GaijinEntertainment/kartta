//! The JSON the dispatch reads and writes: each method's params, decoded with unknown fields refused, and each
//! answer, a borrowing view over what the engine answered, so encoding copies no string twice.

mod build;
pub(crate) mod files;
pub(crate) mod graph;
pub(crate) mod params;

use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub(crate) use self::build::{BuildParams, CancelAnswer, CancelParams, StatsAnswer, StoredErrors};
use crate::error::{Error, ErrorCode, INTERNAL_ENCODE_FAILURE};

/// Decodes `params` as the method's params, empty bytes and `null` reading as `{}`.
pub(crate) fn params<P: DeserializeOwned>(params: &[u8]) -> Result<P, Error> {
    let text = std::str::from_utf8(params)
        .map_err(|error| {
            Error::new(
                ErrorCode::InvalidRequest,
                format!("params are not UTF-8: {error}"),
            )
        })?
        .trim();
    let text = if text.is_empty() || text == "null" {
        "{}"
    } else {
        text
    };
    serde_json::from_str(text).map_err(|error| {
        let code = if error.is_syntax() || error.is_eof() {
            ErrorCode::InvalidRequest
        } else {
            ErrorCode::InvalidParams
        };
        Error::new(code, format!("params: {error}"))
    })
}

/// The wire's `{"ok": ...}` answer for `answer`.
pub(crate) fn ok(answer: &impl Serialize) -> Result<Vec<u8>, Error> {
    #[derive(Serialize)]
    struct Answered<'a, T> {
        ok: &'a T,
    }

    serde_json::to_vec(&Answered { ok: answer }).map_err(|error| {
        Error::new(
            ErrorCode::Internal,
            format!("the answer could not be encoded as JSON: {error}"),
        )
    })
}

/// The wire's `{"err": ...}` answer for `error`.
pub(crate) fn err(error: &Error) -> Vec<u8> {
    #[derive(Serialize)]
    struct Failed<'a> {
        err: &'a Error,
    }

    serde_json::to_vec(&Failed { err: error })
        .unwrap_or_else(|_| [&br#"{"err":"#[..], INTERNAL_ENCODE_FAILURE, b"}"].concat())
}

/// The params of a method that takes none: `{}`, or nothing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoParams {}

/// The params of `open`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenParams {
    pub(crate) root: PathBuf,
}
