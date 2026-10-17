#!/usr/bin/env python3
import os, stat, sys

def cpio_newc(root, out):
    entries = []
    ino = 300000
    # collect
    def walk(dirpath, rel):
        nonlocal ino
        names = sorted(os.listdir(dirpath))
        for n in names:
            if n.startswith('._') or n == '.DS_Store':
                continue
            p = os.path.join(dirpath, n)
            r = (rel + '/' + n) if rel else n
            st = os.lstat(p)
            ino += 1
            if stat.S_ISLNK(st.st_mode):
                target = os.readlink(p)
                entries.append((r, ino, st.st_mode, target.encode(), b''))
            elif stat.S_ISDIR(st.st_mode):
                entries.append((r, ino, st.st_mode, b'', b''))
                walk(p, r)
            else:
                data = open(p, 'rb').read()
                entries.append((r, ino, st.st_mode, b'', data))
    walk(root, '')
    def hdr(name, ino, mode, nlink, size, devmajor=0, devminor=0, rdevmajor=0, rdevminor=0, namesize=None):
        nm = name.encode() + b'\0'
        fields = [ino, mode, 0, 0, nlink, 0, size, devmajor, devminor, rdevmajor, rdevminor, len(nm), 0]
        h = '070701' + ''.join('%08x' % f for f in fields)
        return h.encode() + nm
    with open(out, 'wb') as f:
        def pad4():
            p = f.tell()
            if p % 4: f.write(b'\0' * (4 - p % 4))
        # root dir entry
        f.write(hdr('.', 300000, stat.S_IFDIR | 0o755, 3, 0))
        pad4()
        for name, i, mode, content, data in entries:
            if stat.S_ISLNK(mode):
                f.write(hdr(name, i, mode, 1, len(content))); pad4()
                f.write(content); pad4()
            elif stat.S_ISDIR(mode):
                f.write(hdr(name, i, mode, 2, 0)); pad4()
            else:
                f.write(hdr(name, i, mode, 1, len(data))); pad4()
                f.write(data); pad4()
        f.write(hdr('TRAILER!!!', 0, 0, 1, 0)); pad4()
    print(f"wrote {out}: {len(entries)} entries, {os.path.getsize(out)} bytes")

if __name__ == '__main__':
    cpio_newc(sys.argv[1], sys.argv[2])
