use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct AtomicCounters {
    /// Packs together a number of counters. The counters are ordered as
    /// follows, from least to most significant bits (here, we assuming
    /// that [`THREADS_BITS`] is equal to 10):
    ///
    /// * Bits 0..10: Stores the number of **sleeping threads**
    /// * Bits 10..20: Stores the number of **inactive threads**
    /// * Bits 20..: Stores the **job event counter** (JEC)
    ///
    /// This uses 10 bits ([`THREADS_BITS`]) to encode the number of threads. Note
    /// that the total number of bits (and hence the number of bits used for the
    /// JEC) will depend on whether we are using a 32- or 64-bit architecture.
    sleeping_threads: AtomicUsize,
    inactive_threads: AtomicUsize,
    jobs_event_counter: AtomicUsize,
}

/// A value read from the **Jobs Event Counter**.
/// See the [`README.md`](README.md) for more
/// coverage of how the jobs event counter works.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub(super) struct JobsEventCounter(usize);

impl JobsEventCounter {
    pub(super) const DUMMY: JobsEventCounter = JobsEventCounter(std::usize::MAX);

    #[inline]
    pub(super) fn as_usize(self) -> usize {
        self.0
    }

    /// The JEC "is sleepy" if the last thread to increment it was in the
    /// process of becoming sleepy. This is indicated by its value being *even*.
    /// When new jobs are posted, they check if the JEC is sleepy, and if so
    /// they incremented it.
    #[inline]
    pub(super) fn is_sleepy(self) -> bool {
        (self.as_usize() & 1) == 0
    }

    /// The JEC "is active" if the last thread to increment it was posting new
    /// work. This is indicated by its value being *odd*. When threads get
    /// sleepy, they will check if the JEC is active, and increment it.
    #[inline]
    pub(super) fn is_active(self) -> bool {
        !self.is_sleepy()
    }
}

/// Number of bits used for the thread counters.
#[cfg(target_pointer_width = "64")]
const THREADS_BITS: usize = 16;

#[cfg(target_pointer_width = "32")]
const THREADS_BITS: usize = 8;

/// Max value for the thread counters.
pub(crate) const THREADS_MAX: usize = (1 << THREADS_BITS) - 1;

impl AtomicCounters {
    #[inline]
    pub(super) fn new() -> AtomicCounters {
        AtomicCounters {
            sleeping_threads: AtomicUsize::new(0),
            inactive_threads: AtomicUsize::new(0),
            jobs_event_counter: AtomicUsize::new(0),
        }
    }

    #[inline]
    pub(super) fn load_sleeping_threads_counter(
        &self,
        ordering: Ordering,
    ) -> SleepingThreadsCounter {
        SleepingThreadsCounter::new(self.sleeping_threads.load(ordering))
    }

    #[inline]
    pub(super) fn load_inactive_threads_counter(
        &self,
        ordering: Ordering,
    ) -> InactiveThreadsCounter {
        InactiveThreadsCounter::new(self.inactive_threads.load(ordering))
    }

    #[inline]
    pub(super) fn load_jobs_event_counter(&self, ordering: Ordering) -> JobEventsCounter {
        JobEventsCounter::new(self.jobs_event_counter.load(ordering))
    }

