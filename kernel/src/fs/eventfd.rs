use super::File;
use spin::Mutex;

pub struct EventFd {
    counter: Mutex<u64>,
    semaphore_mode: bool,
}

impl EventFd {
    pub fn new(initval: u64, semaphore_mode: bool) -> Self {
        Self {
            counter: Mutex::new(initval),
            semaphore_mode,
        }
    }
}

impl File for EventFd {
    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        true
    }
    fn read(&self, buf: &mut [u8]) -> usize {
        if buf.len() < 8 {
            return 0;
        }
        let mut c = self.counter.lock();
        if *c == 0 {
            return 0;
        }
        let val = if self.semaphore_mode {
            let v = 1u64;
            *c -= 1;
            v
        } else {
            let v = *c;
            *c = 0;
            v
        };
        buf[0..8].copy_from_slice(&val.to_ne_bytes());
        8
    }
    fn write(&self, buf: &[u8]) -> usize {
        if buf.len() < 8 {
            return 0;
        }
        let add = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
        *self.counter.lock() += add;
        8
    }
    fn poll_readable(&self) -> bool {
        *self.counter.lock() != 0
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}
