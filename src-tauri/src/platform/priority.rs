// A lookup ahead of a game for the CPU, as Lookupper does (REA: `0x140190ae0` raises its priority
// when a lookup starts, `0x140190b90` gives it back).
//
// Windows: while a lookup runs, the process is in HIGH_PRIORITY_CLASS and out of the power
// throttling (EcoQoS) Windows 11 puts a background app in when a game has the foreground: the same
// `SetPriorityClass` and `SetProcessInformation(ProcessPowerThrottling)` calls, with the same
// masks, and both are given back after it.
//
// Linux, without root, in two ways:
// - While a lookup runs, the whole process (its web views included) gets the highest CPU weight in
//   its cgroup, as Lookupper raises its whole process: the systemd user manager sets `CPUWeight`
//   on the unit the desktop started Lookitan in (KDE starts each app in an `app-…` scope), and
//   sets the earlier weight back after it. A weight only counts while processes compete for the
//   CPU, and the D-Bus calls run on a thread of their own, so a lookup never waits for them. A
//   Lookitan started from a terminal shares the terminal's unit, which is left alone.
// - rtkit (RealtimeKit, the D-Bus service PipeWire uses, allowed for the active session by polkit)
//   gives a high priority thread (nice -10, which a user cannot set alone) to the lookup's own
//   thread and to the threads that create the OCR sessions (CPU, and WebGPU on the GPU, whose
//   Dawn threads are made then); their worker threads inherit it. rtkit rate-limits its requests,
//   so those threads keep it; they only use the CPU during a lookup.
// Without systemd or rtkit, nothing changes.

use parking_lot::Mutex;

/// Lookups running now; the first raises the priority and the last gives it back.
static RUNNING: Mutex<usize> = Mutex::new(0);

/// A running lookup: the priority stays raised until it is dropped.
pub struct Lookup(());

/// Raises the priority for a lookup.
pub fn lookup() -> Lookup {
    let mut running = RUNNING.lock();
    if *running == 0 {
        #[cfg(any(windows, target_os = "linux"))]
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
            #[cfg(any(windows, target_os = "linux"))]
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
    use parking_lot::Mutex;
    use std::sync::mpsc::{channel, Sender};
    use std::sync::OnceLock;
    use zbus::zvariant::{OwnedValue, Value};

    /// The nice level asked of rtkit, within its default floor (MinNiceLevel) of -15.
    const NICE: i32 = -10;

    /// The CPU weight of Lookitan's unit during a lookup: systemd's highest.
    const LOOKUP_WEIGHT: u64 = 10_000;
    /// systemd's weight for a unit that sets none.
    const DEFAULT_WEIGHT: u64 = 100;

    /// The unit Lookitan runs in, from `/proc/self/cgroup` (cgroup v2: "0::/…/app.slice/<unit>"),
    /// when the desktop made it for Lookitan.
    pub(super) fn own_unit(cgroup: &str) -> Option<String> {
        let path = cgroup.lines().find_map(|l| l.strip_prefix("0::"))?;
        let unit = path.rsplit('/').next()?;
        let made = unit.ends_with(".scope") || unit.ends_with(".service");
        (made && unit.to_lowercase().contains("lookitan")).then(|| unit.to_string())
    }

    /// The thread that sets the unit's weight, in the order asked.
    fn weights() -> Option<&'static Mutex<Sender<bool>>> {
        static SENDER: OnceLock<Option<Mutex<Sender<bool>>>> = OnceLock::new();
        SENDER
            .get_or_init(|| {
                let unit = own_unit(&std::fs::read_to_string("/proc/self/cgroup").ok()?)?;
                let (tx, rx) = channel::<bool>();
                std::thread::Builder::new()
                    .name("cpu-weight".into())
                    .spawn(move || {
                        let Ok(bus) = zbus::blocking::Connection::session() else { return };
                        let earlier = weight(&bus, &unit).unwrap_or(DEFAULT_WEIGHT);
                        for raise in rx {
                            set_weight(&bus, &unit, if raise { LOOKUP_WEIGHT } else { earlier });
                        }
                    })
                    .ok()?;
                Some(Mutex::new(tx))
            })
            .as_ref()
    }

    /// The unit's CPU weight now; none set (systemd's "infinity") reads as the default.
    fn weight(bus: &zbus::blocking::Connection, unit: &str) -> Option<u64> {
        let reply = bus
            .call_method(Some("org.freedesktop.systemd1"), "/org/freedesktop/systemd1", Some("org.freedesktop.systemd1.Manager"), "GetUnit", &(unit,))
            .ok()?;
        let path: zbus::zvariant::OwnedObjectPath = reply.body().deserialize().ok()?;
        let iface = if unit.ends_with(".scope") { "org.freedesktop.systemd1.Scope" } else { "org.freedesktop.systemd1.Service" };
        let reply = bus
            .call_method(Some("org.freedesktop.systemd1"), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &(iface, "CPUWeight"))
            .ok()?;
        let value: OwnedValue = reply.body().deserialize().ok()?;
        let weight = u64::try_from(&value).ok()?;
        Some(if (1..=10_000).contains(&weight) { weight } else { DEFAULT_WEIGHT })
    }

    fn set_weight(bus: &zbus::blocking::Connection, unit: &str, weight: u64) {
        let properties = vec![("CPUWeight", Value::from(weight))];
        let set = bus.call_method(
            Some("org.freedesktop.systemd1"),
            "/org/freedesktop/systemd1",
            Some("org.freedesktop.systemd1.Manager"),
            "SetUnitProperties",
            &(unit, true, properties),
        );
        if let Err(e) = set {
            log::debug!("systemd did not set the CPU weight of {unit}: {e}");
        }
    }

    pub fn raise() {
        if let Some(weights) = weights() {
            let _ = weights.lock().send(true);
        }
    }

    pub fn restore() {
        if let Some(weights) = weights() {
            let _ = weights.lock().send(false);
        }
    }

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

    #[cfg(target_os = "linux")]
    #[test]
    fn only_a_unit_made_for_lookitan_gets_its_weight_raised() {
        let app = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-io.github.arrowpoppa.Lookitan@4f2c.service
";
        assert_eq!(imp::own_unit(app).as_deref(), Some("app-io.github.arrowpoppa.Lookitan@4f2c.service"));
        let scope = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-lookitan-81726.scope
";
        assert_eq!(imp::own_unit(scope).as_deref(), Some("app-lookitan-81726.scope"));
        let terminal = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.kde.konsole-2231.scope
";
        assert_eq!(imp::own_unit(terminal), None, "a terminal's unit is left alone");
        assert_eq!(imp::own_unit("12:cpu:/
"), None, "cgroup v1");
    }

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
