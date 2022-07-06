//! Code that decides when workers should go to sleep. See README.md
//! for an overview.

use crate::latch::CoreLatch;
use crate::log::Event::*;
use crate::log::Logger;
use crossbeam_utils::CachePadded;
use std::sync::atomic::Ordering;
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::usize;

mod counters;
pub(crate) use self::counters::THREADS_MAX;
use self::counters::{AtomicCounters, JobsEventCounter};

/// The `Sleep` struct is embedded into each registry. It governs the waking and sleeping
/// of workers. It has callbacks that are invoked periodically at significant events,
/// such as when workers are looping and looking for work, when latches are set, or when
/// jobs are published, and it either blocks threads or wakes them in response to these
/// events. See the [`README.md`] in this module for more details.
///
/// [`README.md`] README.md
pub(super) struct Sleep {
    logger: Logger,

    /// One "sleep state" per worker. Used to track if a worker is sleeping and to have
    /// them block.
    worker_sleep_states: Vec<CachePadded<WorkerSleepState>>,

    counters: AtomicCounters,
}

/// An instance of this struct is created when a thread becomes idle.
/// It is consumed when the thread finds work, and passed by `&mut`
/// reference for operations that preserve the idle state. (In other
/// words, producing one of these structs is evidence the thread is
/// idle.) It tracks state such as how long the thread has been idle.
pub(super) struct IdleState {
    /// What is worker index of the idle thread?
    worker_index: usize,

    waiting_cycles: u64,

    last_waited_duration: Duration,

    /// Once we become sleepy, what was the sleepy counter value?
    /// Set to `INVALID_SLEEPY_COUNTER` otherwise.
    jobs_counter: JobsEventCounter,
}

/// The "sleep state" for an individual worker.
#[derive(Default)]
struct WorkerSleepState {
    /// Set to true when the worker goes to sleep; set to false when
    /// the worker is notified or when it wakes.
    is_blocked: Mutex<bool>,

    condvar: Condvar,
}

const INITIAL_WAITING_CYCLES: u64 = 40;
const WAITING_CYCLES_MULTIPLIER: f64 = 1.25;
const SLEEPING_THRESHOLD: Duration = Duration::from_micros(500);

impl Sleep {
    pub(super) fn new(logger: Logger, n_threads: usize) -> Sleep {
        assert!(n_threads <= THREADS_MAX);
        Sleep {
            logger,
            worker_sleep_states: (0..n_threads).map(|_| Default::default()).collect(),
            counters: AtomicCounters::new(),
        }
    }

    #[inline]
    pub(super) fn start_looking(&self, worker_index: usize, latch: &CoreLatch) -> IdleState {
        self.logger.log(|| ThreadIdle {
            worker: worker_index,
            latch_addr: latch.addr(),
        });

        self.counters.add_inactive_thread();

        IdleState {
            worker_index,
            waiting_cycles: INITIAL_WAITING_CYCLES,
            last_waited_duration: Duration::from_secs(0),
            jobs_counter: JobsEventCounter::DUMMY,
        }
    }

    #[inline]
    pub(super) fn work_found(&self, idle_state: IdleState) {
        self.logger.log(|| ThreadFoundWork {
            worker: idle_state.worker_index,
            yields: 0,
        });

        // If we were the last idle thread and other threads are still sleeping,
        // then we should wake up another thread.
        let threads_to_wake = self.counters.sub_inactive_thread();
        self.wake_any_threads(threads_to_wake as u32);
    }

    #[inline]
    pub(super) fn no_work_found(
        &self,
        idle_state: &mut IdleState,
        latch: &CoreLatch,
        has_injected_jobs: impl FnOnce() -> bool,
    ) {
        if idle_state.last_waited_duration <= SLEEPING_THRESHOLD {
            let start = Instant::now();

            for _ in 0..idle_state.waiting_cycles {
                unsafe { std::arch::asm!("nop") }
            }

            idle_state.last_waited_duration = start.elapsed();
            idle_state.waiting_cycles =
                (idle_state.waiting_cycles as f64 * WAITING_CYCLES_MULTIPLIER) as u64;
        } else {
            let span = tracing::span!(tracing::Level::TRACE, "sleep");
            let _guard = span.enter();
            self.sleep(idle_state, latch, has_injected_jobs);
        }
    }

    #[cold]
    fn sleep(
        &self,
        idle_state: &mut IdleState,
        latch: &CoreLatch,
        has_injected_jobs: impl FnOnce() -> bool,
    ) {
        thread::sleep(SLEEPING_THRESHOLD)
    }

    /// Notify the given thread that it should wake up (if it is
    /// sleeping).  When this method is invoked, we typically know the
    /// thread is asleep, though in rare cases it could have been
    /// awoken by (e.g.) new work having been posted.
    pub(super) fn notify_worker_latch_is_set(&self, target_worker_index: usize) {
        self.wake_specific_thread(target_worker_index);
    }

