use crate::{memory,fs};
fn u16at(d:&[u8],o:usize)->usize{u16::from_le_bytes(d[o..o+2].try_into().unwrap()) as usize}
fn u64at(d:&[u8],o:usize)->usize{u64::from_le_bytes(d[o..o+8].try_into().unwrap()) as usize}
pub unsafe fn load_image(d:&[u8],base:usize)->(usize,usize,usize,usize){
 assert!(&d[..4]==b"\x7fELF");let phoff=u64at(d,32);let phsz=u16at(d,54);let phnum=u16at(d,56);
 for i in 0..phnum{let p=phoff+i*phsz;let ty=u32::from_le_bytes(d[p..p+4].try_into().unwrap());if ty==1{
 let off=u64at(d,p+8);let va=base+u64at(d,p+16);let filesz=u64at(d,p+32);let memsz=u64at(d,p+40);
 memory::map(va,memsz);core::ptr::copy_nonoverlapping(d[off..off+filesz].as_ptr(),va as *mut u8,filesz);
 }}
 (base+u64at(d,24),base+phoff,phsz,phnum)
}
pub unsafe fn load_program(path:&str,args:&[&str])->(usize,usize){
 let prog=fs::file_data(path).unwrap();let (entry,phdr,phent,phnum)=load_image(&prog,0x400000);
 let ld=fs::file_data("/lib/ld-musl-riscv64.so.1").unwrap();let ld_base=0x30000000;let (pc,_,_,_)=load_image(&ld,ld_base);
 let top=0x3ff00000;memory::map(top-2*1024*1024,2*1024*1024);let mut sp=top;
 let mut push_str=|s:&str|{sp-=s.len()+1;core::ptr::copy_nonoverlapping(s.as_ptr(),sp as *mut u8,s.len());*( (sp+s.len()) as *mut u8)=0;sp};
 let argptrs:alloc::vec::Vec<usize>=args.iter().map(|a|push_str(a)).collect();
 let envs=[push_str("PATH=/usr/sbin:/usr/bin:/bin"),push_str("HOME=/"),push_str("LANG=C")];
 let execfn=push_str(path);sp-=16;let rand=sp;for i in 0..16{*((rand+i) as *mut u8)=(i*17+42) as u8;}
 let aux=[(3,phdr),(4,phent),(5,phnum),(6,4096),(7,ld_base),(8,0),(9,entry),(11,0),(12,0),(13,0),(14,0),(16,0x112d),(17,100),(23,0),(25,rand),(31,execfn),(0,0)];
 let mut words=alloc::vec![args.len()];words.extend(argptrs);words.push(0);words.extend(envs);words.push(0);for (a,b) in aux{words.push(a);words.push(b);}
 sp=(sp-words.len()*8)&!15;core::ptr::copy_nonoverlapping(words.as_ptr(),sp as *mut usize,words.len());
 (pc,sp)
}
