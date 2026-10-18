//! The JSON the dispatch reads and writes: each method's params, decoded with unknown fields refused, and each
//! answer, a borrowing view over what the engine answered, so encoding copies no string twice.

mod build;
mod graph;

use std::path::PathBuf;

use serde::Deserialize;
use serde::de::DeserializeOwned;

pub(crate) use self::build::{BuildParams, CancelAnswer, CancelParams, StatsAnswer, StoredErrors};
pub(crate) use self::graph::{WalkParams, Walks};
use crate::error::{Error, ErrorCode};

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
