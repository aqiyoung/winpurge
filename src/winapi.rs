//! Thin, hand-rolled FFI for the couple of Win32 calls this tool needs.
//!
//! We deliberately avoid the `windows-sys`/`windows` crates here: they pull in
//! a large feature matrix and their handle/string pointer types have changed
//! between releases. The four functions below are stable ABI entry points, so
//! declaring them directly keeps the build reproducible and the surface tiny.

use std::ffi::c_void;
use std::path::Path;

#[repr(C)]
struct TokenElevation {
    token_is_elevated: u32,
}

/// `TokenElevation` from the `TOKEN_INFORMATION_CLASS` enumeration.
const TOKEN_ELEVATION_CLASS: i32 = 20;
const TOKEN_QUERY: u32 = 0x0008;

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetCurrentProcess() -> *mut c_void;
        pub fn CloseHandle(handle: *mut c_void) -> i32;
        pub fn GetDiskFreeSpaceExW(
            lpfilename: *const u16,
            lpfreebytesavailable: *mut u64,
            lptotalnumberofbytes: *mut u64,
            lptotalnumberoffreebytes: *mut u64,
        ) -> i32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        pub fn OpenProcessToken(
            processhandle: *mut c_void,
            desiredaccess: u32,
            tokenhandle: *mut *mut c_void,
        ) -> i32;
        pub fn GetTokenInformation(
            tokenhandle: *mut c_void,
            tokeninformationclass: i32,
            tokeninformation: *mut c_void,
            tokeninformationlength: u32,
            returnlength: *mut u32,
        ) -> i32;
    }
}

/// True when the current process is running with an elevated (admin) token.
pub fn is_elevated() -> bool {
    #[cfg(windows)]
    unsafe {
        let mut token: *mut c_void = std::ptr::null_mut();
        if sys::OpenProcessToken(sys::GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TokenElevation {
            token_is_elevated: 0,
        };
        let mut returned: u32 = 0;
        let ok = sys::GetTokenInformation(
            token,
            TOKEN_ELEVATION_CLASS,
            &mut elevation as *mut TokenElevation as *mut c_void,
            std::mem::size_of::<TokenElevation>() as u32,
            &mut returned,
        );
        sys::CloseHandle(token);
        ok != 0 && elevation.token_is_elevated != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub struct Space {
    pub total: u64,
    pub free: u64,
}

/// Total and free bytes for the volume that `path` lives on.
pub fn drive_space(path: &Path) -> Option<Space> {
    #[cfg(windows)]
    unsafe {
        use std::os::windows::ffi::OsStrExt;

        let mut wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut available = 0u64;
        let mut total = 0u64;
        let mut _free = 0u64;
        let ok = sys::GetDiskFreeSpaceExW(
            wide.as_mut_ptr(),
            &mut available,
            &mut total,
            &mut _free,
        );
        if ok == 0 {
            return None;
        }
        Some(Space {
            total,
            free: available,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}