    #[inline]
    fn try_exchange_sleeping_threads(
        &self,
        old_value: SleepingThreadsCounter,
        new_value: SleepingThreadsCounter,
        ordering: Ordering,
    ) -> bool {
        self.sleeping_threads
            .compare_exchange(
                old_value.sleeping_threads,
                new_value.sleeping_threads,
                ordering,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    #[inline]
    fn try_exchange_jec(
        &self,
        old_value: JobEventsCounter,
        new_value: JobEventsCounter,
        ordering: Ordering,
    ) -> bool {
        self.jobs_event_counter
            .compare_exchange(
                old_value.job_events_counter,
                new_value.job_events_counter,
                ordering,
                Ordering::Relaxed,
            )
            .is_ok()
    }
    /// Adds an inactive thread. This cannot fail.
    ///
    /// This should be invoked when a thread enters its idle loop looking
    /// for work. It is decremented when work is found. Note that it is
    /// not decremented if the thread transitions from idle to sleepy or sleeping;
    /// so the number of inactive threads is always greater-than-or-equal
    /// to the number of sleeping threads.
    #[inline]
    pub(super) fn add_inactive_thread(&self) {
        self.inactive_threads.fetch_add(1, Ordering::SeqCst);
    }

    /// Increments the jobs event counter if `increment_when`, when applied to
    /// the current value, is true. Used to toggle the JEC from even (sleepy) to
    /// odd (active) or vice versa. Returns the final value of the counters, for
    /// which `increment_when` is guaranteed to return false.
    pub(super) fn increment_jobs_event_counter_if(
        &self,
        increment_when: impl Fn(JobsEventCounter) -> bool,
    ) -> JobEventsCounter {
        loop {
            let old_value = self.load_jobs_event_counter(Ordering::SeqCst);
            if increment_when(old_value.jobs_counter()) {
                let new_value = old_value.increment_jobs_counter();
                if self.try_exchange_jec(old_value, new_value, Ordering::SeqCst) {
                    return new_value;
                }
            } else {
                return old_value;
            }
        }
    }

    /// Subtracts an inactive thread. This cannot fail. It is invoked
    /// when a thread finds work and hence becomes active. It returns the
    /// number of sleeping threads to wake up (if any).
    ///
    /// See `add_inactive_thread`.
    #[inline]
    pub(super) fn sub_inactive_thread(&self) -> usize {
        self.inactive_threads.fetch_sub(1, Ordering::SeqCst);
        let sleeping_threads = self.sleeping_threads.load(Ordering::SeqCst);

        // Current heuristic: whenever an inactive thread goes away, if
        // there are any sleeping threads, wake 'em up.
        std::cmp::min(sleeping_threads, 2)
    }

    /// Subtracts a sleeping thread. This cannot fail, but it is only
    /// safe to do if you you know the number of sleeping threads is
    /// non-zero (i.e., because you have just awoken a sleeping
    /// thread).
    #[inline]
    pub(super) fn sub_sleeping_thread(&self) {
        self.sleeping_threads.fetch_sub(1, Ordering::SeqCst);
    }

    #[inline]
    pub(super) fn try_add_sleeping_thread(&self, old_value: SleepingThreadsCounter) -> bool {
        let mut new_value = old_value;
        new_value.sleeping_threads += 1;

        self.try_exchange_sleeping_threads(old_value, new_value, Ordering::SeqCst)
    }
}

#[derive(Copy, Clone)]
pub(super) struct SleepingThreadsCounter {
    sleeping_threads: usize,
}

impl SleepingThreadsCounter {
    #[inline]
    fn new(sleeping_threads: usize) -> Self {
        Self { sleeping_threads }
    }

    #[inline]
    pub(super) fn sleeping_threads(self) -> usize {
        self.sleeping_threads
    }
}

#[derive(Copy, Clone)]
pub(super) struct InactiveThreadsCounter {
    inactive_threads: usize,
}

impl InactiveThreadsCounter {
    #[inline]
    fn new(inactive_threads: usize) -> Self {
        Self { inactive_threads }
    }

    #[inline]
    pub(super) fn inactive_threads(self) -> usize {
        self.inactive_threads
    }
}

#[derive(Copy, Clone)]
pub(super) struct JobEventsCounter {
    job_events_counter: usize,
}

impl JobEventsCounter {
    #[inline]
    fn new(job_events_counter: usize) -> Self {
        Self { job_events_counter }
    }

    #[inline]
    fn increment_jobs_counter(self) -> Self {
        Self {
            job_events_counter: self.job_events_counter.wrapping_add(1),
            ..self
        }
    }

    #[inline]
    pub(super) fn jobs_counter(self) -> JobsEventCounter {
        JobsEventCounter(self.job_events_counter)
    }
}
