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

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }

    . = ALIGN(16);
    _ebss = .;

    . = ALIGN(4K);
    _kernel_end = .;
}
