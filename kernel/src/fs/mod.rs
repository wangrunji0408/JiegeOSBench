//! Filesystem layer (initramfs + tmpfs + VFS).
use crate::sync::SpinLock;
use alloc::string::String;

pub fn init() {}

pub fn initramfs_load(_start: usize, _end: usize) {}
