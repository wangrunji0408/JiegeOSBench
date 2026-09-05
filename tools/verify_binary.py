#!/usr/bin/env python3
"""Verify every downloaded package and byte-for-byte nginx package provenance."""
import hashlib,json,pathlib,tarfile
root=pathlib.Path(__file__).resolve().parents[1]
manifest=json.loads((root/'vendor/manifest.json').read_text())
for item in manifest:
 path=root/item['file'] if 'file' in item else root/'vendor'/item['url'].rsplit('/',1)[-1]
 actual=hashlib.sha256(path.read_bytes()).hexdigest()
 assert actual==item['sha256'],f'Checksum mismatch: {path}'
nginx=next(x for x in manifest if x.get('name')=='nginx')
with tarfile.open(root/'vendor'/nginx['url'].rsplit('/',1)[-1],ignore_zeros=True)as package:
 original=package.extractfile('usr/sbin/nginx').read()
assert original==(root/'rootfs/usr/sbin/nginx').read_bytes(),'nginx ELF has been modified'
assert original[18:20]==b'\xf3\x00','not a RISC-V ELF'
print(f'PASS: original Alpine nginx {nginx["version"]} RISC-V ELF is byte-identical; all package SHA-256 checks pass.')
