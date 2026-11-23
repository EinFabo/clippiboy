//! Wakes the engine the moment an application opens an audio session.
//!
//! With application sources active every output device is recorded as the
//! leftovers, one tap per remaining application, rebuilt on the two-second
//! tick. An application that starts playing between two ticks would miss up to
//! two seconds. Windows can say when a session appears
//! (`IAudioSessionNotification`); this module turns that into "refresh now".
//! The tick stays as the fallback.
//!
//! The COM callback only sends a message. Everything else — debouncing,
//! enumerating, setting up captures — happens on the watcher thread or on the
//! engine's own refresh thread, never inside the callback.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Sessions created together (an app opening several) share one refresh.
pub const DEBOUNCE: Duration = Duration::from_millis(150);
/// A session is often created a moment before it starts rendering, and the
/// refresh only counts sessions that play — so one more refresh follows, this
/// long after the last session created.
pub const FOLLOW_UP: Duration = Duration::from_millis(700);
/// How long a device that could not be watched waits before it is tried again
/// — the two-second tick covers it meanwhile.
pub const RETRY: Duration = Duration::from_secs(30);

/// Decides when the watcher thread fires a refresh. Pure, so it can be tested.
#[derive(Default)]
pub struct Coalescer {
    last_fire: Option<Instant>,
    due: Option<Instant>,
    follow_up: Option<Instant>,
}

impl Coalescer {
    /// A session was created.
    pub fn trigger(&mut self, now: Instant) {
        if self.due.is_none() {
            let earliest = self.last_fire.map_or(now, |last| last + DEBOUNCE);
            self.due = Some(earliest.max(now));
        }
        // Re-armed each time: the follow-up belongs to the last session of a
        // burst, not to the first, which may long have been counted.
        self.follow_up = Some(now + FOLLOW_UP);
    }

    /// Should a refresh run now? Consumes what is due.
    pub fn poll(&mut self, now: Instant) -> bool {
        let mut fire = false;
        if self.due.is_some_and(|due| due <= now) {
            self.due = None;
            fire = true;
        }
        if self.follow_up.is_some_and(|at| at <= now) {
            self.follow_up = None;
            fire = true;
        }
        if fire {
            // Whatever else is pending would only repeat this refresh.
            if let Some(due) = self.due {
                if due < now + DEBOUNCE {
                    self.due = Some(now + DEBOUNCE);
                }
            }
            self.last_fire = Some(now);
        }
        fire
    }

