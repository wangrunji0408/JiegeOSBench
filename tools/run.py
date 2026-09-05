#!/usr/bin/env python3
"""Build, start, stop or run iJiege; all artifacts stay in this workspace."""
import argparse, os, pathlib, signal, subprocess, sys, time
ROOT=pathlib.Path(__file__).resolve().parents[1]
BUILD=ROOT/'build'
KERNEL=ROOT/'target/riscv64gc-unknown-none-elf/release/ijiege'
def command(port=8080,bind='127.0.0.1'):
 return ['qemu-system-riscv64','-machine','virt','-m','256M','-smp','1','-nographic','-bios','default','-kernel',str(KERNEL),'-global','virtio-mmio.force-legacy=false','-device','virtio-net-device,netdev=net0,mac=52:54:00:12:34:56','-netdev',f'user,id=net0,hostfwd=tcp:{bind}:{port}-:8080']
def build(trace=False):
 subprocess.run([sys.executable,str(ROOT/'tools/verify_binary.py')],cwd=ROOT,check=True)
 subprocess.run(['cargo','build','--release','--locked']+(['--features','trace'] if trace else []),cwd=ROOT,check=True)
def main():
 parser=argparse.ArgumentParser(description=__doc__)
 parser.add_argument('action',choices=['run','start','stop','build'],nargs='?',default='run')
 parser.add_argument('--port',type=int,default=8080)
 parser.add_argument('--bind',default='127.0.0.1')
 parser.add_argument('--trace',action='store_true')
 args=parser.parse_args();BUILD.mkdir(exist_ok=True);pidfile=BUILD/'qemu.pid'
 if args.action=='stop':
  if pidfile.exists():
   pid=int(pidfile.read_text())
   try:
    cmd=subprocess.check_output(['ps','-p',str(pid),'-o','command='],text=True)
    if 'qemu-system-riscv64' not in cmd or str(KERNEL) not in cmd:raise RuntimeError('PID no longer belongs to this kernel')
    os.kill(pid,signal.SIGTERM)
   except subprocess.CalledProcessError:pass
   pidfile.unlink()
  return
 if args.action=='start' and pidfile.exists():
  try:os.kill(int(pidfile.read_text()),0)
  except ProcessLookupError:pidfile.unlink()
  else:raise SystemExit('This workspace already has a running QEMU; use stop first.')
 build(args.trace)
 if args.action=='build':return
 if args.action=='run':os.execvp(command(args.port,args.bind)[0],command(args.port,args.bind))
 with (BUILD/'qemu.log').open('wb') as log:
  p=subprocess.Popen(command(args.port,args.bind),cwd=ROOT,stdin=subprocess.DEVNULL,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
 pidfile.write_text(str(p.pid)+'\n');time.sleep(.5)
 if p.poll() is not None:raise SystemExit((BUILD/'qemu.log').read_text())
 print(f'QEMU PID {p.pid}: http://{args.bind}:{args.port}/\nLog: {BUILD}/qemu.log')
if __name__=='__main__':main()
