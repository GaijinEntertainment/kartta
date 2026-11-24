mod build;
pub(crate) mod files;
pub(crate) mod graph;
pub(crate) mod params;
mod refresh;

use std::path::PathBuf;

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};

pub(crate) use self::build::{
    BuildParams, By, CancelAnswer, CancelParams, StatsAnswer, StoppedView, StoredErrors,
};
pub(crate) use self::refresh::{RefreshAnswer, RefreshParams};
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
    // constraint: serde reads a struct from a JSON array by position, so a non-object never reaches the struct
    if let Some(kind) = not_an_object(text)
        && serde_json::from_str::<IgnoredAny>(text).is_ok()
    {
        return Err(Error::new(
            ErrorCode::InvalidParams,
            format!("params are a JSON object or null, not {kind}"),
        ));
    }
    serde_json::from_str(text).map_err(|error| {
        let code = if error.is_syntax() || error.is_eof() {
            ErrorCode::InvalidRequest
        } else {
            ErrorCode::InvalidParams
        };
        Error::new(code, format!("params: {error}"))
    })
}

fn not_an_object(text: &str) -> Option<&'static str> {
    match text.bytes().next()? {
        b'[' => Some("an array"),
        b'"' => Some("a string"),
        b't' | b'f' => Some("a boolean"),
        b'-' | b'0'..=b'9' => Some("a number"),
        _ => None,
    }
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

pub(crate) use no_params::NoParams;

#[expect(
    clippy::empty_enums,
    reason = "serde's derive names a struct's fields in an enum, which has no variant for a struct of no fields"
)]
mod no_params {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct NoParams {}
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenParams {
    pub(crate) root: PathBuf,
}