    /// When the thread has to look again, if anything is pending.
    pub fn next_deadline(&self) -> Option<Instant> {
        match (self.due, self.follow_up) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// When failed device registrations are tried again. Pure, so it can be tested.
#[derive(Default)]
pub struct Retry {
    at: Option<Instant>,
}

impl Retry {
    /// After registering: everything wanted is watched (`complete`), or a
    /// device is missing and gets another go after [`RETRY`].
    pub fn after(&mut self, complete: bool, now: Instant) {
        self.at = (!complete).then(|| now + RETRY);
    }

    /// Is a retry due? Consumes it; [`Retry::after`] sets the next one.
    pub fn due(&mut self, now: Instant) -> bool {
        if self.at.is_some_and(|at| at <= now) {
            self.at = None;
            return true;
        }
        false
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.at
    }
}

enum Cmd {
    /// The output devices to watch (ids); replaces the previous set.
    Watch(Vec<String>),
    /// A session was created on a watched device.
    Created,
    Stop,
}

/// One thread that holds the registrations. Dropping it unregisters them.
pub struct Watcher {
    tx: Sender<Cmd>,
    devices: Vec<String>,
}

impl Watcher {
    /// `fire` runs on the watcher thread when a refresh should happen. It must
    /// return quickly (the engine's refresh hands off to a thread of its own).
    pub fn start(fire: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let own = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("session-watch".into())
            .spawn(move || run(rx, own, fire));
        if let Err(e) = spawned {
            log::warn!("session watcher thread did not start: {e}");
        }
        Self {
            tx,
            devices: Vec::new(),
        }
    }

    /// Watch exactly these output devices (empty: none). Cheap when unchanged.
    pub fn set_devices(&mut self, mut devices: Vec<String>) {
        devices.sort();
        devices.dedup();
        if devices == self.devices {
            return;
        }
        self.devices = devices.clone();
        let _ = self.tx.send(Cmd::Watch(devices));
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

fn run(rx: mpsc::Receiver<Cmd>, tx: Sender<Cmd>, fire: impl Fn()) {
    let _com = platform::ComGuard::new();
    let mut registrations = platform::Registrations::new(tx);
    let mut coalescer = Coalescer::default();
    let mut retry = Retry::default();
    loop {
        let wait = [coalescer.next_deadline(), retry.next_deadline()]
            .into_iter()
            .flatten()
            .min()
            .map_or(Duration::from_secs(3600), |at| {
                at.saturating_duration_since(Instant::now())
            });
        match rx.recv_timeout(wait) {
            Ok(Cmd::Watch(devices)) => {
                let complete = registrations.set(&devices);
                retry.after(complete, Instant::now());
            }
            Ok(Cmd::Created) => coalescer.trigger(Instant::now()),
            Ok(Cmd::Stop) => break,
            // Never while this loop runs: `registrations` keeps a sender of its
            // own (`own` in `Watcher::start`), so the channel cannot lose its
            // last one — dropping the `Watcher` sends `Stop` instead. Kept for
            // completeness, and as a way out should that ever change.
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if retry.due(Instant::now()) {
            let complete = registrations.retry();
            retry.after(complete, Instant::now());
        }
        if coalescer.poll(Instant::now()) {
            fire();
        }
    }
    registrations.clear();
}

#[cfg(windows)]
mod platform {
    use super::Cmd;
    use std::sync::mpsc::Sender;
    use windows::core::implement;
    use windows::Win32::Media::Audio::{
        IAudioSessionControl, IAudioSessionManager2, IAudioSessionNotification,
        IAudioSessionNotification_Impl, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    /// The session manager has to be used from the multithreaded apartment.
    pub struct ComGuard(bool);

    impl ComGuard {
        pub fn new() -> Self {
            Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() })
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    #[implement(IAudioSessionNotification)]
    struct Notify {
        tx: Sender<Cmd>,
    }

    impl IAudioSessionNotification_Impl for Notify_Impl {
        // Only signals. Windows calls this on one of its own threads, and the
        // docs forbid blocking or re-entering the audio API from here.
        fn OnSessionCreated(
            &self,
            _newsession: Option<&IAudioSessionControl>,
        ) -> windows::core::Result<()> {
            let _ = self.tx.send(Cmd::Created);
            Ok(())
        }
    }

    struct Registration {
        device: String,
        manager: IAudioSessionManager2,
        notify: IAudioSessionNotification,
    }

    pub struct Registrations {
        tx: Sender<Cmd>,
        wanted: Vec<String>,
        active: Vec<Registration>,
    }

    impl Registrations {
        pub fn new(tx: Sender<Cmd>) -> Self {
            Self {
                tx,
                wanted: Vec::new(),
                active: Vec::new(),
            }
        }

        /// Drop what is no longer wanted, register what is new. A device that
        /// fails stays out until [`Registrations::retry`] or the next change.
        /// True when every wanted device is watched.
        pub fn set(&mut self, devices: &[String]) -> bool {
            self.wanted = devices.to_vec();
            let (keep, drop): (Vec<_>, Vec<_>) = std::mem::take(&mut self.active)
                .into_iter()
                .partition(|r| devices.contains(&r.device));
            for r in drop {
                unregister(&r);
            }
            self.active = keep;
            self.register_missing(true)
        }

        /// The wanted devices that failed before, once more. Logged only when
        /// one now works — a device that stays broken said so the first time.
        pub fn retry(&mut self) -> bool {
            self.register_missing(false)
        }

        fn register_missing(&mut self, first: bool) -> bool {
            let mut complete = true;
            for id in &self.wanted {
                if self.active.iter().any(|r| &r.device == id) {
                    continue;
                }
                match self.register(id) {
                    Ok(r) => {
                        if !first {
                            log::info!("session notifications on '{id}' after a retry");
                        }
                        self.active.push(r);
                    }
                    Err(e) => {
                        complete = false;
                        if first {
                            log::warn!("no session notifications on '{id}': {e}");
                        }
                    }
                }
            }
            complete
        }

        fn register(&self, id: &str) -> windows::core::Result<Registration> {
            unsafe {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
                let device = enumerator.GetDevice(windows::core::PCWSTR(wide.as_ptr()))?;
                let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
                // Notifications only start once the sessions were enumerated.
                let _ = manager.GetSessionEnumerator()?;
                let notify: IAudioSessionNotification = Notify {
                    tx: self.tx.clone(),
                }
                .into();
                manager.RegisterSessionNotification(&notify)?;
                Ok(Registration {
                    device: id.to_string(),
                    manager,
                    notify,
                })
            }
        }

        pub fn clear(&mut self) {
            for r in self.active.drain(..) {
                unregister(&r);
            }
        }
    }

    fn unregister(r: &Registration) {
        if let Err(e) = unsafe { r.manager.UnregisterSessionNotification(&r.notify) } {
            log::warn!(
                "session notifications on '{}' not unregistered: {e}",
                r.device
            );
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Cmd;
    use std::sync::mpsc::Sender;

    pub struct ComGuard;
    impl ComGuard {
        pub fn new() -> Self {
            Self
        }
    }

    pub struct Registrations;
    impl Registrations {
        pub fn new(_tx: Sender<Cmd>) -> Self {
            Self
        }
        pub fn set(&mut self, _devices: &[String]) -> bool {
            true
        }
        pub fn retry(&mut self) -> bool {
            true
        }
        pub fn clear(&mut self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn a_first_session_fires_at_once() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        c.trigger(t0);
        assert!(c.poll(t0));
        assert!(!c.poll(at(t0, 10)));
    }

    #[test]
    fn a_burst_shares_one_refresh_after_the_debounce() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        c.trigger(t0);
        assert!(c.poll(t0));
        // Three more within the window: one refresh, not three.
        c.trigger(at(t0, 20));
        c.trigger(at(t0, 40));
        c.trigger(at(t0, 60));
        assert!(!c.poll(at(t0, 100)));
        assert!(c.poll(at(t0, 150)));
        assert!(!c.poll(at(t0, 160)));
    }

    #[test]
    fn a_follow_up_refresh_comes_once() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        c.trigger(t0);
        assert!(c.poll(t0));
        assert!(!c.poll(at(t0, 699)));
        assert!(c.poll(at(t0, 700)));
        assert!(!c.poll(at(t0, 5000)));
        assert_eq!(c.next_deadline(), None);
    }

    /// Sessions that keep coming: the follow-up waits for the last of them.
    #[test]
    fn the_follow_up_is_re_armed_by_each_session() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        c.trigger(t0);
        assert!(c.poll(t0));
        c.trigger(at(t0, 500));
        assert!(c.poll(at(t0, 500)));
        // Not 700 ms after the first session, 700 ms after the last.
        assert!(!c.poll(at(t0, 700)));
        assert_eq!(c.next_deadline(), Some(at(t0, 1200)));
        assert!(c.poll(at(t0, 1200)));
        assert_eq!(c.next_deadline(), None);
    }

    #[test]
    fn a_device_that_could_not_be_watched_is_tried_again_slowly() {
        let t0 = Instant::now();
        let mut r = Retry::default();
        r.after(true, t0);
        assert_eq!(r.next_deadline(), None);
        assert!(!r.due(t0 + RETRY));
        r.after(false, t0);
        assert_eq!(r.next_deadline(), Some(t0 + RETRY));
        assert!(!r.due(t0 + RETRY - Duration::from_millis(1)));
        assert!(r.due(t0 + RETRY));
        // Consumed: only the next result arms it again.
        assert!(!r.due(t0 + RETRY * 2));
        r.after(false, t0 + RETRY);
        assert_eq!(r.next_deadline(), Some(t0 + RETRY * 2));
        r.after(true, t0 + RETRY * 2);
        assert_eq!(r.next_deadline(), None);
    }

    #[test]
    fn nothing_pending_means_no_deadline() {
        let c = Coalescer::default();
        assert_eq!(c.next_deadline(), None);
    }

    #[test]
    fn a_late_trigger_waits_out_the_debounce_since_the_last_refresh() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        c.trigger(t0);
        assert!(c.poll(t0));
        c.trigger(at(t0, 100));
        assert_eq!(c.next_deadline(), Some(at(t0, 150)));
    }
}
