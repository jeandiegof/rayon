//! Code that decides when workers should go to sleep. See README.md
//! for an overview.

use crate::latch::CoreLatch;
use crate::log::Event::*;
use crate::log::Logger;
use std::thread;
use std::time::Duration;
use std::usize;

/// Number of bits used for the thread counters.
#[cfg(target_pointer_width = "64")]
const THREADS_BITS: usize = 16;

#[cfg(target_pointer_width = "32")]
const THREADS_BITS: usize = 8;

pub(crate) const THREADS_MAX: usize = (1 << THREADS_BITS) - 1;

/// The `Sleep` struct is embedded into each registry. It governs the waking and sleeping
/// of workers. It has callbacks that are invoked periodically at significant events,
/// such as when workers are looping and looking for work, when latches are set, or when
/// jobs are published, and it either blocks threads or wakes them in response to these
/// events. See the [`README.md`] in this module for more details.
///
/// [`README.md`] README.md
pub(super) struct Sleep {
    logger: Logger,
}

/// An instance of this struct is created when a thread becomes idle.
/// It is consumed when the thread finds work, and passed by `&mut`
/// reference for operations that preserve the idle state. (In other
/// words, producing one of these structs is evidence the thread is
/// idle.) It tracks state such as how long the thread has been idle.
pub(super) struct IdleState {
    /// What is worker index of the idle thread?
    worker_index: usize,

    /// Steal attempts
    steal_attempts: u32,

    /// Waiting time
    waiting_time: u64,
}

const MAX_STEAL_ATTEMPTS: u32 = 16;
const INITIAL_WAITING_TIME: u64 = 40;
const WAITING_TIME_MULTIPLIER: u64 = 2;
const SLEEP_DURATION: Duration = Duration::from_millis(10);

impl Sleep {
    pub(super) fn new(logger: Logger, n_threads: usize) -> Sleep {
        assert!(n_threads <= THREADS_MAX);
        Sleep { logger }
    }

    #[inline]
    pub(super) fn start_looking(&self, worker_index: usize, latch: &CoreLatch) -> IdleState {
        self.logger.log(|| ThreadIdle {
            worker: worker_index,
            latch_addr: latch.addr(),
        });

        IdleState {
            worker_index,
            steal_attempts: 0,
            waiting_time: INITIAL_WAITING_TIME,
        }
    }

    #[inline]
    pub(super) fn work_found(&self, idle_state: IdleState) {
        self.logger.log(|| ThreadFoundWork {
            worker: idle_state.worker_index,
            yields: idle_state.steal_attempts,
        });
    }

    #[inline]
    pub(super) fn no_work_found(
        &self,
        idle_state: &mut IdleState,
        latch: &CoreLatch,
        has_injected_jobs: impl FnOnce() -> bool,
    ) {
        if idle_state.steal_attempts < MAX_STEAL_ATTEMPTS {
            idle_state.steal_attempts += 1;
<<<<<<< Updated upstream
            for _ in 0..idle_state.waiting_time {}
            idle_state.waiting_time = idle_state.waiting_time * WAITING_TIME_MULTIPLIER;
        } else {
=======
            let span = tracing::span!(tracing::Level::TRACE, "busy");
            let _guard = span.enter();
            for _ in 0..idle_state.waiting_time {
                // without this, the loop is optimized away by the compiler
                unsafe { std::arch::asm!("nop") }
            }
            idle_state.waiting_time = idle_state.waiting_time * WAITING_TIME_MULTIPLIER;
        } else {
            let span = tracing::span!(tracing::Level::TRACE, "sleep");
            let _guard = span.enter();
>>>>>>> Stashed changes
            self.sleep(idle_state, latch, has_injected_jobs);
        }
    }

    #[cold]
    fn sleep(
        &self,
        _idle_state: &mut IdleState,
        _latch: &CoreLatch,
        _has_inject_jobs: impl FnOnce() -> bool,
    ) {
        thread::sleep(SLEEP_DURATION);
    }
}
