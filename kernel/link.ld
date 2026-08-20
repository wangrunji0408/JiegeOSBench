OUTPUT_ARCH(riscv)
ENTRY(_start)

BASE_ADDRESS = 0x80200000;

SECTIONS
{
    . = BASE_ADDRESS;

    .text : {
        *(.text.init)
        *(.text .text.*)
    }

    .rodata : ALIGN(8) {
        __rodata_start = .;
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        . = ALIGN(8);
    }

    .data : ALIGN(8) {
        __global_pointer$ = . + 0x800;
        *(.data .data.*)
        *(.sdata .sdata.*)
        . = ALIGN(8);
    }

    .bss : ALIGN(8) {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        . = ALIGN(8);
        __bss_end = .;
    }

    . = ALIGN(0x1000);
    __kernel_end = .;

    .bootstack (NOLOAD) : {
        _boot_stack_bottom = .;
        . += 0x10000;
        _boot_stack_top = .;
    }

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
        *(.note .note.*)
        *(.comment)
    }
}
