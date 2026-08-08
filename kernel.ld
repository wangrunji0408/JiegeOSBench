OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x80200000;
    .text : ALIGN(4K) {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }
    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }
    .data : ALIGN(4K) {
        *(.data .data.*)
    }
    .bss : ALIGN(4K) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }
    . = ALIGN(16);
    __kernel_end = .;
}

