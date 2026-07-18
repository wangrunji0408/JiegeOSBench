use super::File;

pub struct Stdin;
pub struct Stdout;

impl File for Stdin {
    fn readable(&self) -> bool {
        true
    }
    fn read(&self, buf: &mut [u8]) -> usize {
        for byte in buf.iter_mut() {
            let mut c: usize;
            loop {
                c = crate::sbi::console_getchar();
                if c != usize::MAX {
                    break;
                }
            }
            *byte = c as u8;
        }
        buf.len()
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl File for Stdout {
    fn writable(&self) -> bool {
        true
    }
    fn write(&self, buf: &[u8]) -> usize {
        for &b in buf {
            crate::sbi::console_putchar(b);
        }
        buf.len()
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}
