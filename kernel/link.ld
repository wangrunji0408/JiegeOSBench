OUTPUT_ARCH(riscv)
ENTRY(_start)

BASE = 0x80200000;

SECTIONS
{
    . = BASE;

    .text : {
        *(.text.init)
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    .data : {
        *(.data .data.*)
    }

    __bss_start = .;
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }
    __bss_end = .;

    . = ALIGN(16);
    _ebss = .;

    /* kernel heap: reserved in the address space (not loaded from file) */
    . = ALIGN(4K);
    _kernel_heap_start = .;
    . += 64M;
    _kernel_heap_end = .;

    . = ALIGN(4K);
    _kernel_end = .;
}
