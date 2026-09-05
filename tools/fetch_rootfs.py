#!/usr/bin/env python3
"""Restore the pinned official Alpine packages recorded in vendor/manifest.json."""
import hashlib,json,pathlib,tarfile,urllib.request
base=pathlib.Path(__file__).resolve().parents[1]
root=base/'rootfs';vendor=base/'vendor'
manifest=json.loads((vendor/'manifest.json').read_text())
for item in manifest:
 if 'url' not in item:continue
 dest=vendor/item['url'].rsplit('/',1)[-1]
 if not dest.exists():urllib.request.urlretrieve(item['url'],dest)
 assert hashlib.sha256(dest.read_bytes()).hexdigest()==item['sha256'],f'Package checksum mismatch: {dest}'
 with tarfile.open(dest,ignore_zeros=True)as package:
  for m in package:
   parts=pathlib.PurePosixPath(m.name).parts
   if not parts or m.name.startswith('.') or '..' in parts or m.name.startswith('/'):continue
   target=root/m.name;target.parent.mkdir(parents=True,exist_ok=True)
   if m.isdir():target.mkdir(exist_ok=True)
   elif m.issym():
    if not target.exists() and not target.is_symlink():target.symlink_to(m.linkname)
   elif m.isfile():
    content=package.extractfile(m).read()
    if target.exists():
     assert target.read_bytes()==content,f'Existing file differs; refusing to overwrite: {target}'
    else:target.write_bytes(content)
print('Restored pinned packages; nginx and shared libraries are unmodified.')
