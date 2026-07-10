/// 向上对齐
#[inline]
pub fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

/// 向下对齐
#[inline]
pub fn align_down(val: usize, align: usize) -> usize {
    val & !(align - 1)
}

/// 将物理地址转换为内核虚拟地址
#[inline]
pub fn phys_to_virt(pa: usize) -> usize {
    pa + crate::config::KERNEL_OFFSET
}

/// 将内核虚拟地址转换为物理地址
#[inline]
pub fn virt_to_phys(va: usize) -> usize {
    va - crate::config::KERNEL_OFFSET
}
