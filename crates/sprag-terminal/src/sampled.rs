//! Holding a fact the daemon can only SAMPLE, and answering it with the age it actually has.
//!
//! Some of what a client wants to know is not the daemon's to publish. The registry's structure —
//! sessions, windows, panes, the arrangement — changes only when the daemon performs the change, so
//! it is announced and the scene revision carries it. The operating system's answers are the other
//! kind: a working directory changes when somebody types `cd`, a listening port when a server
//! binds, a pane's foreground job when a shell hands its terminal to `cargo`. The daemon sees bytes,
//! not events. Those facts have to be looked up, they cost the filesystem and `/proc` to look up,
//! and every answer is a reading taken at some instant rather than a state.
//!
//! R282 measured what happens when the two kinds are served together: a display client re-reads the
//! session list on every poll wake, and a wake is a batch of PTY output — so a character arriving in
//! any pane cost a `/proc` walk of the whole box, per attached client, for three facts a printed
//! character says nothing about. **1.257 us** for the list with no live pane against **3478.178 us**
//! with one.
//!
//! # The shape that fixes it, and why it lives here rather than in one fact's module
//!
//! A sample is [ASKED FOR](Sampled::read) with the staleness the caller will accept, and answered
//! with the age it has:
//!
//! * within the caller's tolerance, the held sample is cloned — no filesystem at all;
//! * otherwise ONE fresh sample is taken while every other asker waits on it, so N concurrent
//!   readers cost one walk rather than N;
//! * with nobody asking, nothing is sampled. The idle cost is zero, which no timer-driven refresher
//!   can say.
//!
//! None of that is about sessions, and R290 needed the identical thing for a pane's foreground job.
//! A second copy would be a second answer to "how stale may this be" — the kind of duplication that
//! only shows up as two facts drifting to two cadences. So the machinery is generic and the two
//! named samplers are its users; what stays with each fact is the part that IS about that fact, its
//! sampling function.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A held sample and the instant it was taken — a sampler's whole state.
struct Held<T> {
    taken: Instant,
    value: T,
}

/// One reading of a sampled fact, and how old it is.
///
/// The age travels WITH the value rather than being left for the reader to assume. A sampled fact
/// read without its age is a fact whose freshness the caller has to guess at, and the guess is
/// wrong exactly when it matters — a `ports` list that predates the server somebody just started,
/// or a foreground job that predates the build somebody just launched, looks identical to one that
/// does not.
///
/// ONE age for the whole reading, never one per row: a reading is one pass, so no part of it is
/// fresher than another and a per-row age would invite a reader to believe otherwise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading<T> {
    /// How long ago [`value`](Self::value) was sampled. Zero for a sample taken to answer this very
    /// read.
    pub age: Duration,
    /// What was sampled.
    pub value: T,
}

/// A fact sampled from the world, held between reads, and shared by every reader that asks for it.
///
/// Generic over WHAT is sampled and deliberately NOT over how: the sampling function is supplied at
/// each [`read`](Self::read), which is what lets a named sampler own the question (and its locking
/// order) while this owns only the cache and the coalescing. A caller that reached for this type
/// directly could hand two different functions to one holder and cache two different facts in one
/// slot, so the named wrappers — [`ActivitySampler`](crate::ActivitySampler) is the first — are
/// the public way in.
///
/// # Locking
///
/// The lock is taken FIRST and held ACROSS the sample. That is not an oversight to be optimised
/// away later: it IS the coalescing. A second reader arriving mid-sample waits, then finds a value
/// fresh enough for any tolerance it could have declared, and pays nothing. The consequence a
/// caller must respect is that whatever locks the sampling function takes are taken UNDER this one,
/// so every user of a given sampler must agree on that order.
pub struct Sampled<T> {
    /// `None` until the first read asks for one — nothing is sampled on a box nobody is looking at.
    held: Mutex<Option<Held<T>>>,
}

impl<T> Default for Sampled<T> {
    fn default() -> Self {
        Self {
            held: Mutex::new(None),
        }
    }
}

