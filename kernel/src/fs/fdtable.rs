//! Per-process file descriptor table.
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::file::File;
use crate::abi::*;

#[derive(Clone)]
pub struct FdEntry {
    pub file: Arc<File>,
    pub cloexec: bool,
}

pub struct FdTable {
    entries: Vec<Option<FdEntry>>,
    pub limit: usize,
}

impl FdTable {
    pub fn new() -> Self {
        FdTable { entries: Vec::new(), limit: 1024 }
    }

    pub fn clone_table(&self) -> FdTable {
        FdTable { entries: self.entries.clone(), limit: self.limit }
    }

    pub fn get(&self, fd: i32) -> Result<Arc<File>, i32> {
        if fd < 0 {
            return Err(EBADF);
        }
        self.entries.get(fd as usize).and_then(|e| e.as_ref()).map(|e| e.file.clone()).ok_or(EBADF)
    }

    pub fn get_entry(&self, fd: i32) -> Result<&FdEntry, i32> {
        if fd < 0 {
            return Err(EBADF);
        }
        self.entries.get(fd as usize).and_then(|e| e.as_ref()).ok_or(EBADF)
    }

    pub fn set_cloexec(&mut self, fd: i32, on: bool) -> Result<(), i32> {
        let e = self.entries.get_mut(fd as usize).and_then(|e| e.as_mut()).ok_or(EBADF)?;
        e.cloexec = on;
        Ok(())
    }

    /// Install `file` at the lowest free fd >= min.
    pub fn alloc(&mut self, file: Arc<File>, cloexec: bool, min: usize) -> Result<i32, i32> {
        let mut i = min;
        while i < self.entries.len() {
            if self.entries[i].is_none() {
                self.entries[i] = Some(FdEntry { file, cloexec });
                return Ok(i as i32);
            }
            i += 1;
        }
        if i >= self.limit {
            return Err(EMFILE);
        }
        while self.entries.len() < i {
            self.entries.push(None);
        }
        self.entries.push(Some(FdEntry { file, cloexec }));
        Ok(i as i32)
    }

    /// Install at a specific fd (closing whatever was there).
    pub fn set(&mut self, fd: i32, file: Arc<File>, cloexec: bool) -> Result<(), i32> {
        if fd < 0 || fd as usize >= self.limit {
            return Err(EBADF);
        }
        let fd = fd as usize;
        while self.entries.len() <= fd {
            self.entries.push(None);
        }
        self.entries[fd] = Some(FdEntry { file, cloexec });
        Ok(())
    }

    pub fn close(&mut self, fd: i32) -> Result<Arc<File>, i32> {
        if fd < 0 {
            return Err(EBADF);
        }
        let slot = self.entries.get_mut(fd as usize).ok_or(EBADF)?;
        let e = slot.take().ok_or(EBADF)?;
        Ok(e.file)
    }

    pub fn close_on_exec(&mut self) -> Vec<Arc<File>> {
        let mut closed = Vec::new();
        for slot in self.entries.iter_mut() {
            if slot.as_ref().map(|e| e.cloexec).unwrap_or(false) {
                closed.push(slot.take().unwrap().file);
            }
        }
        closed
    }

    pub fn iter(&self) -> impl Iterator<Item = (i32, &FdEntry)> {
        self.entries.iter().enumerate().filter_map(|(i, e)| e.as_ref().map(|e| (i as i32, e)))
    }

    pub fn count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_some()).count()
    }
}
