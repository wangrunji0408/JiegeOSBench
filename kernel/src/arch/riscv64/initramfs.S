    .section .initramfs
    .globl _initramfs_start
_initramfs_start:
    .incbin "/Users/wangrunji/Codes/iJiege-sonnet-1m/initramfs.cpio"
    .globl _initramfs_end
_initramfs_end:
