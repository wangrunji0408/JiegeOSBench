#!/usr/bin/env python3
"""Extract the bootable kernel image out of the Ubuntu riscv64 linux-image deb.

Ubuntu's riscv64 /boot/vmlinuz-<ver>-generic is simultaneously a valid PE32+
EFI application and a valid flat RISC-V Linux Image (the first instruction
decodes to "MZ" so that UEFI accepts it, and the RISC-V image header with the
0x05435352 magic sits at offset 0x38).  QEMU's -kernel loader accepts the flat
image form, so we just copy the member out and verify its header.

Writes ref/.build/vmlinuz-<ver> and prints the parsed RISC-V image header.
"""
import glob
import os
import struct
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
DEBS = os.path.join(HERE, "debs")
BUILD = os.path.join(ROOT, "ref", ".build")

RISCV_MAGIC = 0x05435352


def find_image_deb():
    pats = sorted(glob.glob(os.path.join(DEBS, "linux-image-*-generic_*.deb")))
    # prefer the versioned image package over the linux-image-generic meta
    cands = [p for p in pats if not os.path.basename(p).startswith("linux-image-generic_")]
    if not cands:
        sys.exit("no linux-image-*-generic deb in %s; run tools/ref_fetch_pkgs.py" % DEBS)
    return cands[-1]


def main():
    deb = find_image_deb()
    members = subprocess.run(["ar", "t", deb], capture_output=True, text=True,
                             check=True).stdout.split()
    data = [m for m in members if m.startswith("data.tar")][0]
    tmp = "/tmp/_ref_kernel_data.tar"
    with open(tmp, "wb") as fh:
        subprocess.run(["ar", "p", deb, data], stdout=fh, check=True)
    listing = subprocess.run(["tar", "-tf", tmp], capture_output=True, text=True,
                             check=True).stdout.split()
    vmlinuz = [m for m in listing if m.endswith("vmlinuz-" + os.path.basename(deb).split("_")[1].split("~")[0])
               or ("/boot/vmlinuz-" in m)]
    if not vmlinuz:
        vmlinuz = [m for m in listing if m.endswith(".vmlinuz") or "vmlinuz" in m]
    member = vmlinuz[0].lstrip("./")
    os.makedirs(BUILD, exist_ok=True)
    out = os.path.join(BUILD, os.path.basename(member))
    subprocess.check_call(["tar", "-xf", tmp, "-C", BUILD, "./" + member])
    extracted = os.path.join(BUILD, member)
    if extracted != out:
        os.replace(extracted, out)
    os.unlink(tmp)

    with open(out, "rb") as fh:
        head = fh.read(0x40)
    kind = subprocess.run(["file", "-b", out], capture_output=True, text=True).stdout.strip()
    print("kernel deb   :", os.path.basename(deb))
    print("vmlinuz      :", out, "(%d bytes)" % os.path.getsize(out))
    print("file(1)      :", kind)
    print("first 8 bytes:", head[:8].hex())
    magic = struct.unpack_from("<I", head, 0x38)[0]
    if magic == RISCV_MAGIC:
        code0, code1 = struct.unpack_from("<II", head, 0x00)
        text_offset, image_size = struct.unpack_from("<QQ", head, 0x08)
        res2 = struct.unpack_from("<Q", head, 0x20)[0]
        print("RISC-V image header OK: code0=0x%08x code1=0x%08x "
              "text_offset=0x%x image_size=0x%x res2=%d"
              % (code0, code1, text_offset, image_size, res2))
        print("kernel release:", kernel_release_from_name(out))
    else:
        print("WARNING: no RISC-V image magic at 0x38 (magic=0x%08x)" % magic)
    print(out)


def kernel_release_from_name(path):
    b = os.path.basename(path)
    return b.split("vmlinuz-")[-1] if "vmlinuz-" in b else b


if __name__ == "__main__":
    main()
