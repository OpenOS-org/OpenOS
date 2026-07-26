#!/usr/bin/env python3
"""Apply readv/writev implementation to mod.rs atomically."""
import sys

with open('/home/dev/projects/open-os/kernel/src/syscall/mod.rs', 'r') as f:
    content = f.read()

# 1. Add SYS_READV and SYS_WRITEV to imports
content = content.replace(
    "    SYS_PROCESS_EXIT, SYS_PROCESS_START, SYS_PROCESS_WAIT, SYS_READLINK, SYS_RECVFROM, SYS_SENDTO,",
    "    SYS_PROCESS_EXIT, SYS_PROCESS_START, SYS_PROCESS_WAIT, SYS_READLINK, SYS_READV, SYS_RECVFROM,\n    SYS_SENDTO,",
    1
)

content = content.replace(
    "    SYS_SOCKET, SYS_SYMLINK, SYS_SYSLOG_DRAIN, SYS_THREAD_CREATE, SYS_THREAD_EXIT, SYS_THREAD_YIELD, SYS_TIMER_CREATE, SYS_TIMER_SETTIME, SYS_TIMER_GETTIME,\n    SYS_UMASK,",
    "    SYS_SOCKET, SYS_SYMLINK, SYS_THREAD_CREATE, SYS_THREAD_EXIT, SYS_THREAD_YIELD,\n    SYS_UMASK, SYS_WRITEV,",
    1
)

# 2. Add dispatch entries
content = content.replace(
    "        SYS_HANDLE_DUPLICATE => sys_handle_duplicate(arg1, arg2),\n        SYS_HANDLE_TRANSFER => sys_handle_transfer(arg1, arg2, arg3),\n\n        SYS_PROCESS_CREATE => sys_process_create(arg1, arg2, arg3),",
    "        SYS_HANDLE_DUPLICATE => sys_handle_duplicate(arg1, arg2),\n        SYS_HANDLE_TRANSFER => sys_handle_transfer(arg1, arg2, arg3),\n\n        SYS_READV => sys_readv(arg1, arg2, arg3),\n        SYS_WRITEV => sys_writev(arg1, arg2, arg3),\n\n        SYS_PROCESS_CREATE => sys_process_create(arg1, arg2, arg3),",
    1
)