    /// Signals that `num_jobs` new jobs were injected into the thread
    /// pool from outside. This function will ensure that there are
    /// threads available to process them, waking threads from sleep
    /// if necessary.
    ///
    /// # Parameters
    ///
    /// - `source_worker_index` -- index of the thread that did the
    ///   push, or `usize::MAX` if this came from outside the thread
    ///   pool -- it is used only for logging.
    /// - `num_jobs` -- lower bound on number of jobs available for stealing.
    ///   We'll try to get at least one thread per job.
    #[inline]
    pub(super) fn new_injected_jobs(
        &self,
        source_worker_index: usize,
        num_jobs: u32,
        queue_was_empty: bool,
    ) {
        // This fence is needed to guarantee that threads
        // as they are about to fall asleep, observe any
        // new jobs that may have been injected.
        std::sync::atomic::fence(Ordering::SeqCst);

        self.new_jobs(source_worker_index, num_jobs, queue_was_empty)
    }

    /// Signals that `num_jobs` new jobs were pushed onto a thread's
    /// local deque. This function will try to ensure that there are
    /// threads available to process them, waking threads from sleep
    /// if necessary. However, this is not guaranteed: under certain
    /// race conditions, the function may fail to wake any new
    /// threads; in that case the existing thread should eventually
    /// pop the job.
    ///
    /// # Parameters
    ///
    /// - `source_worker_index` -- index of the thread that did the
    ///   push, or `usize::MAX` if this came from outside the thread
    ///   pool -- it is used only for logging.
    /// - `num_jobs` -- lower bound on number of jobs available for stealing.
    ///   We'll try to get at least one thread per job.
    #[inline]
    pub(super) fn new_internal_jobs(
        &self,
        source_worker_index: usize,
        num_jobs: u32,
        queue_was_empty: bool,
    ) {
        self.new_jobs(source_worker_index, num_jobs, queue_was_empty)
    }

    /// Common helper for `new_injected_jobs` and `new_internal_jobs`.
    #[inline]
    fn new_jobs(&self, source_worker_index: usize, num_jobs: u32, queue_was_empty: bool) {
        // Read the counters and -- if sleepy workers have announced themselves
        // -- announce that there is now work available. The final value of `counters`
        // with which we exit the loop thus corresponds to a state when
        let counters = self
            .counters
            .increment_jobs_event_counter_if(JobsEventCounter::is_sleepy);
        let num_awake_but_idle = counters.awake_but_idle_threads();
        let num_sleepers = counters.sleeping_threads();

        self.logger.log(|| JobThreadCounts {
            worker: source_worker_index,
            num_idle: num_awake_but_idle as u16,
            num_sleepers: num_sleepers as u16,
        });

        if num_sleepers == 0 {
            // nobody to wake
            return;
        }

        // Promote from u16 to u32 so we can interoperate with
        // num_jobs more easily.
        let num_awake_but_idle = num_awake_but_idle as u32;
        let num_sleepers = num_sleepers as u32;

        // If the queue is non-empty, then we always wake up a worker
        // -- clearly the existing idle jobs aren't enough. Otherwise,
        // check to see if we have enough idle workers.
        if !queue_was_empty {
            let num_to_wake = std::cmp::min(num_jobs, num_sleepers);
            self.wake_any_threads(num_to_wake);
        } else if num_awake_but_idle < num_jobs {
            let num_to_wake = std::cmp::min(num_jobs - num_awake_but_idle, num_sleepers);
            self.wake_any_threads(num_to_wake);
        }
    }

    #[cold]
    fn wake_any_threads(&self, mut num_to_wake: u32) {
        if num_to_wake > 0 {
            for i in 0..self.worker_sleep_states.len() {
                if self.wake_specific_thread(i) {
                    num_to_wake -= 1;
                    if num_to_wake == 0 {
                        return;
                    }
                }
            }
        }
    }

    fn wake_specific_thread(&self, index: usize) -> bool {
        let sleep_state = &self.worker_sleep_states[index];

        let mut is_blocked = sleep_state.is_blocked.lock().unwrap();
        if *is_blocked {
            *is_blocked = false;
            sleep_state.condvar.notify_one();

            // When the thread went to sleep, it will have incremented
            // this value. When we wake it, its our job to decrement
            // it. We could have the thread do it, but that would
            // introduce a delay between when the thread was
            // *notified* and when this counter was decremented. That
            // might mislead people with new work into thinking that
            // there are sleeping threads that they should try to
            // wake, when in fact there is nothing left for them to
            // do.
            self.counters.sub_sleeping_thread();

            self.logger.log(|| ThreadNotify { worker: index });

            true
        } else {
            false
        }
    }
}

impl IdleState {
    fn wake_fully(&mut self) {
        self.waiting_cycles = INITIAL_WAITING_CYCLES;
        self.last_waited_duration = Duration::from_secs(0);
        self.jobs_counter = JobsEventCounter::DUMMY;
    }

    fn wake_partly(&mut self) {}
}
