//! Socket address conversion between the Linux `sockaddr` ABI and smoltcp types.

use crate::fs::{self, Result};
use crate::mm::uaccess;
use alloc::vec::Vec;
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint, Ipv4Address};

pub const AF_UNSPEC: u16 = 0;
pub const AF_UNIX: u16 = 1;
pub const AF_INET: u16 = 2;
pub const AF_INET6: u16 = 10;
pub const AF_NETLINK: u16 = 16;
pub const AF_PACKET: u16 = 17;

/// `struct sockaddr_in`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SockAddrIn {
    pub family: u16,
    /// Port in network byte order.
    pub port: u16,
    /// Address in network byte order.
    pub addr: u32,
    pub zero: [u8; 8],
}

/// `struct sockaddr_in6`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SockAddrIn6 {
    pub family: u16,
    pub port: u16,
    pub flowinfo: u32,
    pub addr: [u8; 16],
    pub scope_id: u32,
}

/// `struct sockaddr_un`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SockAddrUn {
    pub family: u16,
    pub path: [u8; 108],
}

/// A parsed socket address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SockAddr {
    /// An IPv4 endpoint. `addr` of `None` means the wildcard (`0.0.0.0`).
    V4 {
        addr: Option<Ipv4Address>,
        port: u16,
    },
    /// An IPv6 endpoint. We accept these so `AF_INET6` sockets can bind, and map
    /// the unspecified address to the v4 wildcard so a dual-stack listener works.
    V6 { addr: [u8; 16], port: u16 },
    /// A Unix domain socket path.
    Unix(alloc::string::String),
}

impl SockAddr {
    /// Read a `sockaddr` from user space.
    pub fn from_user(ptr: usize, len: usize) -> Result<Self> {
        if ptr == 0 || len < 2 {
            crate::bail!(EINVAL);
        }
        let family: u16 = uaccess::read(ptr)?;
        match family {
            AF_INET => {
                if len < core::mem::size_of::<SockAddrIn>() {
                    crate::bail!(EINVAL);
                }
                let sa: SockAddrIn = uaccess::read(ptr)?;
                let port = u16::from_be(sa.port);
                let raw = u32::from_be(sa.addr);
                Ok(SockAddr::V4 {
                    addr: if raw == 0 {
                        None
                    } else {
                        Some(Ipv4Address::from_bytes(&raw.to_be_bytes()))
                    },
                    port,
                })
            }
            AF_INET6 => {
                if len < core::mem::size_of::<SockAddrIn6>() {
                    crate::bail!(EINVAL);
                }
                let sa: SockAddrIn6 = uaccess::read(ptr)?;
                Ok(SockAddr::V6 {
                    addr: sa.addr,
                    port: u16::from_be(sa.port),
                })
            }
            AF_UNIX => {
                let sa: SockAddrUn = uaccess::read(ptr)?;
                let end = sa.path.iter().position(|&b| b == 0).unwrap_or(sa.path.len());
                Ok(SockAddr::Unix(
                    alloc::string::String::from_utf8_lossy(&sa.path[..end]).into_owned(),
                ))
            }
            _ => Err(fs::Error::new(fs::errno::EAFNOSUPPORT)),
        }
    }

    /// Serialize into the bytes user space expects, returning (bytes, full_len).
    pub fn to_bytes(&self) -> (Vec<u8>, usize) {
        match self {
            SockAddr::V4 { addr, port } => {
                let sa = SockAddrIn {
                    family: AF_INET,
                    port: port.to_be(),
                    addr: match addr {
                        Some(a) => u32::from_be_bytes(a.octets()).to_be(),
                        None => 0,
                    },
                    zero: [0; 8],
                };
                let size = core::mem::size_of::<SockAddrIn>();
                let bytes =
                    unsafe { core::slice::from_raw_parts(&sa as *const _ as *const u8, size) };
                (bytes.to_vec(), size)
            }
            SockAddr::V6 { addr, port } => {
                let sa = SockAddrIn6 {
                    family: AF_INET6,
                    port: port.to_be(),
                    flowinfo: 0,
                    addr: *addr,
                    scope_id: 0,
                };
                let size = core::mem::size_of::<SockAddrIn6>();
                let bytes =
                    unsafe { core::slice::from_raw_parts(&sa as *const _ as *const u8, size) };
                (bytes.to_vec(), size)
            }
            SockAddr::Unix(path) => {
                let mut sa = SockAddrUn {
                    family: AF_UNIX,
                    path: [0; 108],
                };
                let b = path.as_bytes();
                let n = b.len().min(107);
                sa.path[..n].copy_from_slice(&b[..n]);
                let size = 2 + n + 1;
                let bytes = unsafe {
                    core::slice::from_raw_parts(
                        &sa as *const _ as *const u8,
                        core::mem::size_of::<SockAddrUn>(),
                    )
                };
                (bytes[..size.min(bytes.len())].to_vec(), size)
            }
        }
    }

