#!/usr/bin/env python3
"""Fetch unmodified official Alpine riscv64 binary packages, recording provenance."""
import tarfile, pathlib, urllib.request, hashlib, json, os
base=pathlib.Path(__file__).resolve().parents[1]
root=base/'rootfs'; vendor=base/'vendor'
index={}
for repo in ['main','community']:
 with tarfile.open(vendor/f'APKINDEX-{repo}.tar.gz') as t:
  for block in t.extractfile('APKINDEX').read().decode().split('\n\n'):
   d=dict(line.split(':',1) for line in block.splitlines() if ':' in line)
   if 'P' in d: index[d['P']]=(repo,d)
manifest=[]
for name in ['musl','libcrypto3','libssl3','pcre2','zlib','nginx']:
 repo,d=index[name]; fn=f"{name}-{d['V']}.apk"
 url=f'https://dl-cdn.alpinelinux.org/alpine/edge/{repo}/riscv64/{fn}'
 dest=vendor/fn
 if not dest.exists(): urllib.request.urlretrieve(url,dest)
 with tarfile.open(dest,ignore_zeros=True) as t:
  for m in t:
   if m.name.startswith('.') or not (m.isfile() or m.issym() or m.isdir() or m.islnk()): continue
   target=root/m.name; target.parent.mkdir(parents=True,exist_ok=True)
   if m.isdir(): target.mkdir(exist_ok=True)
   elif m.issym():
    if not target.is_symlink(): target.symlink_to(m.linkname)
   elif m.isfile(): target.write_bytes(t.extractfile(m).read())
 manifest.append({'name':name,'version':d['V'],'url':url,'sha256':hashlib.sha256(dest.read_bytes()).hexdigest()})
manifest.append({'file':'rootfs/usr/sbin/nginx','sha256':hashlib.sha256((root/'usr/sbin/nginx').read_bytes()).hexdigest(),'modified':False})
(vendor/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps(manifest,indent=2))
