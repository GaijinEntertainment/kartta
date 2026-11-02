use std::any::Any;
use std::ffi::{CStr, c_char};
use std::panic::{self, AssertUnwindSafe};
use std::{ptr, slice, thread};

use crate::error::{Error, ErrorCode};
use crate::wire::err as answer_error;
use crate::{ABI_VERSION, Kartta};

// constraint: a caller's thread can be as small as musl's 131,072 bytes, and an overflow kills the process past any
// `catch_unwind`, where the deepest call a pattern can ask for takes 2.2 MiB in a debug build
const CALL_STACK: usize = 8 * 1024 * 1024;

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
                Ok(params) => on_call_stack(|| kartta.call(method, params)),
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

/// Runs `call` on a thread of [`CALL_STACK`] bytes and waits for it, whatever stack the caller's thread has.
fn on_call_stack(call: impl FnOnce() -> Vec<u8> + Send) -> Vec<u8> {
    thread::scope(|scope| {
        match thread::Builder::new()
            .stack_size(CALL_STACK)
            .spawn_scoped(scope, call)
        {
            Ok(running) => running
                .join()
                .unwrap_or_else(|panic| answer_error(&Error::panicked(panic.as_ref()))),
            Err(error) => answer_error(&Error::new(
                ErrorCode::Internal,
                format!("no thread for the call: {error}"),
            )),
        }
    })
}

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

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::fs;

    use serde_json::{Value, json};

    use super::*;

    const MUSL_THREAD_STACK: usize = 131_072;

    fn call_on_small_stack(kartta: &Kartta, method: &str, params: &Value) -> Value {
        let method = CString::new(method).expect("a method name without NUL");
        let params = params.to_string();
        thread::scope(|scope| {
            thread::Builder::new()
                .stack_size(MUSL_THREAD_STACK)
                .spawn_scoped(scope, || {
                    // SAFETY: the handle, the name and the params all outlive the call.
                    let answer = unsafe {
                        kartta_call(
                            ptr::from_ref(kartta),
                            method.as_ptr(),
                            params.as_ptr(),
                            params.len(),
                        )
                    };
                    // SAFETY: `kartta_call` answers `len` readable bytes at a non-null `ptr`.
                    let text = unsafe { slice::from_raw_parts(answer.ptr, answer.len) };
                    let value = serde_json::from_slice(text).expect("the answer is JSON");
                    // SAFETY: the string came from `kartta_call`, is freed once, and `text` ends here.
                    unsafe { kartta_string_free(answer) };
                    value
                })
                .expect("a thread for the call")
                .join()
                .expect("the call returns")
        })
    }

    #[test]
    fn a_pattern_nested_to_the_parser_limit_survives_a_small_stack_of_the_caller() {
        let tree = tempfile::tempdir().expect("a temporary directory");
        fs::write(tree.path().join("hit.txt"), "b\n").expect("hit.txt");
        let opened = json!({ "root": tree.path() }).to_string();
        let kartta = Kartta::open(opened.as_bytes()).expect("the tree opens");
        let built = kartta.call("build", br#"{"languages":[]}"#);
        assert!(built.starts_with(br#"{"ok""#), "the build fails");

        let pattern = format!("{}b{}", "(".repeat(124), ")*".repeat(124));
        let answer = call_on_small_stack(&kartta, "grep", &json!({ "patterns": [pattern] }));

        assert_eq!(answer["ok"]["lines"], 1, "{answer}");
    }
}
