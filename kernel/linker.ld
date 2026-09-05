OUTPUT_ARCH(riscv)
ENTRY(_start)
BASE_ADDRESS = 0x80200000;

SECTIONS
{
    . = BASE_ADDRESS;
    __kernel_start = .;
    .text : {
        __text_start = .;
        *(.text.entry)
        *(.text .text.*)
        __text_end = .;
    }
    . = ALIGN(4K);
    .rodata : {
        __rodata_start = .;
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        __rodata_end = .;
    }
    . = ALIGN(4K);
    .data : {
        __data_start = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        __data_end = .;
    }
    . = ALIGN(4K);
    .bss : {
        __bss_start = .;
        *(.bss.stack)
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        __bss_end = .;
    }
    . = ALIGN(4K);
    __kernel_end = .;
    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
    }
}
