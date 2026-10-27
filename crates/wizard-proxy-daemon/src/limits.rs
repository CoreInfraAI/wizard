//! Process-local descriptor budget for the daemon, which owns the proxy sockets.

/// Raise the soft limit without lowering it or exceeding the hard limit.
#[expect(
    unsafe_code,
    reason = "std has no safe bindings for getrlimit and setrlimit"
)]
pub(super) fn raise_open_file_limit() {
    // macOS setrlimit(2) recommends min(OPEN_MAX, rlim_max) for RLIMIT_NOFILE.
    const TARGET: libc::rlim_t = 10240;
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: limit is a valid, writable rlimit.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) } != 0 {
        let error = std::io::Error::last_os_error();
        log::error!("failed to read daemon open file limit: {error}");
        return;
    }

    let previous = limit.rlim_cur;
    limit.rlim_cur = TARGET.min(limit.rlim_max);
    if limit.rlim_cur <= previous {
        return;
    }

    // SAFETY: limit is valid and its soft limit does not exceed its hard limit.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const limit) } != 0 {
        let error = std::io::Error::last_os_error();
        log::error!("failed to raise daemon open file limit from {previous}: {error}");
        return;
    }
    log::info!(
        "daemon open file limit raised from {previous} to {}",
        limit.rlim_cur
    );
}
