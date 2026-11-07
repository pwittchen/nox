//! Foreground process inspection (SPEC §4.4, §4.6): `tcgetpgrp` on the PTY master, then
//! `proc_pidpath` / `proc_pidinfo` / `KERN_PROCARGS2` for the executable, arguments and cwd.
//! Cheap enough to run on prompt marks, focus changes and at most every 500 ms while output
//! flows.

use std::ffi::CStr;
use std::os::fd::RawFd;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundProcess {
    pub pid: i32,
    /// Executable basename (`zsh`, `vim`, `node`).
    pub exe: String,
    /// Arguments without argv[0].
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

/// The foreground process group leader of the terminal behind `pty_fd`.
pub fn foreground_pid(pty_fd: RawFd) -> Option<i32> {
    let pgrp = unsafe { libc::tcgetpgrp(pty_fd) };
    (pgrp > 0).then_some(pgrp)
}

pub fn foreground_process(pty_fd: RawFd) -> Option<ForegroundProcess> {
    let pid = foreground_pid(pty_fd)?;
    process_info(pid)
}

pub fn process_info(pid: i32) -> Option<ForegroundProcess> {
    let (path_exe, args) = match proc_args(pid) {
        Some((exe, args)) => (Some(exe), args),
        None => (None, vec![]),
    };
    let exe_path = proc_path(pid).or(path_exe)?;
    let exe = basename(&exe_path);
    // argv[0] of login shells is `-zsh`; normalize.
    let exe = exe.trim_start_matches('-').to_string();
    Some(ForegroundProcess { pid, exe, args, cwd: proc_cwd(pid) })
}

fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

pub fn proc_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

pub fn proc_cwd(pid: i32) -> Option<PathBuf> {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut libc::proc_vnodepathinfo).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    let raw = &info.pvi_cdir.vip_path;
    // `vip_path` is a fixed MAXPATHLEN buffer (declared as nested arrays in libc).
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(raw as *const _ as *const u8, std::mem::size_of_val(raw)) };
    let cstr = CStr::from_bytes_until_nul(bytes).ok()?;
    let s = cstr.to_str().ok()?;
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// Executable path and arguments (without argv[0]) via `sysctl(KERN_PROCARGS2)`.
pub fn proc_args(pid: i32) -> Option<(String, Vec<String>)> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    let rc = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0)
    };
    if rc != 0 || size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size];
    let rc = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0)
    };
    if rc != 0 {
        return None;
    }
    buf.truncate(size);
    parse_procargs2(&buf)
}

/// Layout: `argc: i32`, exec path, NULs, then argc NUL-terminated args, then env.
fn parse_procargs2(buf: &[u8]) -> Option<(String, Vec<String>)> {
    if buf.len() < 4 {
        return None;
    }
    let argc = i32::from_ne_bytes(buf[..4].try_into().ok()?) as usize;
    let mut rest = &buf[4..];
    let end = rest.iter().position(|&b| b == 0)?;
    let exe = String::from_utf8_lossy(&rest[..end]).into_owned();
    rest = &rest[end..];
    // Skip padding NULs.
    let start = rest.iter().position(|&b| b != 0)?;
    rest = &rest[start..];
    let mut args = Vec::with_capacity(argc);
    for _ in 0..argc {
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        args.push(String::from_utf8_lossy(&rest[..end]).into_owned());
        rest = rest.get(end + 1..).unwrap_or(&[]);
    }
    if !args.is_empty() {
        args.remove(0);
    }
    Some((exe, args))
}

/// The user's login shell: `$SHELL`, then the passwd entry, then `/bin/zsh`.
pub fn login_shell() -> String {
    if let Ok(s) = std::env::var("SHELL")
        && !s.is_empty()
    {
        return s;
    }
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null()
            && !(*pw).pw_shell.is_null()
            && let Ok(s) = CStr::from_ptr((*pw).pw_shell).to_str()
        {
            return s.to_string();
        }
    }
    "/bin/zsh".to_string()
}

/// `user@host` for titles (`pw@mbp`). Host is the short hostname.
pub fn user_at_host() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let mut buf = [0u8; 256];
    let host = unsafe {
        if libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) == 0 {
            CStr::from_bytes_until_nul(&buf).ok().and_then(|c| c.to_str().ok()).unwrap_or("localhost")
        } else {
            "localhost"
        }
    };
    let host = host.split('.').next().unwrap_or(host);
    format!("{user}@{host}")
}

/// `$HOME` shortened to `~`.
pub fn tildify(path: &std::path::Path) -> String {
    if let Some(home) = dirs_home()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        let rest = rest.to_string_lossy();
        return if rest.is_empty() { "~".into() } else { format!("~/{rest}") };
    }
    path.to_string_lossy().into_owned()
}

pub fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_procargs2() {
        let mut buf = 3i32.to_ne_bytes().to_vec();
        buf.extend(b"/usr/bin/node\0\0\0node\0/opt/claude/cli.js\0--resume\0HOME=/x\0");
        let (exe, args) = parse_procargs2(&buf).unwrap();
        assert_eq!(exe, "/usr/bin/node");
        assert_eq!(args, vec!["/opt/claude/cli.js", "--resume"]);
    }

    #[test]
    fn own_process_info() {
        let pid = std::process::id() as i32;
        let info = process_info(pid).expect("info for self");
        assert!(!info.exe.is_empty());
        assert_eq!(info.cwd.as_deref(), std::env::current_dir().ok().as_deref());
    }

    #[test]
    fn tildify_home() {
        let home = dirs_home().unwrap();
        assert_eq!(tildify(&home), "~");
        assert_eq!(tildify(&home.join("x/y")), "~/x/y");
        assert_eq!(tildify(std::path::Path::new("/tmp")), "/tmp");
    }
}