    /// Write this address into a user `sockaddr` buffer with a `socklen_t*`.
    pub fn write_to_user(&self, addr_ptr: usize, len_ptr: usize) -> Result<()> {
        if addr_ptr == 0 || len_ptr == 0 {
            return Ok(());
        }
        let (bytes, full_len) = self.to_bytes();
        let user_len: u32 = uaccess::read(len_ptr)?;
        let n = (user_len as usize).min(bytes.len());
        uaccess::write_bytes(addr_ptr, &bytes[..n])?;
        // Linux reports the full length even when truncated.
        uaccess::write(len_ptr, full_len as u32)?;
        Ok(())
    }

    /// The port, for any address kind.
    pub fn port(&self) -> u16 {
        match self {
            SockAddr::V4 { port, .. } | SockAddr::V6 { port, .. } => *port,
            SockAddr::Unix(_) => 0,
        }
    }

    /// Convert to a smoltcp listen endpoint.
    pub fn to_listen_endpoint(&self) -> Result<IpListenEndpoint> {
        match self {
            SockAddr::V4 { addr, port } => Ok(IpListenEndpoint {
                addr: addr.map(IpAddress::Ipv4),
                port: *port,
            }),
            SockAddr::V6 { addr, port } => {
                // Only the unspecified v6 address is meaningful to us; treat it
                // as the wildcard so `[::]:80` listens on our v4 address.
                if addr.iter().all(|&b| b == 0) {
                    Ok(IpListenEndpoint {
                        addr: None,
                        port: *port,
                    })
                } else if addr[..12] == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff] {
                    // v4-mapped.
                    Ok(IpListenEndpoint {
                        addr: Some(IpAddress::Ipv4(Ipv4Address::from_bytes(&addr[12..16]))),
                        port: *port,
                    })
                } else {
                    Err(fs::Error::new(fs::errno::EADDRNOTAVAIL))
                }
            }
            SockAddr::Unix(_) => Err(fs::Error::new(fs::errno::EAFNOSUPPORT)),
        }
    }

    /// Convert to a smoltcp remote endpoint (requires a concrete address).
    pub fn to_endpoint(&self) -> Result<IpEndpoint> {
        match self {
            SockAddr::V4 { addr, port } => {
                let addr = addr.ok_or(fs::Error::new(fs::errno::EADDRNOTAVAIL))?;
                Ok(IpEndpoint {
                    addr: IpAddress::Ipv4(addr),
                    port: *port,
                })
            }
            SockAddr::V6 { addr, port } => {
                if addr[..12] == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff] {
                    Ok(IpEndpoint {
                        addr: IpAddress::Ipv4(Ipv4Address::from_bytes(&addr[12..16])),
                        port: *port,
                    })
                } else {
                    Err(fs::Error::new(fs::errno::EAFNOSUPPORT))
                }
            }
            SockAddr::Unix(_) => Err(fs::Error::new(fs::errno::EAFNOSUPPORT)),
        }
    }

    /// Build from a smoltcp endpoint.
    pub fn from_endpoint(endpoint: IpEndpoint) -> Self {
        match endpoint.addr {
            IpAddress::Ipv4(v4) => SockAddr::V4 {
                addr: Some(v4),
                port: endpoint.port,
            },
        }
    }
}
