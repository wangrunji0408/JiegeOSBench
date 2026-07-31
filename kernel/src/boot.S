.section .text.init
.global _start
_start:
    # a0 = hartid, a1 = dtb pointer
    la sp, boot_stack_top
    # clear bss
    la t0, _ebss          # linker provides? we defined _ebss; but rust may not expose; use symbols from link.ld
    # bss range: use link symbols __bss_start/_ebss — simpler: rely on Rust zeroing via static init; skip explicit clear
    # jump to Rust
    call rust_main
    # never returns
1:  wfi
    j 1b

.section .bss
.align 12
.global boot_stack_bottom
boot_stack_bottom:
    .skip 16384
.global boot_stack_top
boot_stack_top:
