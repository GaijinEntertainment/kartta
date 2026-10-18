mod build;
pub(crate) mod files;
pub(crate) mod graph;
pub(crate) mod params;

use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub(crate) use self::build::{BuildParams, CancelAnswer, CancelParams, StatsAnswer, StoredErrors};
use crate::error::{Error, ErrorCode, INTERNAL_ENCODE_FAILURE};

pub(crate) fn params<P: DeserializeOwned>(params: &[u8]) -> Result<P, Error> {
    let text = std::str::from_utf8(params)
        .map_err(|error| {
            Error::new(
                ErrorCode::InvalidRequest,
                format!("params are not UTF-8: {error}"),
            )
        })?
        .trim();
    // constraint: the Go binding sends the params of a method it calls with none, stats and version, as null
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

pub(crate) fn err(error: &Error) -> Vec<u8> {
    #[derive(Serialize)]
    struct Failed<'a> {
        err: &'a Error,
    }

    serde_json::to_vec(&Failed { err: error })
        .unwrap_or_else(|_| [&br#"{"err":"#[..], INTERNAL_ENCODE_FAILURE, b"}"].concat())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoParams {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenParams {
    pub(crate) root: PathBuf,
}
