//! Process helpers shared by sessions and `--profile` browsers.

/// Spawn a process that outlives this one and the shell that ran it (the
/// session server, a `--profile` browser): its own process group on Unix;
/// on Windows detached from our console, in a new process group, out of the
/// caller's job when that job allows it (Node's child-process job does),
/// and without a copy of our stdio handles — an inherited stdout would keep
/// the caller's pipe open, and a shell waiting for EOF would hang until the
/// detached process exits.
#[cfg(unix)]
pub fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0).spawn()
}

#[cfg(windows)]
pub fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const STD_HANDLES: [u32; 3] = [-10i32 as u32, -11i32 as u32, -12i32 as u32];
    const HANDLE_FLAG_INHERIT: u32 = 1;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(which: u32) -> *mut std::ffi::c_void;
        fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    for which in STD_HANDLES {
        // SAFETY: plain Win32 calls on this process's own std handles.
        unsafe {
            let handle = GetStdHandle(which);
            if !handle.is_null() && handle as isize != -1 {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
    let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    cmd.creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB).spawn().or_else(|_| cmd.creation_flags(flags).spawn())
}

/// Whether this process runs as root (Chrome refuses its sandbox then).
#[cfg(unix)]
pub fn running_as_root() -> bool {
    extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { geteuid() == 0 }
}

#[cfg(not(unix))]
pub fn running_as_root() -> bool {
    false
}

/// A process `start --parent` ties a session to: the session quits when it
/// exits. On Windows a handle is held, so a reused pid can't pass for it.
pub struct ParentWatch {
    pid: u32,
    #[cfg(windows)]
    handle: *mut std::ffi::c_void,
}

// SAFETY: a process handle may be waited on from any thread.
#[cfg(windows)]
unsafe impl Send for ParentWatch {}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
    fn WaitForSingleObject(handle: *mut std::ffi::c_void, ms: u32) -> u32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

#[cfg(windows)]
impl Drop for ParentWatch {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: the handle is ours, opened in `new`, closed once.
            unsafe { CloseHandle(self.handle) };
        }
    }
}

impl ParentWatch {
    /// `None` when no process `pid` runs (or it can't be watched).
    pub fn new(pid: u32) -> Option<ParentWatch> {
        #[cfg(unix)]
        {
            let watch = ParentWatch { pid };
            watch.alive().then_some(watch)
        }
        #[cfg(windows)]
        {
            const SYNCHRONIZE: u32 = 0x0010_0000;
            // SAFETY: plain Win32 call; a null handle means no such process.
            let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
            let watch = ParentWatch { pid, handle };
            (!handle.is_null() && watch.alive()).then_some(watch)
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Is the process still running?
    pub fn alive(&self) -> bool {
        #[cfg(unix)]
        {
            pid_alive(self.pid)
        }
        #[cfg(windows)]
        {
            const WAIT_TIMEOUT: u32 = 0x102;
            // SAFETY: the handle is ours and open for SYNCHRONIZE.
            unsafe { WaitForSingleObject(self.handle, 0) == WAIT_TIMEOUT }
        }
    }
}

/// Is a process with this pid running (or a zombie not yet reaped)?
#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let Ok(pid) = i32::try_from(pid) else { return false };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { kill(pid, 0) } == 0 {
        return true;
    }
    // EPERM: it exists, and is someone else's.
    std::io::Error::last_os_error().raw_os_error() == Some(1)
}

#[cfg(windows)]
pub fn pid_alive(pid: u32) -> bool {
    ParentWatch::new(pid).is_some()
}

/// Has the process `pid` ended? A child of this process that has exited is
/// reaped here (else it stays a zombie, which [`pid_alive`] counts as
/// running, until this process ends); any other process is gone once no
/// process has its pid.
#[cfg(unix)]
pub fn process_gone(pid: u32) -> bool {
    extern "C" {
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    }
    const WNOHANG: i32 = 1; // the same on Linux and macOS
    let Ok(id) = i32::try_from(pid) else { return true };
    if id <= 0 {
        return true;
    }
    let mut status = 0;
    // SAFETY: `status` is a live i32 the call writes once; WNOHANG never
    // blocks; a pid that is not our child only gets ECHILD.
    match unsafe { waitpid(id, &mut status, WNOHANG) } {
        r if r == id => true,
        0 => false,
        _ => !pid_alive(pid),
    }
}

#[cfg(windows)]
pub fn process_gone(pid: u32) -> bool {
    !pid_alive(pid)
}

