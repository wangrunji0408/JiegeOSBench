//! Minimal file abstraction. Concrete filesystem-backed files arrive in a
//! later milestone; for now this exists so the task module's fd table has
//! somewhere to put stdin/stdout/stderr.

use alloc::sync::Arc;

pub trait File: Send + Sync {
    fn readable(&self) -> bool {
        false
    }
    fn writable(&self) -> bool {
        false
    }
    fn read(&self, buf: &mut [u8]) -> usize {
        let _ = buf;
        0
    }
    fn write(&self, buf: &[u8]) -> usize {
        let _ = buf;
        0
    }
}

mod stdio;
pub use stdio::{Stdin, Stdout};

pub fn stdio_fd_table() -> alloc::vec::Vec<Option<Arc<dyn File>>> {
    alloc::vec![
        Some(Arc::new(Stdin) as Arc<dyn File>),
        Some(Arc::new(Stdout) as Arc<dyn File>),
        Some(Arc::new(Stdout) as Arc<dyn File>),
    ]
}
