use std::fs::File;
use std::os::fd::AsRawFd;
use std::ptr;
use std::slice;

pub struct MmapRegion {
    pub ptr: *mut u8,
    pub len: usize,
}

unsafe impl Send for MmapRegion {}
unsafe impl Sync for MmapRegion {}

impl MmapRegion {
    pub fn open(path: &str) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len() as usize;
        if len == 0 {
            return Err("empty file".to_string());
        }
        unsafe {
            let mut flags = libc::MAP_PRIVATE;
            #[cfg(target_os = "linux")]
            {
                flags |= libc::MAP_POPULATE;
            }

            let ptr = libc::mmap(
                ptr::null_mut(),
                len,
                libc::PROT_READ,
                flags,
                file.as_raw_fd(),
                0,
            );
            if ptr == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error().to_string());
            }
            advise_mapping(ptr, len);
            Ok(Self {
                ptr: ptr.cast::<u8>(),
                len,
            })
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr.cast_const(), self.len) }
    }
}

#[cfg(target_os = "linux")]
unsafe fn advise_mapping(ptr: *mut libc::c_void, len: usize) {
    unsafe {
        libc::madvise(ptr, len, libc::MADV_WILLNEED);
        libc::madvise(ptr, len, libc::MADV_HUGEPAGE);
    }
}

#[cfg(not(target_os = "linux"))]
unsafe fn advise_mapping(_ptr: *mut libc::c_void, _len: usize) {}

impl Drop for MmapRegion {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr.cast::<libc::c_void>(), self.len);
        }
    }
}