/// Every process: pid -> (parent pid, memory in KiB). Memory is the
/// proportional set size on Linux (shared pages split between their
/// users, so a tree's sum is what it costs), the resident set elsewhere.
pub fn process_table() -> std::collections::HashMap<u32, (u32, u64)> {
    let mut table = std::collections::HashMap::new();
    #[cfg(target_os = "linux")]
    {
        let Ok(dir) = std::fs::read_dir("/proc") else { return table };
        for entry in dir.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
            // The name is in parentheses and may hold spaces: fields
            // after the last ')' are state, ppid, ...
            let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else { continue };
            let Some(ppid) = rest.split_whitespace().nth(1).and_then(|s| s.parse().ok()) else { continue };
            let kb = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
                .ok()
                .and_then(|t| {
                    t.lines().find(|l| l.starts_with("Pss:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
                })
                .unwrap_or(0);
            table.insert(pid, (ppid, kb));
        }
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        if let Ok(out) = std::process::Command::new("ps").args(["-axo", "pid=,ppid=,rss="]).output() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let f: Vec<u64> = line.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                if let [pid, ppid, rss] = f[..] {
                    table.insert(pid as u32, (ppid as u32, rss));
                }
            }
        }
    }
    #[cfg(windows)]
    {
        let script = "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId) $($_.ParentProcessId) $([math]::Round($_.WorkingSetSize / 1024)) $(if ($_.CreationDate) { $_.CreationDate.ToFileTimeUtc() } else { 0 })\" }";
        if let Ok(out) = std::process::Command::new("powershell").args(["-NoProfile", "-Command", script]).output() {
            let mut born = std::collections::HashMap::new();
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let f: Vec<u64> = line.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                if let [pid, ppid, kb, created] = f[..] {
                    table.insert(pid as u32, (ppid as u32, kb));
                    born.insert(pid as u32, created);
                }
            }
            orphan_reused_parents(&mut table, &born);
        }
    }
    table
}

/// Windows keeps a process's parent id after the parent exits, and gives
/// that id to the next process it starts: a process younger than the one
/// its parent id now names is not its child (a session would count another
/// session's browser as its own memory). Such a process is cut loose.
#[cfg_attr(not(windows), allow(dead_code))]
fn orphan_reused_parents(
    table: &mut std::collections::HashMap<u32, (u32, u64)>,
    born: &std::collections::HashMap<u32, u64>,
) {
    for (pid, (ppid, _)) in table.iter_mut() {
        let (Some(child), Some(parent)) = (born.get(pid), born.get(ppid)) else { continue };
        if *parent > *child {
            *ppid = 0;
        }
    }
}

/// Memory (KiB) of `root` and every process below it, leaving out the
/// subtrees of `skip` (a shared browser's host that a session started is
/// its child, but not its memory).
pub fn tree_kb(table: &std::collections::HashMap<u32, (u32, u64)>, root: u32, skip: &[u32]) -> u64 {
    let mut total = 0;
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) || (pid != root && skip.contains(&pid)) {
            continue;
        }
        if let Some((_, kb)) = table.get(&pid) {
            total += kb;
        }
        stack.extend(table.iter().filter(|(_, (ppid, _))| *ppid == pid).map(|(child, _)| *child));
    }
    total
}

#[cfg(test)]
mod tests {
    /// A child that exits is gone once reaped (not a zombie counted as
    /// running); a running child and a live process not ours are not gone.
    #[cfg(unix)]
    #[test]
    fn a_child_is_gone_once_it_exits_and_is_reaped() {
        let mut child = std::process::Command::new("sleep").arg("0.3").spawn().unwrap();
        let pid = child.id();
        assert!(!super::process_gone(pid), "running");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !super::process_gone(pid) {
            assert!(std::time::Instant::now() < deadline, "the exited child was never reported gone");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!super::pid_alive(pid), "reaped, so not a zombie");
        let _ = child.try_wait();
        assert!(!super::process_gone(1), "init runs and is not our child");
    }
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn a_parent_id_taken_by_a_younger_process_is_no_parent() {
        // 10 is a session started at t=50; 20 is another session's
        // browser, started at t=30 by a process 10 that has since exited.
        let mut table: HashMap<u32, (u32, u64)> =
            [(10, (1, 5_000)), (11, (10, 200_000)), (20, (10, 225_000))].into_iter().collect();
        let born: HashMap<u32, u64> = [(1, 1), (10, 50), (11, 60), (20, 30)].into_iter().collect();
        orphan_reused_parents(&mut table, &born);
        assert_eq!(tree_kb(&table, 10, &[]), 205_000);
        assert_eq!(table[&20].0, 0);
    }
}
