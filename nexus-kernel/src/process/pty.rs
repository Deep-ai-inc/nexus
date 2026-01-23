//! PTY (pseudo-terminal) handling.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::libc;
use nix::pty::{openpty, OpenptyResult};
use nix::unistd::{dup2, setsid, Pid};

/// A PTY master/slave pair.
pub struct Pty {
    pub master: File,
    pub slave: File,
}

/// Handle to an open PTY master.
pub struct PtyHandle {
    pub master: File,
    pub pid: Pid,
}

/// Open a new PTY pair.
pub fn open_pty() -> anyhow::Result<Pty> {
    let OpenptyResult { master, slave } = openpty(None, None)?;

    // Convert to File handles
    let master = unsafe { File::from_raw_fd(master.into_raw_fd()) };
    let slave = unsafe { File::from_raw_fd(slave.into_raw_fd()) };

    Ok(Pty { master, slave })
}

/// Set up the slave side of the PTY in the child process.
pub fn setup_slave(slave: &File) -> anyhow::Result<()> {
    let fd = slave.as_raw_fd();

    // Create a new session and set the controlling terminal
    setsid()?;

    // Make this the controlling terminal
    unsafe {
        libc::ioctl(fd, libc::TIOCSCTTY as _, 0);
    }

    // Duplicate to stdin, stdout, stderr
    dup2(fd, 0)?;
    dup2(fd, 1)?;
    dup2(fd, 2)?;

    // Close the original fd if it's not 0, 1, or 2
    if fd > 2 {
        drop(unsafe { OwnedFd::from_raw_fd(fd) });
    }

    Ok(())
}

/// Set non-blocking mode on a file descriptor.
pub fn set_nonblocking(file: &File, nonblocking: bool) -> anyhow::Result<()> {
    let fd = file.as_raw_fd();
    let flags = fcntl(fd, FcntlArg::F_GETFL)?;
    let mut flags = OFlag::from_bits_truncate(flags);

    if nonblocking {
        flags.insert(OFlag::O_NONBLOCK);
    } else {
        flags.remove(OFlag::O_NONBLOCK);
    }

    fcntl(fd, FcntlArg::F_SETFL(flags))?;
    Ok(())
}

/// Get the window size of a PTY.
pub fn get_window_size(file: &File) -> io::Result<(u16, u16)> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let fd = file.as_raw_fd();

    let result = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ as _, &mut size) };

    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok((size.ws_col, size.ws_row))
    }
}

/// Set the window size of a PTY.
pub fn set_window_size(file: &File, cols: u16, rows: u16) -> io::Result<()> {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let fd = file.as_raw_fd();
    let result = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &size) };

    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
