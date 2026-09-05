pub unsafe fn init(){}
pub fn poll(){}
pub fn socket()->usize{0}
pub fn bind(_:usize,_:&[u8])->isize{0}
pub fn listen(_:usize,_:usize)->isize{0}
pub fn accept(_:usize)->Option<(usize,[u8;4],u16)>{None}
pub fn address(_:usize,_:bool)->([u8;4],u16){([10,0,2,15],8080)}
pub fn close(_:usize){}
pub fn recv(_:usize,_:&mut[u8])->isize{-11}
pub fn send(_:usize,_:&[u8])->isize{-11}
pub fn ready(_:usize)->u32{0}