# 3. Add implementation
impl = '''
// ─────────────────── Scatter-gather I/O syscalls ───────────────────

/// Size of each `iovec` entry: `{ iov_base: u64, iov_len: u64 }` = 16 bytes.
const IOVEC_SIZE: usize = 16;

/// Maximum number of iovec entries to prevent excessive validation work.
const MAX_IOV_COUNT: usize = 1024;

/// Scatter read: read from a file descriptor into multiple buffers.
fn sys_readv(fd: u64, iov_ptr: u64, iov_count: u64) -> i64 {
    if fd == FD_STDIN || fd == FD_STDOUT {
        return Error::InvalidArgument as i64;
    }
    if iov_count == 0 {
        return 0;
    }
    if iov_count as usize > MAX_IOV_COUNT {
        return Error::InvalidArgument as i64;
    }
    if iov_ptr == 0 || iov_ptr >= crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }
    let total_iovec_bytes = iov_count * IOVEC_SIZE as u64;
    if iov_ptr.saturating_add(total_iovec_bytes) > crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    let mut iovec_buf = alloc::vec![0u8; total_iovec_bytes as usize];
    // SAFETY: We validated that `iov_ptr` is in user-space and the range fits.
    unsafe {
        core::ptr::copy_nonoverlapping(
            iov_ptr as *const u8,
            iovec_buf.as_mut_ptr(),
            total_iovec_bytes as usize,
        );
    }

    let (path, ino, offset) = {
        let result = crate::task::scheduler::with_current_task(|task| {
            task.fd_table
                .get(&fd)
                .map(|entry| (entry.path.clone(), entry.ino, entry.offset))
        });
        match result {
            Some(Some(triple)) => triple,
            _ => return Error::NotFound as i64,
        }
    };

    if path.starts_with("<pipe:") {
        let handle = crate::handle::Handle::from_raw(ino);
        let mut total_read: usize = 0;
        for i in 0..iov_count as usize {
            let base = i * IOVEC_SIZE;
            let iov_base =
                u64::from_le_bytes(iovec_buf[base..base + 8].try_into().unwrap_or([0; 8]));
            let iov_len = u64::from_le_bytes(
                iovec_buf[base + 8..base + 16].try_into().unwrap_or([0; 8]),
            );
            if iov_len == 0 {
                continue;
            }
            if iov_base == 0 || iov_base >= crate::memory::USER_SPACE_MAX {
                return Error::BadPointer as i64;
            }
            if iov_base.saturating_add(iov_len) > crate::memory::USER_SPACE_MAX {
                return Error::BadPointer as i64;
            }
            #[allow(clippy::cast_possible_truncation)]
            let read_len = iov_len.min(MAX_MSG_SIZE as u64) as usize;
            let mut buf = alloc::vec![0u8; read_len];
            let bytes_read = crate::task::scheduler::with_current_task(|task| {
                if let Some(crate::handle::KernelObject::PipeReader(pr)) =
                    task.handle_table.get(handle)
                {
                    Some(pr.lock().read(&mut buf))
                } else {
                    None
                }
            });
            match bytes_read {
                Some(Some(n)) => {
                    if n > 0 {
                        // SAFETY: We validated iov_base is in user-space and iov_len fits.
                        if !unsafe { copy_to_user(iov_base as *mut u8, &buf[..n]) } {
                            return Error::BadPointer as i64;
                        }
                        total_read += n;
                    }
                    if n < read_len {
                        break;
                    }
                }
                _ => return Error::NotFound as i64,
            }
        }
        crate::serial_println!("[SYSCALL] readv: pipe fd {} read {} bytes", fd, total_read);
        #[allow(clippy::cast_possible_wrap)]
        {
            return total_read as i64;
        }
    }

    let (fs, _rel_path) = crate::fs::vfs::resolve_fs(&path);
    let mut total_read: usize = 0;
    let mut current_offset = offset;
    for i in 0..iov_count as usize {
        let base = i * IOVEC_SIZE;
        let iov_base =
            u64::from_le_bytes(iovec_buf[base..base + 8].try_into().unwrap_or([0; 8]));
        let iov_len =
            u64::from_le_bytes(iovec_buf[base + 8..base + 16].try_into().unwrap_or([0; 8]));
        if iov_len == 0 {
            continue;
        }
        if iov_base == 0 || iov_base >= crate::memory::USER_SPACE_MAX {
            return Error::BadPointer as i64;
        }
        if iov_base.saturating_add(iov_len) > crate::memory::USER_SPACE_MAX {
            return Error::BadPointer as i64;
        }
        #[allow(clippy::cast_possible_truncation)]
        let read_len = iov_len.min(MAX_MSG_SIZE as u64) as usize;
        let mut buf = alloc::vec![0u8; read_len];
        match fs.read(ino, current_offset as u64, &mut buf) {
            Ok(n) => {
                if n > 0 {
                    // SAFETY: We validated iov_base is in user-space and iov_len fits.
                    if !unsafe { copy_to_user(iov_base as *mut u8, &buf[..n]) } {
                        return Error::BadPointer as i64;
                    }
                    total_read += n;
                    current_offset += n;
                }
                if n < read_len {
                    break;
                }
            }
            Err(_) => {
                if total_read == 0 {
                    return Error::NotFound as i64;
                }
                break;
            }
        }
    }
    crate::task::scheduler::with_current_task_mut(|task| {
        if let Some(entry) = task.fd_table.get_mut(&fd) {
            entry.offset = current_offset;
        }
    });
    crate::serial_println!(
        "[SYSCALL] readv: fd {} read {} bytes across {} iovecs",
        fd,
        total_read,
        iov_count
    );
    #[allow(clippy::cast_possible_wrap)]
    {
        total_read as i64
    }
}

/// Gather write: write from multiple buffers to a file descriptor.
fn sys_writev(fd: u64, iov_ptr: u64, iov_count: u64) -> i64 {
    if fd == FD_STDOUT {
        if iov_count == 0 {
            return 0;
        }
        if iov_count as usize > MAX_IOV_COUNT {
            return Error::InvalidArgument as i64;
        }
        if iov_ptr == 0 || iov_ptr >= crate::memory::USER_SPACE_MAX {
            return Error::BadPointer as i64;
        }
        let total_iovec_bytes = iov_count * IOVEC_SIZE as u64;
        if iov_ptr.saturating_add(total_iovec_bytes) > crate::memory::USER_SPACE_MAX {
            return Error::BadPointer as i64;
        }
        let mut iovec_buf = alloc::vec![0u8; total_iovec_bytes as usize];
        // SAFETY: We validated that `iov_ptr` is in user-space and the range fits.
        unsafe {
            core::ptr::copy_nonoverlapping(
                iov_ptr as *const u8,
                iovec_buf.as_mut_ptr(),
                total_iovec_bytes as usize,
            );
        }
        let mut total_written: usize = 0;
        for i in 0..iov_count as usize {
            let base = i * IOVEC_SIZE;
            let iov_base =
                u64::from_le_bytes(iovec_buf[base..base + 8].try_into().unwrap_or([0; 8]));
            let iov_len = u64::from_le_bytes(
                iovec_buf[base + 8..base + 16].try_into().unwrap_or([0; 8]),
            );
            if iov_len == 0 {
                continue;
            }
            let Some(data) =
                (unsafe { copy_from_user(iov_base as *const u8, iov_len as usize) })
            else {
                if total_written > 0 {
                    #[allow(clippy::cast_possible_wrap)]
                    {
                        return total_written as i64;
                    }
                }
                return Error::BadPointer as i64;
            };
            for &byte in &data {
                crate::serial_print!("{}", byte as char);
            }
            total_written += data.len();
        }
        #[allow(clippy::cast_possible_wrap)]
        {
            return total_written as i64;
        }
    }

    if fd == FD_STDIN {
        return Error::InvalidArgument as i64;
    }
    if iov_count == 0 {
        return 0;
    }
    if iov_count as usize > MAX_IOV_COUNT {
        return Error::InvalidArgument as i64;
    }
    if iov_ptr == 0 || iov_ptr >= crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }
    let total_iovec_bytes = iov_count * IOVEC_SIZE as u64;
    if iov_ptr.saturating_add(total_iovec_bytes) > crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    let mut iovec_buf = alloc::vec![0u8; total_iovec_bytes as usize];
    // SAFETY: We validated that `iov_ptr` is in user-space and the range fits.
    unsafe {
        core::ptr::copy_nonoverlapping(
            iov_ptr as *const u8,
            iovec_buf.as_mut_ptr(),
            total_iovec_bytes as usize,
        );
    }

    let (path, ino, offset) = {
        let result = crate::task::scheduler::with_current_task(|task| {
            task.fd_table
                .get(&fd)
                .map(|entry| (entry.path.clone(), entry.ino, entry.offset))
        });
        match result {
            Some(Some(triple)) => triple,
            _ => return Error::NotFound as i64,
        }
    };

    if path.starts_with("<pipe:") {
        let handle = crate::handle::Handle::from_raw(ino);
        let mut total_written: usize = 0;
        for i in 0..iov_count as usize {
            let base = i * IOVEC_SIZE;
            let iov_base =
                u64::from_le_bytes(iovec_buf[base..base + 8].try_into().unwrap_or([0; 8]));
            let iov_len = u64::from_le_bytes(
                iovec_buf[base + 8..base + 16].try_into().unwrap_or([0; 8]),
            );
            if iov_len == 0 {
                continue;
            }
            let Some(data) =
                (unsafe { copy_from_user(iov_base as *const u8, iov_len as usize) })
            else {
                if total_written > 0 {
                    #[allow(clippy::cast_possible_wrap)]
                    {
                        return total_written as i64;
                    }
                }
                return Error::BadPointer as i64;
            };
            let write_result = crate::task::scheduler::with_current_task(|task| {
                if let Some(crate::handle::KernelObject::PipeWriter(pw)) =
                    task.handle_table.get(handle)
                {
                    Some(pw.lock().write(&data))
                } else {
                    None
                }
            });
            match write_result {
                Some(Some(Ok(n))) => {
                    total_written += n;
                    if n < data.len() {
                        break;
                    }
                }
                Some(Some(Err(_))) => {
                    if total_written > 0 {
                        #[allow(clippy::cast_possible_wrap)]
                        {
                            return total_written as i64;
                        }
                    }
                    return Error::ChannelClosed as i64;
                }
                _ => return Error::NotFound as i64,
            }
        }
        crate::serial_println!("[SYSCALL] writev: pipe fd {} wrote {} bytes", fd, total_written);
        #[allow(clippy::cast_possible_wrap)]
        {
            return total_written as i64;
        }
    }

    let (fs, _rel_path) = crate::fs::vfs::resolve_fs(&path);
    let mut total_written: usize = 0;
    let mut current_offset = offset;
    for i in 0..iov_count as usize {
        let base = i * IOVEC_SIZE;
        let iov_base =
            u64::from_le_bytes(iovec_buf[base..base + 8].try_into().unwrap_or([0; 8]));
        let iov_len =
            u64::from_le_bytes(iovec_buf[base + 8..base + 16].try_into().unwrap_or([0; 8]));
        if iov_len == 0 {
            continue;
        }
        let Some(data) =
            (unsafe { copy_from_user(iov_base as *const u8, iov_len as usize) })
        else {
            if total_written > 0 {
                break;
            }
            return Error::BadPointer as i64;
        };
        match fs.write(ino, current_offset as u64, &data) {
            Ok(n) => {
                total_written += n;
                current_offset += n;
                if n < data.len() {
                    break;
                }
            }
            Err(_) => {
                if total_written == 0 {
                    return Error::InvalidArgument as i64;
                }
                break;
            }
        }
    }
    crate::task::scheduler::with_current_task_mut(|task| {
        if let Some(entry) = task.fd_table.get_mut(&fd) {
            entry.offset = current_offset;
        }
    });
    crate::serial_println!(
        "[SYSCALL] writev: fd {} wrote {} bytes across {} iovecs",
        fd,
        total_written,
        iov_count
    );
    #[allow(clippy::cast_possible_wrap)]
    {
        total_written as i64
    }
}

'''

marker = '// ─' * 10 + ' Process syscalls ' + '─' * 10
# Use ASCII dashes instead
marker = '// ─────────────────── Process syscalls ───────────────────'
assert marker in content, f"Marker not found"
content = content.replace(marker, impl + marker, 1)

with open('/home/dev/projects/open-os/kernel/src/syscall/mod.rs', 'w') as f:
    f.write(content)

print("All edits applied successfully")
