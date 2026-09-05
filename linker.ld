OUTPUT_ARCH(riscv)
ENTRY(_start)
SECTIONS {
 . = 0x80200000;
 .text : { KEEP(*(.text.entry)) *(.text .text.*) }
 . = ALIGN(4096);
 .rodata : { *(.rodata .rodata.*) }
 . = ALIGN(4096);
 .data : { *(.data .data.*) }
 . = ALIGN(4096);
 .bss : { sbss = .; *(.bss .bss.*) *(COMMON) ebss = .; }
 . = ALIGN(4096);
 kernel_end = .;
 /DISCARD/ : { *(.eh_frame) }
}
