//! 外部中断处理。QEMU virt 用 SiFive PLIC。
//! 此阶段仅占位，Phase 7/8 接入 virtio 设备。

pub fn external() {
    // 读取 claim 寄存器，处理外部设备中断
    // PLIC 在 0x0c00_0000, claim@0x201004
    // TODO: Phase 7 接入 virtio-blk/net 时实现
}
