use crate::mm::translated_byte_buffer;
use crate::task::{current_task, current_user_token};

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let file = {
        let inner = task.inner_lock();
        match inner.fd_table.get(fd).and_then(|f| f.clone()) {
            Some(f) => f,
            None => return -9, // EBADF
        }
    };
    if !file.writable() {
        return -13; // EACCES
    }
    let buffers = translated_byte_buffer(token, buf, len);
    let mut written = 0;
    for b in buffers {
        written += file.write(b);
    }
    written as isize
}
