use std::path::Path;

#[cfg(target_os = "macos")]
pub(crate) fn on_disk(native_path: &Path) -> Option<String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // constraint: the answer is its own length, then an `attrreference_t` — an offset from the reference itself and a
    // length that counts the terminating NUL — then the name, at most 255 UTF-16 code units on HFS+, which UTF-8 spells
    // in 765 bytes
    const REFERENCE: usize = 4;
    const ANSWER: usize = REFERENCE + 8 + 768;

    let path = CString::new(native_path.as_os_str().as_bytes()).ok()?;
    let mut asked = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_NAME,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut answer = [0_u8; ANSWER];
    // SAFETY: `path` is NUL-terminated, and `asked` and `answer` outlive the call, which writes at most `answer.len()`
    // bytes into `answer`.
    let returned = unsafe {
        libc::getattrlist(
            path.as_ptr(),
            (&raw mut asked).cast(),
            answer.as_mut_ptr().cast(),
            answer.len(),
            libc::FSOPT_NOFOLLOW,
        )
    };
    if returned != 0 {
        return None;
    }

    let word = |at: usize| Some(u32::from_ne_bytes(answer.get(at..at + 4)?.try_into().ok()?));
    let start = REFERENCE + usize::try_from(word(REFERENCE)?).ok()?;
    let len = usize::try_from(word(REFERENCE + 4)?).ok()?.checked_sub(1)?;
    let name = answer.get(start..start.checked_add(len)?)?;
    String::from_utf8(name.to_vec()).ok()
}

#[cfg(windows)]
pub(crate) fn on_disk(native_path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use std::{iter, ptr};

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, WIN32_FIND_DATAW,
    };

    // constraint: the search takes these as wildcards, so a name holding one would answer another entry's name
    if native_path
        .file_name()?
        .to_str()?
        .contains(['*', '?', '<', '>', '"'])
    {
        return None;
    }
    let path: Vec<u16> = native_path
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    let mut found = WIN32_FIND_DATAW::default();
    // SAFETY: `path` is NUL-terminated, and `found` outlives the call, which fills it as the basic information level
    // defines.
    let search = unsafe {
        FindFirstFileExW(
            path.as_ptr(),
            FindExInfoBasic,
            (&raw mut found).cast(),
            FindExSearchNameMatch,
            ptr::null(),
            0,
        )
    };
    if search == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: the handle is the open search FindFirstFileExW returned, and nothing uses it after this.
    unsafe {
        FindClose(search);
    }

    let len = found.cFileName.iter().position(|&unit| unit == 0)?;
    String::from_utf16(found.cFileName.get(..len)?).ok()
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) const fn on_disk(_native_path: &Path) -> Option<String> {
    None
}
