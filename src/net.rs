use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub fn new_bound_socket(local_port: u16) -> io::Result<Socket> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    let local = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, local_port));
    socket.bind(&local.into())?;
    Ok(socket)
}

pub fn is_retryable_accept_error(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::WouldBlock
            | io::ErrorKind::Interrupted
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    ) {
        return true;
    }

    #[cfg(target_os = "linux")]
    {
        const LINUX_TRANSIENT_ACCEPT_ERRNOS: &[i32] = &[
            libc::ENETDOWN,
            libc::EPROTO,
            libc::ENOPROTOOPT,
            libc::EHOSTDOWN,
            libc::ENONET,
            libc::EHOSTUNREACH,
            libc::EOPNOTSUPP,
            libc::ENETUNREACH,
        ];
        error
            .raw_os_error()
            .is_some_and(|errno| LINUX_TRANSIENT_ACCEPT_ERRNOS.contains(&errno))
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

pub fn connect_from_local(local_port: u16, remote: SocketAddrV4) -> io::Result<TcpStream> {
    let socket = new_bound_socket(local_port)?;
    socket.set_keepalive(true)?;
    let remote = SocketAddr::V4(remote);
    socket.connect_timeout(&remote.into(), CONNECT_TIMEOUT)?;
    let stream: TcpStream = socket.into();
    stream.set_nodelay(true)?;
    Ok(stream)
}
