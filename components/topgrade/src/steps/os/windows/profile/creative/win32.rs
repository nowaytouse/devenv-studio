use color_eyre::eyre::{Result, ensure, eyre};
use std::{
    ffi::c_void,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
};
use windows::{
    Win32::{
        Foundation::WAIT_OBJECT_0,
        Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
        System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject},
        UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
    },
    core::{HSTRING, PCWSTR, w},
};

// The Windows API returns pointers into this aligned, owned version-resource buffer.
fn query<'a>(resource: &'a [u32], key: &str, unit: usize) -> Result<&'a [u16]> {
    let mut ptr: *mut c_void = ptr::null_mut();
    let mut count = 0;
    ensure!(
        unsafe { VerQueryValueW(resource.as_ptr().cast(), &HSTRING::from(key), &mut ptr, &mut count) }.as_bool(),
        "Missing Windows version field {key}"
    );
    let bytes = count as usize * unit;
    let offset = (ptr as usize)
        .checked_sub(resource.as_ptr() as usize)
        .ok_or_else(|| eyre!("Invalid version resource offset"))?;
    ensure!(
        offset.is_multiple_of(2)
            && bytes.is_multiple_of(2)
            && offset
                .checked_add(bytes)
                .is_some_and(|end| end <= std::mem::size_of_val(resource)),
        "Invalid version resource range"
    );
    Ok(unsafe { std::slice::from_raw_parts(ptr.cast(), bytes / 2) })
}
pub(super) fn version(exe: &Path, key: &str) -> Result<String> {
    let path = HSTRING::from(exe.as_os_str());
    let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
    ensure!(
        (1..=16 * 1024 * 1024).contains(&size),
        "Missing or invalid version information for {}",
        exe.display()
    );
    let mut data = vec![0u32; (size as usize).div_ceil(4)];
    unsafe {
        GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast())?;
    }
    for lang in query(&data, r"\VarFileInfo\Translation", 1)?.as_chunks::<2>().0 {
        let sub = format!(r"\StringFileInfo\{:04x}{:04x}\{key}", lang[0], lang[1]);
        if let Ok(s) = query(&data, &sub, 2) {
            return Ok(String::from_utf16(s.strip_suffix(&[0]).unwrap_or(s))?);
        }
    }
    Err(eyre!("Missing translated version field {key}"))
}
pub(super) fn elevated_krita(setup: &Path, root: &Path) -> Result<()> {
    let path = HSTRING::from(setup.as_os_str());
    // NSIS requires /D= to be the final, unquoted argument, including any spaces.
    let args: Vec<u16> = "/S /D="
        .encode_utf16()
        .chain(root.as_os_str().encode_wide())
        .chain([0])
        .collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(path.as_ptr()),
        lpParameters: PCWSTR(args.as_ptr()),
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info)?;
        ensure!(!info.hProcess.is_invalid(), "Missing elevated installer process handle");
        let _owned = OwnedHandle::from_raw_handle(info.hProcess.0);
        ensure!(
            WaitForSingleObject(info.hProcess, INFINITE) == WAIT_OBJECT_0,
            "Failed to wait for the Krita installer"
        );
        let mut exit = 0;
        GetExitCodeProcess(info.hProcess, &mut exit)?;
        ensure!(exit == 0, "Krita installer returned {exit}");
    }
    Ok(())
}
