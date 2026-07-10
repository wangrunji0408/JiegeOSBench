OUTPUT_ARCH(riscv)
ENTRY(_start)

BASE_ADDRESS = 0x80200000;

SECTIONS
{
    . = BASE_ADDRESS;
    skernel = .;

    .text : ALIGN(4K) {
        KEEP(*(.text.entry))
        *(.text .text.*)
    }
    .rodata : ALIGN(4K) { *(.rodata .rodata.*) }
    .data : ALIGN(4K) { *(.data .data.*) }

    .bss.stack (NOLOAD) : ALIGN(16) {
        *(.bss.stack)
    }
    .bss (NOLOAD) : ALIGN(4K) {
        sbss = .;
        *(.bss .bss.* COMMON)
        ebss = .;
    }

    . = ALIGN(4K);
    ekernel = .;

    /DISCARD/ : { *(.eh_frame) *(.comment) }
}

