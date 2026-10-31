// A model's loaded sessions (comic-text-detector, IndraOCR's LineNet and recognizer), by what they
// were loaded for (`K`: IndraOCR's recognizer group) and the device asked for. With a dedicated card
// (`both`) a CPU session and a GPU one stay side by side, the CPU's in RAM and the GPU's in the
// card's own memory, so selectors on different devices do not reload the model in turn. With an
// integrated card, whose memory is the system's RAM, only the last one loaded stays (2026-10-30, at
// the user's request; `gpu_kind`).

use std::sync::Arc;

pub struct DeviceSessions<T, K = ()> {
    kept: Vec<(K, bool, Arc<T>)>,
}

impl<T, K> Default for DeviceSessions<T, K> {
    fn default() -> Self {
        Self { kept: Vec::new() }
    }
}

impl<T, K: Copy + PartialEq> DeviceSessions<T, K> {
    /// The session loaded for `key` on the device asked for (`gpu`).
    pub fn get(&self, key: K, gpu: bool) -> Option<Arc<T>> {
        self.kept.iter().find(|(k, g, _)| *k == key && *g == gpu).map(|(_, _, s)| s.clone())
    }

    /// Whether a session for `key` is loaded on the other device than `gpu`: only then does keeping
    /// both matter.
    pub fn holds_other(&self, key: K, gpu: bool) -> bool {
        self.kept.iter().any(|(k, g, _)| *k == key && *g != gpu)
    }

    /// Drops what a new session for `key` on `gpu` replaces, before it loads: another key's, the same
    /// device's, and without `both` the other device's too.
    pub fn make_room(&mut self, key: K, gpu: bool, both: bool) {
        self.kept.retain(|(k, g, _)| both && *k == key && *g != gpu);
    }

    pub fn insert(&mut self, key: K, gpu: bool, session: Arc<T>) {
        self.kept.push((key, gpu, session));
    }

    /// Keeps only the sessions on a device the engines in use ask for.
    pub fn keep_devices(&mut self, devices: &[bool]) {
        self.kept.retain(|(_, g, _)| devices.contains(g));
    }

    pub fn clear(&mut self) {
        self.kept.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.kept.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.kept.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dedicated_card_keeps_a_cpu_and_a_gpu_session_an_integrated_one_only_the_last() {
        let mut dedicated: DeviceSessions<&str> = DeviceSessions::default();
        dedicated.make_room((), false, true);
        dedicated.insert((), false, Arc::new("cpu"));
        assert!(dedicated.holds_other((), true) && !dedicated.holds_other((), false));
        dedicated.make_room((), true, true);
        dedicated.insert((), true, Arc::new("gpu"));
        assert_eq!((dedicated.get((), false).as_deref(), dedicated.get((), true).as_deref()), (Some(&"cpu"), Some(&"gpu")));
        // A selector moved off the GPU: its session goes.
        dedicated.keep_devices(&[false]);
        assert_eq!((dedicated.len(), dedicated.get((), true)), (1, None));
        dedicated.keep_devices(&[]);
        assert_eq!(dedicated.len(), 0, "no selector uses it");

        let mut integrated: DeviceSessions<&str> = DeviceSessions::default();
        integrated.make_room((), false, false);
        integrated.insert((), false, Arc::new("cpu"));
        integrated.make_room((), true, false);
        integrated.insert((), true, Arc::new("gpu"));
        assert_eq!((integrated.len(), integrated.get((), false)), (1, None), "the CPU's went before the GPU's loaded");
    }

    #[test]
    fn another_key_replaces_every_session_of_the_old_one() {
        let mut groups: DeviceSessions<&str, u8> = DeviceSessions::default();
        groups.insert(1, false, Arc::new("a cpu"));
        groups.insert(1, true, Arc::new("a gpu"));
        groups.make_room(2, true, true);
        assert_eq!(groups.len(), 0);
        groups.insert(2, true, Arc::new("b gpu"));
        assert_eq!(groups.get(2, true).as_deref(), Some(&"b gpu"));
        assert_eq!(groups.get(1, true), None);
    }
}
