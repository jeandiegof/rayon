# Description

Compared to the algorithm of the branch `new-algorithm`, the idea here
is to decrease the time taken to find new jobs from O(log p) (note to self:
is it O(log p) or something else?) to O(1) using a counter for new jobs.
Whenever a job is posted, the global jobs counter is incremented. Whenever
a thread enters the Idle State, it saves the current value of the jobs
counter. Before calling `thread::sleep`, the thread verifies the current value
of the global jobs counter and goes to sleep only if the value hasn't changed.
If it has changed, the worker will update its local jobs counter and look
for jobs one last time.

```rust
pub(super) fn no_work_found(
    &self,
    idle_state: &mut IdleState,
    latch: &CoreLatch,
    has_injected_jobs: impl FnOnce() -> bool,
) {
    if idle_state.last_waited_duration < *SLEEPING_THRESHOLD {
        let start = Instant::now();

        for _ in 0..idle_state.waiting_cycles {
            unsafe { std::arch::asm!("nop") }
        }

        idle_state.last_waited_duration = start.elapsed();
        idle_state.waiting_cycles = idle_state.waiting_cycles * WAITING_TIME_MULTIPLIER;
    } else {
        let jobs_counter_now = self.jobs_counter.load(Ordering::SeqCst);

        if jobs_counter_now == idle_state.jobs_counter {
            self.sleep(idle_state, latch, has_injected_jobs);
        } else {
            idle_state.jobs_counter = jobs_counter_now;
        }
    }
}
```
