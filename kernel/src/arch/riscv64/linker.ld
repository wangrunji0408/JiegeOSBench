OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS {
    . = 0x80200000;

    .text : {
        *(.text.entry)
        *(.text .text.*)
    }

    . = ALIGN(4096);
    .rodata : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }

    . = ALIGN(4096);
    .data : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    . = ALIGN(4096);
    .bss : {
        sbss = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(.bss.stack)
        ebss = .;
    }

    . = ALIGN(4096);
    ekernel = .;

    /DISCARD/ : {
        *(.eh_frame)
    }
}
