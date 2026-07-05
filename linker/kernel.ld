OUTPUT_ARCH(riscv)
ENTRY(_start)

/* QEMU virt: OpenSBI 占用 0x80000000，内核被加载到 0x80200000 */
BASE_ADDR = 0x80200000;

MEMORY {
    RAM (rwxa) : ORIGIN = BASE_ADDR, LENGTH = 128M
}

SECTIONS
{
    . = BASE_ADDR;

    /* 启动入口，必须放在最前，OpenSBI 跳转到这里 */
    .text : {
        *(.text.init)      /* entry.S 里的 _start */
        *(.text .text.*)
    } > RAM

    .rodata : ALIGN(4) {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    } > RAM

    .data : ALIGN(4) {
        *(.data .data.*)
        *(.sdata .sdata.*)
    } > RAM

    .bss : ALIGN(8) {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        __bss_end = .;
    } > RAM

    /* 用于分配页表/堆的元信息 */
    . = ALIGN(4096);
    __kernel_end = .;
}
