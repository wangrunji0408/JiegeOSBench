#!/usr/bin/env python3
"""Boot the Rust kernel in a fresh QEMU and test nginx over real TCP forwarding."""
import concurrent.futures,hashlib,http.client,json,pathlib,subprocess,sys,time
from run import ROOT,BUILD,command,build

def main():
 BUILD.mkdir(exist_ok=True);build()
 logpath=BUILD/'test-qemu.log';results=[]
 with logpath.open('wb')as log:
  qemu=subprocess.Popen(command(18080),cwd=ROOT,stdin=subprocess.DEVNULL,stdout=log,stderr=subprocess.STDOUT)
  try:
   def conn():return http.client.HTTPConnection('127.0.0.1',18080,timeout=15)
   def request(path='/',method='GET',headers=None,c=None):
    own=c is None;c=c or conn();c.request(method,path,headers=headers or {});r=c.getresponse();body=r.read();status=r.status;hs=dict(r.getheaders())
    if own:c.close()
    return status,hs,body
   for _ in range(100):
    if qemu.poll()is not None:raise RuntimeError(logpath.read_text())
    try:
     status,headers,body=request()
     if status==200:break
    except (OSError,http.client.HTTPException):time.sleep(.1)
   else:raise RuntimeError('nginx did not become ready')
   expected=(ROOT/'rootfs/www/index.html').read_bytes()
   assert body==expected and headers['Server']=='nginx/1.30.4'
   results.append('GET /: 200, nginx/1.30.4, exact index.html bytes')
   (BUILD/'response.headers').write_text('HTTP/1.1 200 OK\n'+''.join(f'{k}: {v}\n'for k,v in headers.items()))
   (BUILD/'response.html').write_bytes(body)
   status,h,b=request(method='HEAD');assert status==200 and b==b''and int(h['Content-Length'])==len(expected)
   results.append('HEAD /: correct headers, empty body')
   status,h,b=request('/does-not-exist');assert status==404 and b'nginx/1.30.4'in b
   results.append('Missing file: genuine nginx 404 page')
   status,h,b=request(headers={'Range':'bytes=10-99'});assert status==206 and b==expected[10:100]
   results.append('Range: 206, correct Content-Range and exact bytes')
   status,h,b=request(headers={'If-None-Match':headers['ETag']});assert status==304 and not b
   results.append('Conditional GET: 304 for matching ETag')
   status,h,b=request('/health');assert status==200 and json.loads(b)=={'server':'nginx','kernel':'ijiege','arch':'riscv64'}
   results.append('nginx return directive: /health JSON')
   payload=(ROOT/'rootfs/www/payload.bin').read_bytes()
   status,h,b=request('/payload.bin');assert status==200 and b==payload
   results.append('sendfile: 1 MiB exact binary transfer (exceeds TCP buffer)')
   c=conn()
   for _ in range(25):
    status,h,b=request(c=c);assert status==200 and b==expected
   c.close();results.append('Keep-alive: 25 sequential requests on one connection')
   def worker(i):
    c=conn()
    try:
     for j in range(5):
      path='/payload.bin'if j==0 else '/'
      status,h,b=request(path,c=c);assert status==200 and b==(payload if j==0 else expected)
    finally:c.close()
   with concurrent.futures.ThreadPoolExecutor(max_workers=8)as pool:list(pool.map(worker,range(24)))
   results.append('Concurrency: 24 connections × 5 requests, 8 simultaneous clients, including 24 MiB binary data')
   status,h,b=request();assert status==200 and b==expected
   results.append('Server remains healthy after all tests')
   text=logpath.read_text()
   assert '[fault]'not in text and 'PANIC'not in text and '[emerg]'not in text and '[alert]'not in text,text[-4000:]
   report={'result':'PASS','checks':results,'kernel_sha256':hashlib.sha256(pathlib.Path(command()[command().index('-kernel')+1]).read_bytes()).hexdigest(),'nginx_sha256':hashlib.sha256((ROOT/'rootfs/usr/sbin/nginx').read_bytes()).hexdigest(),'qemu_log':str(logpath)}
   (BUILD/'test-results.json').write_text(json.dumps(report,indent=2)+'\n')
   for result in results:print('PASS:',result)
  finally:
   qemu.terminate()
   try:qemu.wait(timeout=5)
   except subprocess.TimeoutExpired:qemu.kill();qemu.wait()
if __name__=='__main__':main()
