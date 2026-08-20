//! 极简 FDT/DTB 解析器：只需要拿到内存大小

const FDT_MAGIC: u32 = 0xd00d_feed;

#[repr(C)]
struct FdtHeader {
    magic: u32,
    totalsize: u32,
    off_struct: u32,
    off_strings: u32,
    off_mem_rsvmap: u32,
    version: u32,
    last_comp_version: u32,
    boot_cpuid: u32,
    size_strings: u32,
    size_struct: u32,
}

fn u32_at(base: usize, off: usize) -> u32 {
    unsafe { ((base + off) as *const u32).read_volatile().swap_bytes() }
}

fn string_at(base: usize, off: usize) -> &'static str {
    unsafe {
        let p = (base + off) as *const u8;
        let mut len = 0usize;
        while p.add(len).read_volatile() != 0 {
            len += 1;
        }
        core::str::from_utf8_unchecked(core::slice::from_raw_parts(p, len))
    }
}

/// 解析 DTB，返回 (内存起始地址, 内存大小)
pub fn parse_memory(dtb_paddr: usize) -> (usize, usize) {
    let base = dtb_paddr;
    assert_eq!(u32_at(base, 0), FDT_MAGIC, "bad dtb magic");

    let off_struct = u32_at(base, 8) as usize;
    let off_strings = u32_at(base, 12) as usize;

    let mut pos = base + off_struct;
    let mut depth = 0usize;
    let mut cur_name = "";
    // #address-cells / #size-cells 按根节点默认 (2,2)
    let mut mem: Option<(usize, usize)> = None;

    loop {
        let token = u32_at(base, pos - base);
        match token {
            0x1 => {
                // FDT_BEGIN_NODE
                pos += 4;
                let name = string_at(base, pos - base);
                cur_name = name;
                pos += name.len();
                pos = (pos + 3) & !3usize; // 4 字节对齐（含结尾 NUL）
                // name 包含 NUL 后需要对齐：先 +1 再对齐
                depth += 1;
            }
            0x2 => {
                // FDT_END_NODE
                pos += 4;
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            0x3 => {
                // FDT_PROP
                let len = u32_at(base, pos - base + 4) as usize;
                let nameoff = u32_at(base, pos - base + 8) as usize;
                let pname = string_at(base, off_strings + nameoff);
                let data_off = pos + 12;
                if pname == "reg" && (cur_name.starts_with("memory@") || cur_name == "memory") {
                    // riscv virt: reg = <u64 start, u64 size>
                    let start = u32_at(base, data_off) as usize as u64
                        | ((u32_at(base, data_off + 4) as u64) << 32);
                    let size = u32_at(base, data_off + 8) as usize as u64
                        | ((u32_at(base, data_off + 12) as u64) << 32);
                    mem = Some((start as usize, size as usize));
                }
                pos = data_off + len;
                pos = (pos + 3) & !3usize;
            }
            0x4 => {
                // FDT_NOP
                pos += 4;
            }
            0x9 => {
                // FDT_END
                break;
            }
            _ => {
                // 不认识的 token，安全退出
                break;
            }
        }
    }

    match mem {
        Some(m) => m,
        None => (0x8000_0000, 128 << 20), // QEMU virt 默认兜底
    }
}