impl<T: Clone> Sampled<T> {
    /// A sampler holding nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The value, no older than `max_age`, with the age it actually has — taking a fresh sample via
    /// `sample` only if the held one is too old.
    ///
    /// `max_age` is the caller's STALENESS TOLERANCE, and it is the only cadence control in the
    /// design: a one-shot human command passes [`Duration::ZERO`] and waits for its own fresh walk,
    /// while a display poll passes a window it can live with and is answered from the held sample
    /// for free. A caller that asks for zero tolerance is asking to WAIT, which is why this samples
    /// in place rather than scheduling and answering stale.
    ///
    /// Note what `max_age` does NOT do: it never makes an answer OLDER than it has to be. A sample
    /// taken a moment ago for somebody else is handed to a `Duration::ZERO` caller only if it is
    /// genuinely younger than the tolerance — and `ZERO` admits nothing, so that caller always
    /// samples.
    pub fn read(&self, max_age: Duration, sample: impl FnOnce() -> T) -> Reading<T> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        // Re-checked AFTER the lock, not before it: a reader that waited out somebody else's sample
        // arrives here with a value that did not exist when it started waiting. Checking before
        // would make every concurrent reader take its own sample, which is the whole cost this
        // exists to remove.
        if let Some(current) = held.as_ref() {
            let age = current.taken.elapsed();
            if age < max_age {
                return Reading {
                    age,
                    value: current.value.clone(),
                };
            }
        }
        let value = sample();
        *held = Some(Held {
            taken: Instant::now(),
            value: value.clone(),
        });
        Reading {
            age: Duration::ZERO,
            value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A tolerance of zero admits nothing, so every such reader samples afresh — the property
    /// `sprag ls` and every one-shot human command depend on.
    #[test]
    fn a_zero_tolerance_always_samples() {
        let sampler: Sampled<u32> = Sampled::new();
        let taken = AtomicU32::new(0);
        let sample = || taken.fetch_add(1, Ordering::Relaxed) + 1;

        assert_eq!(sampler.read(Duration::ZERO, sample).value, 1);
        assert_eq!(sampler.read(Duration::ZERO, sample).value, 2);
        assert_eq!(
            sampler.read(Duration::ZERO, sample).age,
            Duration::ZERO,
            "and a fresh sample reports no age at all",
        );
        assert_eq!(taken.load(Ordering::Relaxed), 3);
    }

    /// A tolerance wide enough to cover the held sample is answered from it, without sampling —
    /// which is what keeps a poll off the filesystem.
    #[test]
    fn a_tolerant_read_pays_nothing() {
        let sampler: Sampled<u32> = Sampled::new();
        let taken = AtomicU32::new(0);
        let sample = || taken.fetch_add(1, Ordering::Relaxed) + 1;

        assert_eq!(sampler.read(Duration::ZERO, sample).value, 1);
        let second = sampler.read(Duration::from_secs(3600), sample);
        assert_eq!(second.value, 1, "the held sample, not a new one");
        assert_eq!(taken.load(Ordering::Relaxed), 1, "and nothing was sampled");
        assert!(
            second.age > Duration::ZERO,
            "a held sample reports a real age, so a reader can distrust it for a reason",
        );
    }

    /// THE COALESCING, asserted rather than described: many readers arriving at once against an
    /// empty sampler produce ONE sample between them, and all of them see it.
    ///
    /// The sampling function blocks for long enough that the threads genuinely overlap; without the
    /// lock being held across the sample they would each take their own, which is the cost this
    /// type exists to remove and the one a later "optimisation" would silently restore.
    #[test]
    fn concurrent_readers_share_one_sample() {
        let sampler: Arc<Sampled<u32>> = Arc::new(Sampled::new());
        let taken = Arc::new(AtomicU32::new(0));
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let sampler = Arc::clone(&sampler);
                let taken = Arc::clone(&taken);
                std::thread::spawn(move || {
                    sampler
                        .read(Duration::from_secs(3600), || {
                            std::thread::sleep(Duration::from_millis(50));
                            taken.fetch_add(1, Ordering::Relaxed) + 1
                        })
                        .value
                })
            })
            .collect();
        let seen: Vec<u32> = readers.into_iter().map(|r| r.join().unwrap()).collect();

        assert_eq!(
            taken.load(Ordering::Relaxed),
            1,
            "eight concurrent readers, one walk",
        );
        assert!(
            seen.iter().all(|&v| v == 1),
            "and every one of them got that walk's answer: {seen:?}",
        );
    }
}
