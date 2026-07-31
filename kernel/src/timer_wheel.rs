//! Timer wheel: one-shot timers keyed by wchan; periodic tick from timer.rs.

use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq)]
pub enum TimerKind {
    Wake,
    Net,
}

struct Timer {
    deadline: u64, // ms
    wchan: usize,
    kind: TimerKind,
}

static mut TIMERS: Vec<Timer> = Vec::new();

pub fn set_timer(deadline_ms: u64, wchan: usize, kind: TimerKind) {
    unsafe {
        TIMERS.push(Timer {
            deadline: deadline_ms,
            wchan,
            kind,
        });
    }
}

/// Called on every 10 ms tick.
pub fn on_tick() {
    let now = crate::timer::now_ms();
    let mut fired_wake: Vec<usize> = Vec::new();
    let mut fired_net = false;
    unsafe {
        TIMERS.retain(|t| {
            if t.deadline <= now {
                match t.kind {
                    TimerKind::Wake => fired_wake.push(t.wchan),
                    TimerKind::Net => fired_net = true,
                }
                false
            } else {
                true
            }
        });
    }
    for w in fired_wake {
        crate::task::wake_wchan(w);
    }
    if fired_net {
        crate::net::tcp::net_tick();
    }
}
