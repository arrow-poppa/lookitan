// ONNX Runtime's environment is made under the "C" locale on Linux.
//
// GTK sets the process's locale from the desktop's (`setlocale(LC_ALL, "")`) when Tauri starts.
// With a locale that writes decimals with a comma (the user's pt_BR), an ONNX Runtime environment
// created under it made IndraOCR's LineNet compute wrong, with no error: the same input gave a core
// logit of −3.43 everywhere (no line found on any page) instead of 7.75. Measured on the user's
// Fedora (2026-10-31) with the bundled libonnxruntime.so through its C API: only the locale at
// `CreateEnv` matters (creating the session or running it under pt_BR is fine), and a thread-local
// "C" locale around `CreateEnv` alone fixes it. Python, the C test program and CI ran in the "C"
// locale, and Windows' C runtime stays in it, so only the app on Linux saw it.

/// Runs `f` with this thread in the "C" locale, then gives the thread back its locale. Other
/// threads and the process's locale are left alone.
pub fn in_c<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        struct Restore(libc::locale_t);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe { libc::uselocale(self.0) };
            }
        }
        // Not freed: what ONNX Runtime keeps of it is unknown, and it is made once per process.
        let c = unsafe { libc::newlocale(libc::LC_ALL_MASK, c"C".as_ptr(), std::ptr::null_mut()) };
        if c.is_null() {
            log::warn!("no \"C\" locale to create ONNX Runtime's environment in");
            return f();
        }
        let _restore = Restore(unsafe { libc::uselocale(c) });
        f()
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    f()
}

#[cfg(all(test, target_os = "linux", target_env = "gnu"))]
mod tests {
    use super::*;

    fn decimal_point() -> u8 {
        unsafe { *(*libc::localeconv()).decimal_point as u8 }
    }

    #[test]
    fn the_c_locale_holds_inside_and_the_thread_s_comes_back() {
        // A comma-decimal locale on this thread only; CI images may have none of them.
        let comma = ["pt_BR.UTF-8", "de_DE.UTF-8", "fr_FR.UTF-8", "ru_RU.UTF-8"].iter().find_map(|name| {
            let name = std::ffi::CString::new(*name).unwrap();
            let l = unsafe { libc::newlocale(libc::LC_ALL_MASK, name.as_ptr(), std::ptr::null_mut()) };
            (!l.is_null()).then_some(l)
        });
        let Some(comma) = comma else {
            eprintln!("no comma-decimal locale installed; only the C side is checked");
            assert_eq!(in_c(decimal_point), b'.');
            return;
        };
        let before = unsafe { libc::uselocale(comma) };
        assert_eq!(decimal_point(), b',');
        assert_eq!(in_c(decimal_point), b'.');
        assert_eq!(decimal_point(), b',');
        unsafe {
            libc::uselocale(before);
            libc::freelocale(comma);
        }
    }
}
