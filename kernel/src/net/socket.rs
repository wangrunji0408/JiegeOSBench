//! Socket abstraction shared by TCP/UDP (smoltcp) and AF_UNIX sockets.
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::abi::*;
use crate::fs::file::File;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SockAddr {
    Inet { addr: [u8; 4], port: u16 },
    Unix(Vec<u8>),
    Unspec,
}

impl SockAddr {
    /// Parse a sockaddr from raw user bytes.
    pub fn parse(bytes: &[u8]) -> Result<SockAddr, i32> {
        if bytes.len() < 2 {
            return Err(EINVAL);
        }
        let family = u16::from_le_bytes([bytes[0], bytes[1]]);
        match family {
            AF_INET => {
                if bytes.len() < 8 {
                    return Err(EINVAL);
                }
                let port = u16::from_be_bytes([bytes[2], bytes[3]]);
                let addr = [bytes[4], bytes[5], bytes[6], bytes[7]];
                Ok(SockAddr::Inet { addr, port })
            }
            AF_UNIX => {
                let path = &bytes[2..];
                let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
                Ok(SockAddr::Unix(path[..end].to_vec()))
            }
            AF_UNSPEC => Ok(SockAddr::Unspec),
            _ => Err(EAFNOSUPPORT),
        }
    }

    /// Serialise to the Linux sockaddr layout.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            SockAddr::Inet { addr, port } => {
                let mut v = Vec::with_capacity(16);
                v.extend_from_slice(&AF_INET.to_le_bytes());
                v.extend_from_slice(&port.to_be_bytes());
                v.extend_from_slice(addr);
                v.extend_from_slice(&[0u8; 8]);
                v
            }
            SockAddr::Unix(path) => {
                let mut v = Vec::with_capacity(2 + path.len() + 1);
                v.extend_from_slice(&AF_UNIX.to_le_bytes());
                v.extend_from_slice(path);
                v.push(0);
                v
            }
            SockAddr::Unspec => {
                let mut v = alloc::vec![0u8; 16];
                v[0..2].copy_from_slice(&AF_UNSPEC.to_le_bytes());
                v
            }
        }
    }
}

/// Data passed with sendmsg/recvmsg (SCM_RIGHTS).
#[derive(Default)]
pub struct Ancillary {
    pub fds: Vec<Arc<File>>,
}

pub trait SocketOps: Send + Sync {
    fn bind(&self, _addr: SockAddr) -> Result<(), i32> {
        Err(EOPNOTSUPP)
    }
    fn listen(&self, _backlog: i32) -> Result<(), i32> {
        Err(EOPNOTSUPP)
    }
    /// Accept a connection; returns the new socket's FileOps and the peer address.
    fn accept(&self, _nonblock: bool) -> Result<(Arc<dyn crate::fs::file::FileOps>, SockAddr), i32> {
        Err(EOPNOTSUPP)
    }
    fn connect(&self, _addr: SockAddr, _nonblock: bool) -> Result<(), i32> {
        Err(EOPNOTSUPP)
    }
    fn send(&self, buf: &[u8], flags: u32, nonblock: bool, to: Option<SockAddr>, anc: Ancillary) -> SysResult;
    /// Returns (bytes, source address, ancillary).
    fn recv(&self, buf: &mut [u8], flags: u32, nonblock: bool) -> Result<(usize, Option<SockAddr>, Ancillary), i32>;
    fn shutdown(&self, _how: i32) -> Result<(), i32> {
        Ok(())
    }
    fn local_addr(&self) -> Result<SockAddr, i32>;
    fn peer_addr(&self) -> Result<SockAddr, i32> {
        Err(ENOTCONN)
    }
    fn setsockopt(&self, _level: i32, _opt: i32, _val: &[u8]) -> Result<(), i32> {
        Ok(())
    }
    fn getsockopt(&self, level: i32, opt: i32) -> Result<Vec<u8>, i32>;
    fn sock_type(&self) -> u32;
    fn domain(&self) -> u16;
}

pub fn i32_opt(v: i32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}
