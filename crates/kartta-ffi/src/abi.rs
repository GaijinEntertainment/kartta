//! The C functions `include/kartta.h` declares, which states the contract each one keeps and asks of its caller.

use std::any::Any;
use std::ffi::{CStr, c_char};
use std::panic::{self, AssertUnwindSafe};
use std::{ptr, slice};

use crate::error::{Error, ErrorCode};
use crate::wire::err as answer_error;
use crate::{ABI_VERSION, Kartta};

#[repr(C)]
struct KarttaString {
    ptr: *mut u8,
    len: usize,
}

impl KarttaString {
    const NULL: Self = Self {
        ptr: ptr::null_mut(),
        len: 0,
    };

    fn new(bytes: Vec<u8>) -> Self {
        let len = bytes.len();
        let ptr = Box::into_raw(bytes.into_boxed_slice()).cast::<u8>();
        Self { ptr, len }
    }
}

#[unsafe(no_mangle)]
const extern "C" fn kartta_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
unsafe extern "C" fn kartta_open(
    params: *const u8,
    params_len: usize,
    error: *mut KarttaString,
) -> *mut Kartta {
    let opened = guard(
        || {
            // SAFETY: the header asks for `params_len` readable bytes at `params`, or a null `params`.
            let params = unsafe { bytes(params, params_len) }?;
            Kartta::open(params)
        },
        |panic| Err(Error::panicked(panic)),
    );
    let (handle, reported) = match opened {
        Ok(kartta) => (Box::into_raw(Box::new(kartta)), KarttaString::NULL),
        Err(failure) => (ptr::null_mut(), KarttaString::new(failure.to_json())),
    };
    if error.is_null() {
        // SAFETY: a non-null `reported` was made by `KarttaString::new` above and nothing else holds it.
        unsafe { kartta_string_free(reported) };
    } else {
        // SAFETY: the header asks for a null `error` or one that points to a writable `KarttaString`.
        unsafe { error.write(reported) };
    }
    handle
}

#[unsafe(no_mangle)]
unsafe extern "C" fn kartta_call(
    handle: *const Kartta,
    method: *const c_char,
    params: *const u8,
    params_len: usize,
) -> KarttaString {
    let answer = guard(
        || {
            // SAFETY: the header asks for a null handle or one `kartta_open` returned and `kartta_free` has not freed.
            let Some(kartta) = (unsafe { handle.as_ref() }) else {
                return answer_error(&invalid_request("the handle is null"));
            };
            if method.is_null() {
                return answer_error(&invalid_request("the method is null"));
            }
            // SAFETY: the header asks for a non-null `method` to point to a NUL-terminated string.
            let Ok(method) = unsafe { CStr::from_ptr(method) }.to_str() else {
                return answer_error(&invalid_request("the method is not UTF-8"));
            };
            // SAFETY: the header asks for `params_len` readable bytes at `params`, or a null `params`.
            match unsafe { bytes(params, params_len) } {
                Ok(params) => kartta.call(method, params),
                Err(error) => answer_error(&error),
            }
        },
        |panic| answer_error(&Error::panicked(panic)),
    );
    KarttaString::new(answer)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn kartta_string_free(string: KarttaString) {
    if string.ptr.is_null() {
        return;
    }
    guard(
        || {
            // SAFETY: a non-null string was made by `KarttaString::new`, which leaked a boxed slice of exactly
            // `len` bytes, and the header asks for each string to be freed once.
            drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(string.ptr, string.len)) });
        },
        |_| {},
    );
}

#[unsafe(no_mangle)]
unsafe extern "C" fn kartta_free(handle: *mut Kartta) {
    if handle.is_null() {
        return;
    }
    guard(
        || {
            // SAFETY: the header asks for a handle `kartta_open` returned, freed once, with no call running on it.
            drop(unsafe { Box::from_raw(handle) });
        },
        |_| {},
    );
}

/// Runs `body`, answering `recover` of the panic's payload instead of letting a panic unwind into C.
fn guard<T>(body: impl FnOnce() -> T, recover: impl FnOnce(&(dyn Any + Send)) -> T) -> T {
    // An entry point's state is the caller's pointers and a handle whose builds keep their own state consistent across
    // a panic, so nothing a panic interrupts is observed half-changed afterwards.
    panic::catch_unwind(AssertUnwindSafe(body)).unwrap_or_else(|payload| recover(payload.as_ref()))
}

/// The `len` bytes at `ptr`; none when `ptr` is null and `len` is 0.
///
/// # Safety
///
/// A non-null `ptr` points to `len` bytes that stay readable and unchanged until the borrow ends.
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Result<&'a [u8], Error> {
    if ptr.is_null() {
        return if len == 0 {
            Ok(&[])
        } else {
            Err(invalid_request("the params are null with a nonzero length"))
        };
    }
    // SAFETY: the caller guarantees `len` readable bytes at the non-null `ptr`.
    Ok(unsafe { slice::from_raw_parts(ptr, len) })
}

fn invalid_request(reason: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, reason)
}
