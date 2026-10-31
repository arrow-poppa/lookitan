// Freed memory goes back to the system on Linux as it does on Windows.
//
// Windows' heap hands big blocks to VirtualAlloc and releases them when they are freed, so the
// process shrinks when a model, a capture or a pass's buffers are dropped. glibc keeps freed memory
// for reuse instead: in a 64 MB heap per thread arena (up to 8 arenas per core), and it raises its
// mmap threshold up to 32 MB as big blocks are freed, so those come from the heaps too. Measured on
// the user's Fedora (2026-10-31): 86 such heaps holding 618 MB, and `malloc_trim(0)` took the
// process from 1.18 GB to 0.80 GB (PSS) with the models still loaded.
//
// ONNX Runtime is a C++ library on the same glibc malloc, so a Rust global allocator would not
// reach it; these settings do.

/// At start, before any other thread: blocks of 1 MB or more get their own mapping and are
/// unmapped when freed (as Windows' heap does past about 1 MB), and threads share two arenas.
/// False when glibc refuses one of them.
pub fn configure() -> bool {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // mallopt returns 0 on an error.
    return unsafe { libc::mallopt(libc::M_MMAP_THRESHOLD, 1 << 20) != 0 && libc::mallopt(libc::M_ARENA_MAX, 2) != 0 };
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    true
}

/// Gives the free pages of every arena back to the system: after lookups and after models are freed.
pub fn release() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        let before = resident_mb();
        unsafe { libc::malloc_trim(0) };
        log::debug!("malloc_trim: {before} → {} MB resident", resident_mb());
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn resident_mb() -> u64 {
    let pages = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .unwrap_or(0);
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;
    pages * page / (1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glibc_takes_the_settings_and_the_trim() {
        assert!(configure());
        release();
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        assert!(resident_mb() > 0);
    }
}
