//! Node.js addon of the kartta code graph engine: `kartta-ffi`'s JSON dispatch, called off the event loop.
//!
//! The addon decides nothing. Every method takes the wire's params as a JSON string and answers the wire's envelope,
//! `{"ok": <answer>}` or `{"err": {"code": <code>, "message": <message>}}`, as the dispatch's UTF-8 bytes in a
//! `Buffer`; the package in `bindings/node` parses it and types it. A build runs on a thread of its own, so a walk of
//! millions of entries never holds one of libuv's four pool threads; a query runs on that pool; a cancel answers at
//! once on the calling thread, so it is never queued behind the work it stops.
#![expect(
    clippy::needless_pass_by_value,
    reason = "napi reads a JS string argument only into an owned String, and copies an item's attributes onto the \
              glue it generates, where an expectation on the item goes unfulfilled"
)]

use std::sync::Arc;
use std::thread;

use napi::bindgen_prelude::{AsyncTask, Buffer, Either, Object};
use napi::{Env, Error, Result, Task};
use napi_derive::napi;

/// Opens a handle on the tree that `params`, the JSON `{"root": <path>}`, names, or answers why not as the wire's
/// `{"code", "message"}` error object.
#[napi]
#[must_use]
pub fn open(params: String) -> Either<Handle, Buffer> {
    match kartta_ffi::Kartta::open(params.as_bytes()) {
        Ok(kartta) => Either::A(Handle {
            kartta: Some(Arc::new(kartta)),
        }),
        Err(error) => Either::B(error.to_json().into()),
    }
}

/// A handle on one tree. Each call that runs off the event loop holds the handle's engine until it answers, so
/// [`Handle::close`] frees the engine once the last of them has.
#[napi]
#[derive(Debug)]
pub struct Handle {
    kartta: Option<Arc<kartta_ffi::Kartta>>,
}

#[napi]
impl Handle {
    /// Runs the `build` method on a thread of its own, answering the envelope as a promise.
    ///
    /// # Errors
    ///
    /// The handle is closed, or the thread or the promise could not be made.
    #[napi]
    pub fn build<'env>(&self, env: &'env Env, params: String) -> Result<Object<'env>> {
        self.on_a_thread_of_its_own(env, "build", params)
    }

    /// Runs any method but `build` on libuv's thread pool, answering the envelope as a promise.
    ///
    /// # Errors
    ///
    /// The handle is closed.
    #[napi]
    pub fn call(&self, method: String, params: String) -> Result<AsyncTask<Query>> {
        Ok(AsyncTask::new(Query {
            kartta: self.kartta()?,
            method,
            params,
        }))
    }

    /// Runs the `cancel` method on the calling thread, answering the envelope.
    ///
    /// # Errors
    ///
    /// The handle is closed.
    #[napi]
    pub fn cancel(&self, params: String) -> Result<Buffer> {
        Ok(self.kartta()?.call("cancel", params.as_bytes()).into())
    }

    /// Lets the engine go once the calls in flight answer; every later call fails. It cancels nothing: the caller
    /// cancels the builds it started first.
    #[napi]
    pub fn close(&mut self) {
        self.kartta = None;
    }

    fn on_a_thread_of_its_own<'env>(
        &self,
        env: &'env Env,
        method: &'static str,
        params: String,
    ) -> Result<Object<'env>> {
        let kartta = self.kartta()?;
        let (deferred, promise) = env.create_deferred()?;
        thread::Builder::new()
            .name(format!("kartta-{method}"))
            .spawn(move || {
                let answer = kartta.call(method, params.as_bytes());
                deferred.resolve(move |_| Ok(Buffer::from(answer)));
            })
            .map_err(|error| Error::from_reason(format!("no thread for the {method}: {error}")))?;
        Ok(promise)
    }

    fn kartta(&self) -> Result<Arc<kartta_ffi::Kartta>> {
        self.kartta
            .clone()
            .ok_or_else(|| Error::from_reason("the handle is closed"))
    }
}

/// A method answered on libuv's thread pool.
#[derive(Debug)]
pub struct Query {
    kartta: Arc<kartta_ffi::Kartta>,
    method: String,
    params: String,
}

impl Task for Query {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Vec<u8>> {
        Ok(self.kartta.call(&self.method, self.params.as_bytes()))
    }

    fn resolve(&mut self, _: Env, answer: Vec<u8>) -> Result<Buffer> {
        Ok(answer.into())
    }
}
