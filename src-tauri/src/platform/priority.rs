// A lookup ahead of a game for the CPU, as Lookupper does (REA: `0x140190ae0` raises its priority
// when a lookup starts, `0x140190b90` gives it back).
//
// Windows: while a lookup runs, the process is in HIGH_PRIORITY_CLASS and out of the power
// throttling (EcoQoS) Windows 11 puts a background app in when a game has the foreground: the same
// `SetPriorityClass` and `SetProcessInformation(ProcessPowerThrottling)` calls, with the same
// masks, and both are given back after it.
//
// Linux: a user cannot lower a nice value without root, so the lookup asks rtkit (RealtimeKit, the
// D-Bus service PipeWire uses, allowed for the active session by polkit) for a high priority
// thread: the lookup's own thread, and the threads that create the OCR sessions (CPU, and WebGPU
// on the GPU, whose Dawn threads are made then), whose worker threads inherit their nice value. rtkit rate-limits its requests, so those threads keep it; they
// only use the CPU during a lookup. Without rtkit, nothing changes.

use parking_lot::Mutex;

/// Lookups running now; the first raises the priority and the last gives it back.
static RUNNING: Mutex<usize> = Mutex::new(0);

/// A running lookup: the priority stays raised until it is dropped.
pub struct Lookup(());

/// Raises the priority for a lookup.
pub fn lookup() -> Lookup {
    let mut running = RUNNING.lock();
    if *running == 0 {
        #[cfg(windows)]
        imp::raise();
    }
    *running += 1;
    raise_this_thread();
    Lookup(())
}

impl Drop for Lookup {
    fn drop(&mut self) {
        let mut running = RUNNING.lock();
        *running -= 1;
        if *running == 0 {
            #[cfg(windows)]
            imp::restore();
        }
    }
}

/// Gives the calling thread, and the threads it then creates, a high priority where that is per
/// thread (Linux, through rtkit). On Windows the lookup raises the whole process instead.
pub fn raise_this_thread() {
    #[cfg(target_os = "linux")]
    imp::raise_this_thread();
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetPriorityClass, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation, HIGH_PRIORITY_CLASS,
        NORMAL_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
    };

    /// The priority class before the lookup.
    static SAVED: AtomicU32 = AtomicU32::new(NORMAL_PRIORITY_CLASS);

    /// Sets which power throttling the process controls itself; `StateMask` 0 turns each of them
    /// off. A `control` of 0 gives the decision back to Windows.
    fn throttling(control: u32) {
        let state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: control, StateMask: 0 };
        unsafe {
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                &state as *const PROCESS_POWER_THROTTLING_STATE as *const core::ffi::c_void,
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            );
        }
    }

    pub fn raise() {
        unsafe {
            let process = GetCurrentProcess();
            let class = GetPriorityClass(process);
            if class != 0 && class != HIGH_PRIORITY_CLASS {
                SAVED.store(class, Ordering::SeqCst);
            }
            SetPriorityClass(process, HIGH_PRIORITY_CLASS);
        }
        throttling(PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION);
    }

    pub fn restore() {
        unsafe {
            SetPriorityClass(GetCurrentProcess(), SAVED.load(Ordering::SeqCst));
        }
        throttling(0);
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::OnceLock;

    /// The nice level asked of rtkit, within its default floor (MinNiceLevel) of -15.
    const NICE: i32 = -10;

    static BUS: OnceLock<Option<zbus::blocking::Connection>> = OnceLock::new();

    /// The calling thread's id, from `/proc/thread-self` ("<pid>/task/<tid>").
    fn thread_id() -> Option<u64> {
        std::fs::read_link("/proc/thread-self").ok()?.file_name()?.to_str()?.parse().ok()
    }

    pub fn raise_this_thread() {
        let Some(tid) = thread_id() else { return };
        let Some(bus) = BUS.get_or_init(|| zbus::blocking::Connection::system().ok()).as_ref() else { return };
        let asked = bus.call_method(
            Some("org.freedesktop.RealtimeKit1"),
            "/org/freedesktop/RealtimeKit1",
            Some("org.freedesktop.RealtimeKit1"),
            "MakeThreadHighPriority",
            &(tid, NICE),
        );
        if let Err(e) = asked {
            log::debug!("rtkit did not raise thread {tid}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lookup_gives_the_priority_back_after_the_last_of_them() {
        #[cfg(windows)]
        let class = || unsafe {
            use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetPriorityClass};
            GetPriorityClass(GetCurrentProcess())
        };
        #[cfg(windows)]
        let before = class();
        let first = lookup();
        let second = lookup();
        #[cfg(windows)]
        assert_eq!(class(), windows_sys::Win32::System::Threading::HIGH_PRIORITY_CLASS);
        drop(first);
        #[cfg(windows)]
        assert_eq!(class(), windows_sys::Win32::System::Threading::HIGH_PRIORITY_CLASS, "still raised for the second lookup");
        drop(second);
        #[cfg(windows)]
        assert_eq!(class(), before);
        assert_eq!(*RUNNING.lock(), 0);
    }
}
