//! Process resource limits (SPEC §8).

/// Most file descriptors macOS lets `setrlimit` grant (`OPEN_MAX`); it refuses more, e.g.
/// `RLIM_INFINITY`, even when the hard limit is `unlimited`.
const OPEN_MAX: libc::rlim_t = 10_240;

/// Raise the soft `RLIMIT_NOFILE` to the hard limit (capped at `OPEN_MAX`) and return the new
/// soft limit. Every pane holds a PTY master, pipes and sockets, and an app started from
/// Finder / launchd gets a soft limit of only 256. Never lowers the limit; `None` if it could
/// not be read or raised.
#[must_use]
pub fn raise_fd_limit() -> Option<u64> {
    // SAFETY: plain libc calls on a stack-allocated `rlimit`.
    unsafe {
        let mut lim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut lim) != 0 {
            return None;
        }
        let target = lim.rlim_max.min(OPEN_MAX);
        if lim.rlim_cur >= target {
            return Some(lim.rlim_cur);
        }
        lim.rlim_cur = target;
        (libc::setrlimit(libc::RLIMIT_NOFILE, &raw const lim) == 0).then_some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raises_to_at_least_the_default() {
        let n = raise_fd_limit().expect("getrlimit / setrlimit");
        assert!(n >= 256);
        // Idempotent: a second call keeps the raised limit.
        assert_eq!(raise_fd_limit(), Some(n));
    }
}
