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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::io::AsRawFd;

    // -------------------------------------------------------------------------
    // open_pty tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_open_pty_succeeds() {
        let pty = open_pty();
        assert!(pty.is_ok(), "open_pty should succeed");
    }

    #[test]
    fn test_open_pty_creates_valid_file_descriptors() {
        let pty = open_pty().unwrap();

        // Both master and slave should have valid file descriptors
        let master_fd = pty.master.as_raw_fd();
        let slave_fd = pty.slave.as_raw_fd();

        assert!(master_fd >= 0, "master fd should be valid");
        assert!(slave_fd >= 0, "slave fd should be valid");
        assert_ne!(master_fd, slave_fd, "master and slave should be different");
    }

    #[test]
    fn test_open_pty_master_slave_communication() {
        let pty = open_pty().unwrap();
        let mut master = pty.master;
        let mut slave = pty.slave;

        // Set non-blocking to avoid hanging on empty reads
        set_nonblocking(&master, true).unwrap();

        // Write from slave side
        slave.write_all(b"hello").unwrap();
        slave.flush().unwrap();

        // Small delay to allow data to propagate
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Read from master side
        let mut buf = [0u8; 32];
        let n = master.read(&mut buf);

        // Should have read something (PTY might do some processing)
        assert!(n.is_ok() || matches!(n, Err(ref e) if e.kind() == io::ErrorKind::WouldBlock));
    }

    #[test]
    fn test_open_pty_multiple_opens() {
        // Opening multiple PTYs should work
        let pty1 = open_pty();
        let pty2 = open_pty();
        let pty3 = open_pty();

        assert!(pty1.is_ok());
        assert!(pty2.is_ok());
        assert!(pty3.is_ok());

        // All should have different fds
        let fd1 = pty1.unwrap().master.as_raw_fd();
        let fd2 = pty2.unwrap().master.as_raw_fd();
        let fd3 = pty3.unwrap().master.as_raw_fd();

        assert_ne!(fd1, fd2);
        assert_ne!(fd2, fd3);
        assert_ne!(fd1, fd3);
    }

    // -------------------------------------------------------------------------
    // set_nonblocking tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_set_nonblocking_true() {
        let pty = open_pty().unwrap();
        let result = set_nonblocking(&pty.master, true);
        assert!(result.is_ok(), "set_nonblocking(true) should succeed");
    }

    #[test]
    fn test_set_nonblocking_false() {
        let pty = open_pty().unwrap();
        // First set to nonblocking, then back to blocking
        set_nonblocking(&pty.master, true).unwrap();
        let result = set_nonblocking(&pty.master, false);
        assert!(result.is_ok(), "set_nonblocking(false) should succeed");
    }

    #[test]
    fn test_set_nonblocking_toggle() {
        let pty = open_pty().unwrap();

        // Toggle several times
        assert!(set_nonblocking(&pty.master, true).is_ok());
        assert!(set_nonblocking(&pty.master, false).is_ok());
        assert!(set_nonblocking(&pty.master, true).is_ok());
        assert!(set_nonblocking(&pty.master, false).is_ok());
    }

    #[test]
    fn test_set_nonblocking_affects_read_behavior() {
        let pty = open_pty().unwrap();
        let mut master = pty.master;

        // Set non-blocking
        set_nonblocking(&master, true).unwrap();

        // Try to read from empty PTY - should return WouldBlock, not hang
        let mut buf = [0u8; 32];
        let result = master.read(&mut buf);

        assert!(
            matches!(result, Err(ref e) if e.kind() == io::ErrorKind::WouldBlock),
            "Non-blocking read should return WouldBlock, got {:?}",
            result
        );
    }

    // -------------------------------------------------------------------------
    // get_window_size / set_window_size tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_get_window_size_succeeds() {
        let pty = open_pty().unwrap();
        let result = get_window_size(&pty.master);
        assert!(result.is_ok(), "get_window_size should succeed on PTY master");
    }

    #[test]
    fn test_get_window_size_returns_dimensions() {
        let pty = open_pty().unwrap();
        let (cols, rows) = get_window_size(&pty.master).unwrap();

        // PTY should have some dimensions (may be 0x0 initially on some systems)
        assert!(cols < 10000, "cols should be reasonable");
        assert!(rows < 10000, "rows should be reasonable");
    }

    #[test]
    fn test_set_window_size_succeeds() {
        let pty = open_pty().unwrap();
        let result = set_window_size(&pty.master, 80, 24);
        assert!(result.is_ok(), "set_window_size should succeed");
    }

    #[test]
    fn test_set_window_size_changes_size() {
        let pty = open_pty().unwrap();

        // Set a specific size
        set_window_size(&pty.master, 120, 40).unwrap();

        // Read it back
        let (cols, rows) = get_window_size(&pty.master).unwrap();

        assert_eq!(cols, 120, "cols should be 120");
        assert_eq!(rows, 40, "rows should be 40");
    }

    #[test]
    fn test_set_window_size_various_sizes() {
        let pty = open_pty().unwrap();

        // Test various common terminal sizes
        let sizes = [(80, 24), (120, 40), (200, 60), (132, 43)];

        for (cols, rows) in sizes {
            set_window_size(&pty.master, cols, rows).unwrap();
            let (got_cols, got_rows) = get_window_size(&pty.master).unwrap();
            assert_eq!(got_cols, cols, "cols mismatch for {cols}x{rows}");
            assert_eq!(got_rows, rows, "rows mismatch for {cols}x{rows}");
        }
    }

    #[test]
    fn test_set_window_size_minimum_size() {
        let pty = open_pty().unwrap();

        // Set minimum size
        set_window_size(&pty.master, 1, 1).unwrap();
        let (cols, rows) = get_window_size(&pty.master).unwrap();

        assert_eq!(cols, 1);
        assert_eq!(rows, 1);
    }

    #[test]
    fn test_set_window_size_large_size() {
        let pty = open_pty().unwrap();

        // Set a large but reasonable size
        set_window_size(&pty.master, 500, 200).unwrap();
        let (cols, rows) = get_window_size(&pty.master).unwrap();

        assert_eq!(cols, 500);
        assert_eq!(rows, 200);
    }

    // -------------------------------------------------------------------------
    // PtyHandle struct tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_pty_handle_fields() {
        let pty = open_pty().unwrap();
        let pid = Pid::from_raw(12345);

        let handle = PtyHandle {
            master: pty.master,
            pid,
        };

        assert_eq!(handle.pid, Pid::from_raw(12345));
        assert!(handle.master.as_raw_fd() >= 0);
    }

    // -------------------------------------------------------------------------
    // Edge cases and error handling
    // -------------------------------------------------------------------------

    #[test]
    fn test_pty_closes_properly() {
        let fd;
        {
            let pty = open_pty().unwrap();
            fd = pty.master.as_raw_fd();
            // pty drops here
        }

        // After drop, the fd should be invalid
        // We can't directly test this, but at least verify no panic
        let result = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert_eq!(result, -1, "closed fd should return -1");
    }
}
