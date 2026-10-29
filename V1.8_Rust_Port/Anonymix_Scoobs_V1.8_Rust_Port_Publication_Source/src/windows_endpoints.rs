//! Minimal MMDevice COM calls used by the original GetDefaultEndpointId helper.
//! Call only on the COM-initialized PortAudio owner thread, never in callbacks.
use crate::device_policy::EndpointIds;
use std::{ffi::c_void, marker::PhantomData, ptr, rc::Rc};

#[repr(C)]
struct Guid {
    first: u32,
    second: u16,
    third: u16,
    tail: [u8; 8],
}
const ENUMERATOR_CLASS: Guid = Guid {
    first: 0xbcde0395,
    second: 0xe52f,
    third: 0x467c,
    tail: [0x8e, 0x3d, 0xc4, 0x57, 0x92, 0x91, 0x69, 0x2e],
};
const ENUMERATOR_INTERFACE: Guid = Guid {
    first: 0xa95664d2,
    second: 0x9614,
    third: 0x4f35,
    tail: [0xa7, 0x46, 0xde, 0x8d, 0xb6, 0x36, 0x17, 0xe6],
};

#[link(name = "ole32")]
extern "system" {
    fn CoCreateInstance(
        class: *const Guid,
        outer: *mut c_void,
        context: u32,
        interface: *const Guid,
        output: *mut *mut c_void,
    ) -> i32;
    fn CoTaskMemFree(memory: *mut c_void);
}

// COM interface pointers begin with a vtable pointer. These slots follow the
// Windows SDK's IUnknown / IMMDeviceEnumerator / IMMDevice declaration order.
struct ComPointer {
    raw: *mut c_void,
    _same_thread: PhantomData<Rc<()>>,
}
impl ComPointer {
    unsafe fn slot(&self, index: usize) -> *const c_void {
        // SAFETY: created with the documented IID; callers use only SDK slots
        // present in that interface while this counted reference remains alive.
        unsafe { *(*(self.raw as *const *const *const c_void)).add(index) }
    }
}
impl Drop for ComPointer {
    fn drop(&mut self) {
        let release: unsafe extern "system" fn(*mut c_void) -> u32 =
            unsafe { std::mem::transmute(self.slot(2)) };
        unsafe {
            release(self.raw);
        }
    }
}
struct TaskString(*mut u16);
impl Drop for TaskString {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(self.0.cast());
        }
    }
}

/// Query one multimedia default endpoint on the COM owner thread, copying its opaque UTF-16 ID.
/// Failure returns None so callers conservatively refresh enumeration instead of trusting a stale index.
pub fn default_endpoint_id(capture: bool) -> Option<Vec<u16>> {
    let mut raw = ptr::null_mut();
    // SAFETY: correct SDK GUIDs, CLSCTX_INPROC_SERVER, null aggregation pointer,
    // live out pointer and a COM apartment already initialized by the owner.
    let result = unsafe {
        CoCreateInstance(
            &ENUMERATOR_CLASS,
            ptr::null_mut(),
            1,
            &ENUMERATOR_INTERFACE,
            &mut raw,
        )
    };
    if result < 0 || raw.is_null() {
        return None;
    }
    let enumerator = ComPointer {
        raw,
        _same_thread: PhantomData,
    };
    let get_default: unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> i32 =
        unsafe { std::mem::transmute(enumerator.slot(4)) };
    let mut raw_device = ptr::null_mut();
    // eRender=0, eCapture=1, eMultimedia=1, matching Audio.cpp.
    let result = unsafe {
        get_default(
            enumerator.raw,
            if capture { 1 } else { 0 },
            1,
            &mut raw_device,
        )
    };
    drop(enumerator);
    if result < 0 || raw_device.is_null() {
        return None;
    }
    let device = ComPointer {
        raw: raw_device,
        _same_thread: PhantomData,
    };
    let get_id: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> i32 =
        unsafe { std::mem::transmute(device.slot(5)) };
    let mut raw_id = ptr::null_mut();
    let result = unsafe { get_id(device.raw, &mut raw_id) };
    drop(device);
    let id = TaskString(raw_id);
    if result < 0 || id.0.is_null() {
        return None;
    }
    let mut length = 0;
    // SAFETY: GetId returns a valid NUL-terminated UTF-16 task allocation until
    // CoTaskMemFree. Preserve its opaque code units; do not parse the ID.
    unsafe {
        while *id.0.add(length) != 0 {
            length += 1;
        }
        if length == 0 {
            None
        } else {
            Some(std::slice::from_raw_parts(id.0, length).to_vec())
        }
    }
}

/// Read capture and render endpoint identities using the same owner-thread COM apartment.
pub fn default_endpoint_ids() -> EndpointIds {
    EndpointIds {
        capture: default_endpoint_id(true),
        render: default_endpoint_id(false),
    }
}
