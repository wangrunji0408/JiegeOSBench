/// 内存管理相关syscall

use alloc::vec::Vec;
use crate::task::current_task;
use crate::mm::{MapArea, MapPerm, MapType};
use crate::config::PAGE_SIZE;
use crate::task::process::MmapRegion;

use super::*;

// PROT flags
const PROT_NONE: i32 = 0;
const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const PROT_EXEC: i32 = 4;

// MAP flags
const MAP_SHARED: i32 = 1;
const MAP_PRIVATE: i32 = 2;
const MAP_FIXED: i32 = 16;
const MAP_ANONYMOUS: i32 = 32;
const MAP_ANON: i32 = MAP_ANONYMOUS;

pub fn sys_mmap(
    addr: usize,
    length: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    offset: i64,
) -> isize {
    if length == 0 { return EINVAL; }

    let task = current_task().unwrap();
    let mut t = task.lock();

    // 计算映射地址
    let map_start = if addr != 0 && (flags & MAP_FIXED != 0) {
        addr & !(PAGE_SIZE - 1)
    } else {
        find_free_mmap_addr(&t, length)
    };

    let map_end = (map_start + length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // 计算权限
    let mut perm = MapPerm::U;
    if prot & PROT_READ != 0 { perm |= MapPerm::R; }
    if prot & PROT_WRITE != 0 { perm |= MapPerm::W; }
    if prot & PROT_EXEC != 0 { perm |= MapPerm::X; }
    if perm.is_empty() { perm = MapPerm::R | MapPerm::U; }

    // 创建新映射
    let mut area = MapArea::new(map_start, map_end, MapType::Framed, perm);
    let mut frames_to_add = alloc::collections::BTreeMap::new();
    for vpn in (map_start >> crate::config::PAGE_SIZE_BITS)..(map_end >> crate::config::PAGE_SIZE_BITS) {
        let frame = crate::mm::alloc_frame().expect("out of memory");
        let ppn = frame.ppn();
        t.memory_set.page_table.remap(vpn, ppn, crate::mm::PTEFlags::from(perm));
        frames_to_add.insert(vpn, frame);
    }
    area.frames = frames_to_add;

    // 如果是文件映射，读取文件内容
    if flags & MAP_ANONYMOUS == 0 && fd >= 0 {
        let file_data = if let Some(crate::task::process::FileDesc::File { inode, .. }) = t.fds.get(&fd) {
            let node = inode.lock();
            if let crate::fs::ramfs::INodeKind::File(data) = &node.kind {
                let data = data.lock();
                let start = offset as usize;
                let end = (start + length).min(data.len());
                if start < data.len() {
                    Some(data[start..end].to_vec())
                } else {
                    None
                }
            } else { None }
        } else {
            None
        };

        if let Some(data) = file_data {
            t.memory_set.copy_to_user(map_start, &data);
        }
    }

    t.memory_set.areas.push(area);
    t.mmaps.push(MmapRegion {
        start: map_start,
        end: map_end,
        prot,
        flags,
        fd,
        offset: offset as usize,
    });

    map_start as isize
}

pub fn find_free_mmap_addr_pub(task: &crate::task::process::Task, length: usize) -> usize {
    find_free_mmap_addr(task, length)
}

fn find_free_mmap_addr(task: &crate::task::process::Task, length: usize) -> usize {
    // 从mmap区域上方开始分配
    const MMAP_BASE: usize = 0x40000000; // 1GB
    const MMAP_MAX: usize = 0x7f000000; // 不超过用户空间上限

    let mut addr = MMAP_BASE;
    let length_aligned = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // 找一个不冲突的地址
    loop {
        if addr + length_aligned > MMAP_MAX {
            // No space found in user range, use a high user address
            return MMAP_MAX - length_aligned;
        }
        let end = addr + length_aligned;
        let conflict = task.memory_set.areas.iter().any(|a| {
            a.start_va() < end && a.end_va() > addr
        });
        if !conflict { break; }
        addr += length_aligned;
    }

    addr
}

pub fn sys_munmap(addr: usize, length: usize) -> isize {
    let task = current_task().unwrap();
    let mut t = task.lock();

    let start = addr & !(PAGE_SIZE - 1);
    let end = (addr + length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // 找并移除对应的映射
    let to_remove: Vec<usize> = t.memory_set.areas.iter()
        .filter(|a| a.start_va() >= start && a.end_va() <= end)
        .map(|a| a.start_va())
        .collect();

    for va in to_remove {
        t.memory_set.remove_area_with_start_va(va);
    }

    // 从mmaps列表移除
    t.mmaps.retain(|m| !(m.start >= start && m.end <= end));

    0
}

pub fn sys_brk(addr: usize) -> isize {
    let task = current_task().unwrap();
    let mut t = task.lock();

    // 找到堆区域
    let heap_start = if t.brk == 0 {
        // 第一次调用brk，找堆的起始地址
        // 通常是程序段结束后
        let max_va = t.memory_set.areas.iter()
            .filter(|a| a.end_va() < 0x80000000) // Only user-space areas
            .map(|a| a.end_va())
            .max()
            .unwrap_or(0x10000000);
        let start = (max_va + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        t.brk = start;
        start
    } else {
        t.brk
    };

    if addr == 0 {
        // 返回当前brk
        return t.brk as isize;
    }

    if addr < heap_start {
        return t.brk as isize;
    }

    let new_brk = (addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let old_brk = t.brk;

    if new_brk > old_brk {
        // 扩展堆
        let area = MapArea::new(
            old_brk,
            new_brk,
            MapType::Framed,
            MapPerm::R | MapPerm::W | MapPerm::U,
        );
        t.memory_set.push(area, None);
    } else if new_brk < old_brk {
        // 收缩堆（简化：不实际释放）
    }

    t.brk = new_brk;
    new_brk as isize
}

pub fn sys_mremap(
    old_addr: usize,
    old_size: usize,
    new_size: usize,
    flags: i32,
    new_addr: usize,
) -> isize {
    // 简化实现：分配新区域并复制
    let task = current_task().unwrap();
    let mut t = task.lock();

    let new_addr = find_free_mmap_addr(&t, new_size);
    let new_end = (new_addr + new_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // 分配新区域
    let new_area = MapArea::new(
        new_addr,
        new_end,
        MapType::Framed,
        MapPerm::R | MapPerm::W | MapPerm::U,
    );

    // 复制数据
    let old_size_aligned = (old_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let copy_size = old_size.min(new_size);

    // 读取旧区域数据
    let mut data = vec![0u8; copy_size];
    t.memory_set.copy_from_user(old_addr, &mut data);

    t.memory_set.push(new_area, None);
    t.memory_set.copy_to_user(new_addr, &data);

    new_addr as isize
}
