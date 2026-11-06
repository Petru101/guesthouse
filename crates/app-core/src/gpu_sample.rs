//! The host card's load, power, clock and video memory, read about once a second on a thread of its
//! own, as a driver can take its time and may hang. A reading it hasn't got stays `None`: a caller
//! must show that as unknown, and never fill it in.

use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    /// When it was read, from the sampler's start.
    pub taken: Duration,
    pub load_percent: Option<u32>,
    pub power_watts: Option<f64>,
    pub clock_mhz: Option<u32>,
    pub memory_used_mib: Option<u64>,
    pub memory_total_mib: Option<u64>,
}

/// Something that can read a card. Vendors differ, so the viewer only knows this.
pub trait CardReader: Send {
    fn sample(&mut self) -> Sample;
}

/// Opens a reader on the sampler's own thread. `None` means there's nothing to read.
pub type OpenCard = Box<dyn FnOnce() -> Option<Box<dyn CardReader>> + Send>;

#[derive(Default)]
struct Shared {
    latest: Mutex<Option<Sample>>,
    stopping: Mutex<bool>,
    woken: Condvar,
}

pub struct Sampler {
    shared: Arc<Shared>,
    started: Instant,
}

impl Sampler {
    pub fn start(open: OpenCard, interval: Duration) -> Self {
        let shared = Arc::new(Shared::default());
        let started = Instant::now();
        let on_thread = shared.clone();
        thread::Builder::new()
            .name("host-gpu".to_owned())
            .spawn(move || {
                let Some(mut card) = open() else { return };
                loop {
                    let mut sample = card.sample();
                    sample.taken = started.elapsed();
                    // Asked under this lock, so a reading on its way can't land after `stop` has cleared it.
                    if let Ok(mut latest) = on_thread.latest.lock()
                        && !on_thread.stopping.lock().is_ok_and(|stopping| *stopping)
                    {
                        *latest = Some(sample);
                    }
                    let stopping = on_thread.stopping.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    let waited =
                        on_thread.woken.wait_timeout_while(stopping, interval, |stopping| !*stopping);
                    if waited.is_ok_and(|(stopping, _)| *stopping) {
                        return;
                    }
                }
            })
            .expect("a thread for the sampler");
        Self { shared, started }
    }

    pub fn now(&self) -> Duration {
        self.started.elapsed()
    }

    /// The newest reading: none until the first, and for good if the card can't be read.
    pub fn latest(&self) -> Option<Sample> {
        self.shared.latest.lock().ok()?.clone()
    }

    /// Doesn't wait for the thread: a hung driver must not take the window with it.
    pub fn stop(&self) {
        if let Ok(mut stopping) = self.shared.stopping.lock() {
            *stopping = true;
        }
        if let Ok(mut latest) = self.shared.latest.lock() {
            *latest = None;
        }
        self.shared.woken.notify_all();
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc;

    struct FakeCard(Arc<AtomicU32>, mpsc::Sender<()>);

    impl CardReader for FakeCard {
        fn sample(&mut self) -> Sample {
            self.0.fetch_add(1, Ordering::Relaxed);
            Sample { load_percent: Some(50), ..Sample::default() }
        }
    }

    impl Drop for FakeCard {
        fn drop(&mut self) {
            let _ = self.1.send(());
        }
    }

    #[test]
    fn a_stopped_sampler_has_nothing_current_and_leaves_no_thread() {
        let samples = Arc::new(AtomicU32::new(0));
        let (closed, was_closed) = mpsc::channel();
        let counted = samples.clone();
        let open: OpenCard = Box::new(move || Some(Box::new(FakeCard(counted, closed))));
        let sampler = Sampler::start(open, Duration::from_millis(1));
        while samples.load(Ordering::Relaxed) < 3 {
            thread::yield_now();
        }
        assert_eq!(sampler.latest().unwrap().load_percent, Some(50));
        sampler.stop();
        assert_eq!(sampler.latest(), None);
        was_closed.recv_timeout(Duration::from_secs(5)).expect("the thread ends and closes the card");
    }

    #[test]
    fn no_card_is_an_empty_reading_and_not_a_crash() {
        let sampler = Sampler::start(Box::new(|| None), SAMPLE_INTERVAL);
        thread::sleep(Duration::from_millis(20));
        assert_eq!(sampler.latest(), None);
    }
}
