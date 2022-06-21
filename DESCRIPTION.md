# Description

The idea here is to decrease the time taken by the thread yield
from the original algorithm replacing it by a busy-wait loop. The time
we busy-wait for is multiplied by a factor each time we fail to steal.
When the time reaches a threshold, we stop busy-waiting and start calling
`thread::sleep` instead. The main parameter of this algorithm is the
`SLEEPING_THRESHOLD`, but the `WAITING_TIME_MULTIPLIER` also has impact on
the general behavior.

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
        self.sleep(idle_state, latch, has_injected_jobs);
    }
}
```
