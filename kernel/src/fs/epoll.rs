use super::File;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

pub struct EpollEntry {
    pub fd: i32,
    pub events: u32,
    pub data: u64,
    pub file: Arc<dyn File>,
    /// Readiness already reported for an edge-triggered (`EPOLLET`) entry,
    /// so it isn't reported again until the condition drops and comes back
    /// -- otherwise a still-true condition (e.g. a peer that closed and
    /// stays closed) would be re-reported every single poll forever.
    pub reported_readable: bool,
    pub reported_writable: bool,
}

pub struct EpollFile {
    pub entries: Mutex<Vec<EpollEntry>>,
}

impl EpollFile {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }
}

impl File for EpollFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}
