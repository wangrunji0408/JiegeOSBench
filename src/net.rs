use alloc::{vec,vec::Vec};
use core::{ptr::{read_volatile as read,write_volatile as write},sync::atomic::{fence,Ordering}};
use smoltcp::{phy::{Device,DeviceCapabilities,Medium,RxToken,TxToken},iface::{Config,Interface,SocketSet,SocketHandle},socket::tcp,wire::{EthernetAddress,IpAddress,IpCidr,Ipv4Address},time::Instant};
const Q:usize=64;
const HEADER:usize=12;
struct Queue{desc:usize,avail:usize,used:usize,buffers:Vec<usize>,seen:u16,index:u16}
impl Queue{
 unsafe fn new(base:usize,num:u32,rx:bool)->Self{
  mmw(base,0x30,num);assert!(mmr(base,0x34)>=Q as u32);mmw(base,0x38,Q as u32);
  let desc=crate::memory::page();let avail=crate::memory::page();let used=crate::memory::page();
  let mut buffers=Vec::new();for i in 0..Q{let b=crate::memory::page();buffers.push(b);write((desc+i*16)as *mut u64,b as u64);write((desc+i*16+8)as *mut u32,2048);write((desc+i*16+12)as *mut u16,if rx{2}else{0});if rx{write((avail+4+i*2)as *mut u16,i as u16);}}
  let index=if rx{Q as u16}else{0};write((avail+2)as *mut u16,index);write(avail as *mut u16,1);
  for(off,p)in[(0x80,desc),(0x90,avail),(0xa0,used)]{mmw(base,off,p as u32);mmw(base,off+4,(p>>32)as u32);}
  mmw(base,0x44,1);Self{desc,avail,used,buffers,seen:0,index}
 }
}
struct Virtio{base:usize,rx:Queue,tx:Queue,rx_count:u64,tx_count:u64}
unsafe fn mmr(b:usize,o:usize)->u32{read((b+o)as *const u32)}
unsafe fn mmw(b:usize,o:usize,v:u32){write((b+o)as *mut u32,v)}
impl Virtio{
 unsafe fn new()->Self{
  let base=(0x10001000..=0x10008000).step_by(4096).find(|b|mmr(*b,0)==0x74726976&&mmr(*b,8)==1).expect("virtio-net MMIO device missing");
  assert_eq!(mmr(base,4),2,"requires modern virtio-mmio");mmw(base,0x70,0);mmw(base,0x70,1);mmw(base,0x70,3);
  mmw(base,0x14,0);let low=mmr(base,0x10);mmw(base,0x14,1);assert!(mmr(base,0x10)&1!=0);
  mmw(base,0x24,0);mmw(base,0x20,low & (1<<5));mmw(base,0x24,1);mmw(base,0x20,1);
  mmw(base,0x70,11);assert!(mmr(base,0x70)&8!=0);
  let rx=Queue::new(base,0,true);let tx=Queue::new(base,1,false);mmw(base,0x70,15);fence(Ordering::SeqCst);mmw(base,0x50,0);
  crate::println!("[virtio] net MMIO={:#x}, queues={}, features=VERSION_1|MAC",base,Q);
  Self{base,rx,tx,rx_count:0,tx_count:0}
 }
 unsafe fn receive_frame(&mut self)->Option<Vec<u8>>{
  let q=&mut self.rx;let used=read((q.used+2)as *const u16);if used==q.seen{return None}fence(Ordering::SeqCst);
  let e=q.used+4+(q.seen as usize%Q)*8;let id=read(e as *const u32)as usize;let len=read((e+4)as *const u32)as usize;assert!(id<Q&&len<=2048);
  let data=if len>=HEADER{core::slice::from_raw_parts((q.buffers[id]+HEADER)as *const u8,len-HEADER).to_vec()}else{Vec::new()};
  q.seen=q.seen.wrapping_add(1);write((q.avail+4+(q.index as usize%Q)*2)as *mut u16,id as u16);fence(Ordering::SeqCst);q.index=q.index.wrapping_add(1);write((q.avail+2)as *mut u16,q.index);fence(Ordering::SeqCst);mmw(self.base,0x50,0);
  let irq=mmr(self.base,0x60);if irq!=0{mmw(self.base,0x64,irq);}self.rx_count+=1;Some(data)
 }
 unsafe fn send_frame(&mut self,data:&[u8]){
  let q=&mut self.tx;assert!(data.len()+HEADER<=2048);
  let start=crate::millis();while read((q.used+2)as *const u16)!=q.index{assert!(crate::millis()-start<1000,"virtio TX timeout");core::hint::spin_loop();}
  let id=q.index as usize%Q;core::ptr::write_bytes(q.buffers[id]as *mut u8,0,HEADER);core::ptr::copy_nonoverlapping(data.as_ptr(),(q.buffers[id]+HEADER)as *mut u8,data.len());
  write((q.desc+id*16+8)as *mut u32,(data.len()+HEADER)as u32);write((q.avail+4+id*2)as *mut u16,id as u16);fence(Ordering::SeqCst);q.index=q.index.wrapping_add(1);write((q.avail+2)as *mut u16,q.index);fence(Ordering::SeqCst);mmw(self.base,0x50,1);self.tx_count+=1;
 }
}
struct Rx(Vec<u8>);struct Tx<'a>(&'a mut Virtio);
impl RxToken for Rx{fn consume<R,F>(self,f:F)->R where F:FnOnce(&[u8])->R{f(&self.0)}}
impl TxToken for Tx<'_>{fn consume<R,F>(self,len:usize,f:F)->R where F:FnOnce(&mut[u8])->R{let mut b=vec![0;len];let r=f(&mut b);unsafe{self.0.send_frame(&b);}r}}
impl Device for Virtio{
 type RxToken<'a>=Rx;type TxToken<'a>=Tx<'a>;
 fn receive(&mut self,_:Instant)->Option<(Rx,Tx<'_>)>{unsafe{self.receive_frame().map(|p|(Rx(p),Tx(self)))}}
 fn transmit(&mut self,_:Instant)->Option<Tx<'_>>{Some(Tx(self))}
 fn capabilities(&self)->DeviceCapabilities{let mut c=DeviceCapabilities::default();c.medium=Medium::Ethernet;c.max_transmission_unit=1514;c.max_burst_size=Some(1);c}
}
#[derive(Clone)]enum Endpoint{New(u16),Listener(u16,Vec<SocketHandle>),Stream(SocketHandle)}
struct Network{dev:Virtio,iface:Interface,sockets:SocketSet<'static>,ends:Vec<Option<Endpoint>>,closing:Vec<SocketHandle>}
static mut NET:Option<Network>=None;
unsafe fn net()->&'static mut Network{NET.as_mut().unwrap()}
fn now()->Instant{Instant::from_millis(crate::millis())}
pub unsafe fn init(){
 let mut dev=Virtio::new();let mut cfg=Config::new(EthernetAddress([0x52,0x54,0x00,0x12,0x34,0x56]).into());cfg.random_seed=crate::ticks();
 let mut iface=Interface::new(cfg,&mut dev,now());iface.update_ip_addrs(|ips|{ips.push(IpCidr::new(IpAddress::v4(10,0,2,15),24)).unwrap();});iface.routes_mut().add_default_ipv4_route(Ipv4Address::new(10,0,2,2)).unwrap();
 NET=Some(Network{dev,iface,sockets:SocketSet::new(Vec::new()),ends:Vec::new(),closing:Vec::new()});crate::println!("[net] Ethernet / IPv4 / TCP ready: 10.0.2.15");
}
pub fn poll(){unsafe{let n=net();n.iface.poll(now(),&mut n.dev,&mut n.sockets);n.closing.retain(|h|{if n.sockets.get::<tcp::Socket>(*h).state()==tcp::State::Closed{n.sockets.remove(*h);false}else{true}});}}
fn add(n:&mut Network,e:Endpoint)->usize{if let Some(i)=n.ends.iter().position(Option::is_none){n.ends[i]=Some(e);i}else{n.ends.push(Some(e));n.ends.len()-1}}
fn tcp_socket(n:&mut Network,port:u16)->SocketHandle{let mut s=tcp::Socket::new(tcp::SocketBuffer::new(vec![0;32768]),tcp::SocketBuffer::new(vec![0;32768]));s.set_nagle_enabled(false);s.listen(port).unwrap();n.sockets.add(s)}
pub fn socket()->usize{unsafe{add(net(),Endpoint::New(0))}}
pub fn bind(s:usize,a:&[u8])->isize{if a.len()<8{return -22}let port=u16::from_be_bytes([a[2],a[3]]);unsafe{net().ends[s]=Some(Endpoint::New(port));}0}
pub fn listen(s:usize,backlog:usize)->isize{unsafe{let n=net();let Some(Endpoint::New(port))=n.ends[s]else{return -22};let mut handles=Vec::new();for _ in 0..backlog.clamp(1,16){handles.push(tcp_socket(n,port));}n.ends[s]=Some(Endpoint::Listener(port,handles));crate::println!("[net] listening on 10.0.2.15:{}",port);0}}
pub fn accept(s:usize)->Option<(usize,[u8;4],u16)>{poll();unsafe{let n=net();let Some(Endpoint::Listener(port,mut hs))=n.ends[s].clone()else{return None};let i=hs.iter().position(|h|matches!(n.sockets.get::<tcp::Socket>(*h).state(),tcp::State::Established|tcp::State::CloseWait))?;let h=hs[i];let r=n.sockets.get::<tcp::Socket>(h).remote_endpoint()?;let IpAddress::Ipv4(ip)=r.addr;hs[i]=tcp_socket(n,port);n.ends[s]=Some(Endpoint::Listener(port,hs));let fd=add(n,Endpoint::Stream(h));Some((fd,ip.octets(),r.port))}}
pub fn address(s:usize,peer:bool)->([u8;4],u16){unsafe{let n=net();match n.ends[s].as_ref().unwrap(){Endpoint::New(p)|Endpoint::Listener(p,_) =>([10,0,2,15],*p),Endpoint::Stream(h)=>{let t=n.sockets.get::<tcp::Socket>(*h);let e=if peer{t.remote_endpoint()}else{t.local_endpoint()}.unwrap();let IpAddress::Ipv4(ip)=e.addr;(ip.octets(),e.port)}}}}
pub fn close(s:usize){unsafe{let n=net();if let Some(e)=n.ends[s].take(){match e{Endpoint::Stream(h)=>{n.sockets.get_mut::<tcp::Socket>(h).close();n.closing.push(h);},Endpoint::Listener(_,hs)=>{for h in hs{n.sockets.remove(h);}},_=>{}}}}poll();}
pub fn recv(s:usize,b:&mut[u8])->isize{poll();unsafe{let n=net();let Some(Endpoint::Stream(h))=n.ends[s]else{return -107};let t=n.sockets.get_mut::<tcp::Socket>(h);if t.can_recv(){t.recv_slice(b).map(|l|l as isize).unwrap_or(-11)}else if !t.may_recv(){0}else{-11}}}
pub fn send(s:usize,b:&[u8])->isize{unsafe{let n=net();let Some(Endpoint::Stream(h))=n.ends[s]else{return -107};let t=n.sockets.get_mut::<tcp::Socket>(h);let r=if t.can_send(){t.send_slice(b).map(|l|l as isize).unwrap_or(-11)}else{-11};poll();r}}
pub fn ready(s:usize)->u32{unsafe{let n=net();match n.ends[s].as_ref(){Some(Endpoint::Listener(_,hs))=>if hs.iter().any(|h|matches!(n.sockets.get::<tcp::Socket>(*h).state(),tcp::State::Established|tcp::State::CloseWait)){1}else{0},Some(Endpoint::Stream(h))=>{let t=n.sockets.get::<tcp::Socket>(*h);let mut r=0;if t.can_recv()||!t.may_recv(){r|=1;}if t.can_send(){r|=4;}if !t.may_recv(){r|=0x2000;}r},_=>0}}}
