OUTPUT_ARCH(riscv)
ENTRY(_start)
BASE_ADDRESS = 0x80200000;

SECTIONS
{
    . = BASE_ADDRESS;
    _kernel_start = .;

    .text : {
        *(.text.entry)
        *(.text .text.*)
    }

    .rodata : {
        *(.srodata .srodata.*)
        *(.rodata .rodata.*)
    }

    .data : {
        *(.sdata .sdata.*)
        *(.data .data.*)
    }

    .bss : {
        _bss_start = .;
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
        _bss_end = .;
    }

    . = ALIGN(0x1000);
    _end = .;
    _kernel_end = .;
}
