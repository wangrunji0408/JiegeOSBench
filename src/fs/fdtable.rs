//! The per-process file descriptor table.

use super::file::File;
use super::{Error, Result};
use crate::bail;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Default and maximum number of open files.
pub const DEFAULT_NOFILE: usize = 1024;
pub const MAX_NOFILE: usize = 65536;

#[derive(Clone)]
struct Slot {
    file: Arc<File>,
    cloexec: bool,
}

pub struct FdTable {
    slots: Vec<Option<Slot>>,
    /// `RLIMIT_NOFILE`: nginx raises this with `setrlimit` when configured with
    /// `worker_rlimit_nofile`.
    pub limit: usize,
}

impl FdTable {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            limit: DEFAULT_NOFILE,
        }
    }

    pub fn get(&self, fd: i32) -> Option<Arc<File>> {
        if fd < 0 {
            return None;
        }
        self.slots.get(fd as usize)?.as_ref().map(|s| s.file.clone())
    }

    pub fn get_or_err(&self, fd: i32) -> Result<Arc<File>> {
        self.get(fd).ok_or(Error::new(super::errno::EBADF))
    }

    /// Find the lowest free fd at or above `min`.
    fn find_free(&mut self, min: usize) -> Result<usize> {
        for i in min..self.slots.len() {
            if self.slots[i].is_none() {
                return Ok(i);
            }
        }
        let fd = self.slots.len().max(min);
        if fd >= self.limit {
            bail!(EMFILE);
        }
        // Grow to cover `fd`.
        self.slots.resize(fd + 1, None);
        Ok(fd)
    }

    /// Install a file at the lowest available descriptor.
    pub fn insert(&mut self, file: Arc<File>, cloexec: bool) -> Result<i32> {
        self.insert_from(file, 0, cloexec)
    }

    /// Install a file at the lowest descriptor >= `min` (used by `F_DUPFD`).
    pub fn insert_from(&mut self, file: Arc<File>, min: usize, cloexec: bool) -> Result<i32> {
        let fd = self.find_free(min)?;
        self.slots[fd] = Some(Slot { file, cloexec });
        Ok(fd as i32)
    }

    /// Install at an exact descriptor, closing whatever was there (`dup2`).
    pub fn insert_at(&mut self, fd: i32, file: Arc<File>, cloexec: bool) -> Result<i32> {
        if fd < 0 || fd as usize >= self.limit {
            bail!(EBADF);
        }
        let fd = fd as usize;
        if fd >= self.slots.len() {
            self.slots.resize(fd + 1, None);
        }
        self.slots[fd] = Some(Slot { file, cloexec });
        Ok(fd as i32)
    }

    pub fn close(&mut self, fd: i32) -> Result<()> {
        if fd < 0 || fd as usize >= self.slots.len() {
            bail!(EBADF);
        }
        if self.slots[fd as usize].take().is_none() {
            bail!(EBADF);
        }
        Ok(())
    }

    /// Close every descriptor in `[from, to]` (`close_range`).
    pub fn close_range(&mut self, from: u32, to: u32) {
        let end = (to as usize).min(self.slots.len().saturating_sub(1));
        for fd in from as usize..=end {
            if fd < self.slots.len() {
                self.slots[fd] = None;
            }
        }
    }

    pub fn get_cloexec(&self, fd: i32) -> Result<bool> {
        if fd < 0 {
            bail!(EBADF);
        }
        self.slots
            .get(fd as usize)
            .and_then(|s| s.as_ref())
            .map(|s| s.cloexec)
            .ok_or(Error::new(super::errno::EBADF))
    }

    pub fn set_cloexec(&mut self, fd: i32, cloexec: bool) -> Result<()> {
        if fd < 0 {
            bail!(EBADF);
        }
        let slot = self
            .slots
            .get_mut(fd as usize)
            .and_then(|s| s.as_mut())
            .ok_or(Error::new(super::errno::EBADF))?;
        slot.cloexec = cloexec;
        Ok(())
    }

    /// Duplicate the table for `fork`: descriptors are shared, not copied.
    pub fn clone_for_fork(&self) -> Self {
        Self {
            slots: self.slots.clone(),
            limit: self.limit,
        }
    }

    /// Drop the `O_CLOEXEC` descriptors on `execve`.
    pub fn close_on_exec(&mut self) {
        for slot in self.slots.iter_mut() {
            if slot.as_ref().is_some_and(|s| s.cloexec) {
                *slot = None;
            }
        }
    }

    /// Every open descriptor, as (fd, file) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (i32, Arc<File>)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (i as i32, s.file.clone())))
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }
}
