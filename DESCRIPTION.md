# Description

This algorithm looks for jobs again immediately if the jobs counter has changed.
Whenever the jobs counter changed, we restart the idle state.

```rust
pub(super) fn no_work_found(
    &self,
    idle_state: &mut IdleState,
    latch: &CoreLatch,
    has_injected_jobs: impl FnOnce() -> bool,
) {
    let jobs_counter_now = self.jobs_counter.load(Ordering::SeqCst);

    if jobs_counter_now != idle_state.jobs_counter {
        idle_state.jobs_counter = jobs_counter_now;
        idle_state.last_waited_duration = Duration::from_secs(0);
        idle_state.waiting_cycles = INITIAL_WAITING_CYCLES;

        return;
    }

    if idle_state.last_waited_duration < *SLEEPING_THRESHOLD {
        let start = Instant::now();

        for _ in 0..idle_state.waiting_cycles {
            unsafe { std::arch::asm!("nop") }
        }

        idle_state.last_waited_duration = start.elapsed();
        idle_state.waiting_cycles = idle_state.waiting_cycles * WAITING_TIME_MULTIPLIER;
    } else {
        self.sleep(idle_state, latch, has_injected_jobs);
    }
}
```
