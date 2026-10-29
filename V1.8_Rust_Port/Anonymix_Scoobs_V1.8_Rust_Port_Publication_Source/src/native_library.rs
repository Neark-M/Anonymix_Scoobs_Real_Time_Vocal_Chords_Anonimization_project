//! Thread-bound dynamic library ownership for SDL's C API. PortAudio retains
//! its separate, already validated loader and COM lifetime policy.
#[cfg(not(windows))]
use std::ffi::{c_char, CStr};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::{ffi::c_void, marker::PhantomData, path::Path, rc::Rc};
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryExW(path: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> i32;
}
#[cfg(not(windows))]
#[link(name = "dl")]
extern "C" {
    fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(module: *mut c_void) -> i32;
    fn dlerror() -> *const c_char;
}
/// Thread-confined dynamic-library owner. Symbols must not outlive this object.
pub struct NativeLibrary {
    handle: *mut c_void,
    _same_thread: PhantomData<Rc<()>>,
}
impl NativeLibrary {
    /// Load a canonical library path with the platform loader; return a descriptive loader error on failure.
    pub fn open(path: &Path) -> Result<Self, String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        #[cfg(windows)]
        let handle = {
            let mut wide: Vec<_> = path.as_os_str().encode_wide().collect();
            if wide.contains(&0) {
                return Err("Library path contains a NUL".into());
            }
            wide.push(0);
            // SAFETY: absolute, stable, terminated path; dependency directory/system search.
            unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), 0x1100) }
        };
        #[cfg(not(windows))]
        let handle = {
            use std::os::unix::ffi::OsStrExt;
            let name =
                std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
            // SAFETY: terminated path. GLOBAL lets the test SDL_ttf resolve SDL symbols.
            unsafe { dlopen(name.as_ptr(), 2 | 0x100) }
        };
        if handle.is_null() {
            return Err(format!(
                "Cannot load {}: {}",
                path.display(),
                platform_error()
            ));
        }
        Ok(Self {
            handle,
            _same_thread: PhantomData,
        })
    }
    /// Resolve a NUL-terminated export name. The caller must use its exact C signature and retain the library.
    pub fn symbol(&self, name: &[u8]) -> Result<*mut c_void, String> {
        assert_eq!(name.last(), Some(&0));
        #[cfg(windows)]
        let pointer = unsafe { GetProcAddress(self.handle, name.as_ptr()) };
        #[cfg(not(windows))]
        let pointer = unsafe { dlsym(self.handle, name.as_ptr().cast()) };
        if pointer.is_null() {
            return Err(format!(
                "Missing SDL export {}",
                String::from_utf8_lossy(&name[..name.len() - 1])
            ));
        }
        Ok(pointer)
    }
}
impl Drop for NativeLibrary {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            FreeLibrary(self.handle);
        }
        #[cfg(not(windows))]
        unsafe {
            dlclose(self.handle);
        }
    }
}
fn platform_error() -> String {
    #[cfg(windows)]
    {
        std::io::Error::last_os_error().to_string()
    }
    #[cfg(not(windows))]
    unsafe {
        let value = dlerror();
        if value.is_null() {
            "unknown dynamic-library error".into()
        } else {
            CStr::from_ptr(value).to_string_lossy().into_owned()
        }
    }
}
