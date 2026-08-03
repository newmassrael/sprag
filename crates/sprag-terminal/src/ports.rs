//! Discovering the TCP ports a session's processes are LISTENING on — the cmux-parity display fact
//! the session sidebar (and `sprag ls`) shows beside each session's cwd and git branch.
//!
//! tmux has no equivalent: it exposes a pane's pid (`#{pane_pid}`) but never inspects sockets, so
//! this is pure cmux parity, ADDITIVE on the tmux-faithful session/window/pane model — it reuses
//! exactly the pane pid tmux already tracks as the anchor to walk from. Derived HOST-side from each
//! pane's live child pid, so a display client carries only the resulting port numbers, never the
//! `/proc` logic.
//!
//! Linux-only, via `/proc` (like the cwd read in [`crate::pane_pty`]): a LISTEN socket a session
//! holds is found by intersecting the session's process SUBTREE's open socket inodes with the LISTEN
//! rows of `/proc/net/tcp{,6}`. The pid of a pane is its shell; the server is usually a DESCENDANT
//! (the shell ran `npm run dev`), so the subtree — not the pane pid alone — is the honest scope.
//! Off Linux the scan yields nothing (an honest empty, never a guess).
//!
//! One [`ProcScan`](crate::ports::ProcScan) is built per session-list read and SHARED across every session, so the cost is a
//! single `/proc` pass — the LISTEN table and the whole pid→children map — not one pass per session.
//! That per-read `/proc` walk is deliberate: this favours a live, never-stale, cache-free read (the
//! same purity as the cwd/branch derivation) over the cheaper throttled-cache alternative. A cache
//! is a purely additive follow-up if the walk ever proves too heavy under load.

use std::collections::{HashMap, HashSet, VecDeque};

/// One `/proc` scan shared across a whole [`session_infos_live`](crate::SessionRegistry::session_infos_live)
/// read: the LISTEN socket `inode → local port` table (from `/proc/net/tcp{,6}`) and the
/// `pid → children` map (from `/proc/*/stat`) — the two facts [`listening_ports`](Self::listening_ports)
/// needs to attribute a listening socket to a session's process subtree.
#[derive(Default)]
pub(crate) struct ProcScan {
    /// Socket inode → the local TCP port it is LISTENING on. Only LISTEN sockets are kept: an
    /// established connection or a TIME_WAIT entry is not a server the user can open.
    listen: HashMap<u64, u16>,
    /// Parent pid → its child pids, so [`listening_ports`](Self::listening_ports) can BFS the
    /// descendants of a pane's pid without a per-pid parent lookup.
    children: HashMap<u32, Vec<u32>>,
}

impl ProcScan {
    /// Read `/proc` ONCE: the LISTEN entries of `/proc/net/tcp` and `/proc/net/tcp6` and every
    /// process's parent. Linux-only; elsewhere an empty scan, so [`listening_ports`](Self::listening_ports)
    /// honestly reports no ports rather than guessing.
    #[cfg(target_os = "linux")]
    pub(crate) fn scan() -> Self {
        let mut listen = HashMap::new();
        for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
            if let Ok(contents) = std::fs::read_to_string(path) {
                for (inode, port) in parse_listen(&contents) {
                    listen.insert(inode, port);
                }
            }
        }
        Self {
            listen,
            children: read_children_map(),
        }
    }

    /// No `/proc` off Linux: an empty scan, so [`listening_ports`](Self::listening_ports) yields
    /// nothing (mirroring [`crate::pane_pty`]'s honest `None` cwd off Linux).
    #[cfg(not(target_os = "linux"))]
    pub(crate) fn scan() -> Self {
        Self::default()
    }

    /// The distinct TCP ports any process in the subtrees rooted at `root_pids` is LISTENING on,
    /// ascending. Walks each root's descendants (a pane's server is its descendant, not the pane
    /// pid), reads every such process's open socket inodes from `/proc/<pid>/fd`, and keeps the
    /// ports whose inode names a LISTEN socket in this scan. Deduped (a listening fd is inherited by
    /// children, so one server's inode recurs across the subtree; a server on both IPv4 and IPv6 is
    /// two inodes on one port). Empty for an empty scan (off Linux) or no pids.
    pub(crate) fn listening_ports(&self, root_pids: &[u32]) -> Vec<u16> {
        if self.listen.is_empty() || root_pids.is_empty() {
            return Vec::new();
        }
        let inodes = subtree(&self.children, root_pids)
            .into_iter()
            .flat_map(socket_inodes);
        self.ports_for_inodes(inodes)
    }

    /// The distinct listening ports named by `inodes`, ascending — the intersect-then-dedup-then-sort
    /// tail of [`listening_ports`](Self::listening_ports), split out so the dedup (a listening fd
    /// recurs across a subtree; a dual-stack server is two inodes on one port) and the ordering are
    /// testable without `/proc`.
    fn ports_for_inodes(&self, inodes: impl Iterator<Item = u64>) -> Vec<u16> {
        let mut ports: Vec<u16> = inodes
            .filter_map(|inode| self.listen.get(&inode).copied())
            .collect();
        ports.sort_unstable();
        ports.dedup();
        ports
    }
}

/// Parse the LISTEN rows of a `/proc/net/tcp` or `tcp6` table into `(socket inode, local port)`
/// pairs. Each data row is whitespace-separated: field 1 is `HEXADDR:HEXPORT`, field 3 is the
/// connection state (`0A` = `TCP_LISTEN`), field 9 is the socket inode. The header and every
/// non-LISTEN row are skipped. One parser serves v4 and v6 — the field layout is identical and the
/// local port is the hex after the local address's last colon in both.
#[cfg(target_os = "linux")]
fn parse_listen(contents: &str) -> Vec<(u64, u16)> {
    contents
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || f[3] != "0A" {
                return None;
            }
            let port = u16::from_str_radix(f[1].rsplit_once(':')?.1, 16).ok()?;
            let inode = f[9].parse::<u64>().ok()?;
            Some((inode, port))
        })
        .collect()
}

/// Every process's parent, as `pid → children`, INDEXED from the crate's one `/proc` pass
/// ([`crate::procfs::walk`]) — which is also where the `stat` line's parse and the reason this
/// walks `stat` rather than the `CONFIG_PROC_CHILDREN`-gated `/proc/<pid>/task/*/children` both
/// live. This function is now only the index, which is the part that belongs to ports.
#[cfg(target_os = "linux")]
fn read_children_map() -> HashMap<u32, Vec<u32>> {
    let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, stat) in crate::procfs::walk() {
        map.entry(stat.ppid).or_default().push(pid);
    }
    map
}

/// Every pid in the subtrees rooted at `roots` (the roots included), via BFS over the `pid →
/// children` map. Pure over the map, so the traversal is testable without `/proc`. A visited set
/// guards the (impossible for a real process tree, but cheap to rule out) cyclic case.
fn subtree(children: &HashMap<u32, Vec<u32>>, roots: &[u32]) -> Vec<u32> {
    let mut seen: HashSet<u32> = HashSet::new();
    let mut queue: VecDeque<u32> = roots.iter().copied().collect();
    let mut out = Vec::new();
    while let Some(pid) = queue.pop_front() {
        if !seen.insert(pid) {
            continue;
        }
        out.push(pid);
        if let Some(kids) = children.get(&pid) {
            queue.extend(kids);
        }
    }
    out
}

/// The socket inodes a process holds open — the `socket:[INODE]` targets of its `/proc/<pid>/fd`
/// symlinks. A pid that exits mid-scan (or whose fds we cannot read) simply yields nothing.
/// Linux-only; empty (and never reached — `listening_ports` early-returns on an empty scan)
/// elsewhere.
#[cfg(target_os = "linux")]
fn socket_inodes(pid: u32) -> Vec<u64> {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let target = std::fs::read_link(entry.path()).ok()?;
            let inode = target
                .to_str()?
                .strip_prefix("socket:[")?
                .strip_suffix(']')?;
            inode.parse::<u64>().ok()
        })
        .collect()
}

/// No `/proc/<pid>/fd` off Linux — an empty list (never reached; see the Linux variant).
#[cfg(not(target_os = "linux"))]
fn socket_inodes(_pid: u32) -> Vec<u64> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LISTEN rows are parsed to `(inode, port)` with a HEX port; the header and non-LISTEN rows
    /// (here an `01` = ESTABLISHED connection) are skipped. Ports are hex: `0x1F90` = 8080. The
    /// SAME parser serves the `/proc/net/tcp6` layout — a 32-hex-char local address still has its
    /// port after the last colon, and the inode is still field 9 (here `0x1F91` = 8081).
    #[cfg(target_os = "linux")]
    #[test]
    fn parse_listen_keeps_only_listen_rows_and_decodes_the_hex_port() {
        let v4 = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 54321 1 0000000000000000 100 0 0 10 0
   1: 0100007F:C001 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 99999 1 0000000000000000 20 0 0 10 0
";
        assert_eq!(
            parse_listen(v4),
            vec![(54321, 8080)],
            "only the LISTEN (0A) row, port 0x1F90",
        );

        let v6 = "\
  sl  local_address                         remote_address                    st ... inode
   0: 00000000000000000000000000000000:1F91 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 77777 1 00000000 100 0 0 10 0
";
        assert_eq!(
            parse_listen(v6),
            vec![(77777, 8081)],
            "the same parser decodes the tcp6 layout (long addr, port after the last colon)",
        );
    }

    /// The `pid → children` index is built from the shared walk, so a process whose `comm` is full
    /// of spaces and parentheses — or is not valid UTF-8 — still lands under its real parent. The
    /// PARSE those cases exercise is pinned in [`crate::procfs`]; this pins the INDEX, which is the
    /// half that lives here.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_children_index_places_this_process_under_its_real_parent() {
        let map = read_children_map();
        let own = std::process::id();
        let stat = crate::procfs::stat(own).expect("this process has a /proc/<pid>/stat");
        assert!(
            map.get(&stat.ppid).is_some_and(|kids| kids.contains(&own)),
            "this process ({own}) should be listed among its parent's ({}) children",
            stat.ppid,
        );
    }

    /// BFS collects the whole subtree — roots plus every transitive child — with no duplicates,
    /// and terminates even if the map (impossibly for real pids) contains a cycle.
    #[test]
    fn subtree_collects_transitive_children_without_looping() {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        children.insert(1, vec![2, 3]);
        children.insert(2, vec![4]);
        children.insert(3, vec![1]); // a cycle back to the root — must not hang.
        let mut got = subtree(&children, &[1]);
        got.sort_unstable();
        assert_eq!(
            got,
            vec![1, 2, 3, 4],
            "roots plus all descendants, once each"
        );
        assert_eq!(
            subtree(&children, &[]),
            Vec::<u32>::new(),
            "no roots, no pids"
        );
    }

    /// End-to-end against REAL `/proc`: a listener the test itself binds is found under the test
    /// process's own pid AND is NOT attributed to an unrelated pid — exercising the whole pipe
    /// (LISTEN table parse + fd inode scan + inode intersection) with a port under the test's
    /// control. The negative half is the load-bearing one: it pins ATTRIBUTION. REVERT-PROOF — an
    /// implementation that returned every LISTEN port on the machine (dropping the fd scan +
    /// intersection) would pass the positive `contains` but FAIL the negative one, since the bound
    /// port is in the global table yet held by no process in `u32::MAX`'s (empty) subtree.
    /// Linux-only (the mechanism is `/proc`). The listener drops at the end, leaving nothing behind.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_listener_is_attributed_only_to_the_pid_that_holds_it() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral loopback port");
        let port = listener.local_addr().unwrap().port();
        let scan = ProcScan::scan();

        assert!(
            scan.listening_ports(&[std::process::id()]).contains(&port),
            "the bound port {port} should be discovered by walking our own subtree",
        );
        // `u32::MAX` is far above any `pid_max`, so it names no process: its subtree is just itself
        // with no `/proc/<pid>/fd`, so it can hold no socket and must report no port.
        assert!(
            !scan.listening_ports(&[u32::MAX]).contains(&port),
            "a port held by no process in the queried subtree must not be attributed to it",
        );
    }

    /// `read_children_map` links a REAL parent to a REAL child on live `/proc`, so the subtree walk
    /// reaches a descendant — the module's whole reason for existing (a pane's server is the shell's
    /// child, not the pane pid itself). REVERT-PROOF for the walk: our spawned child appears in the
    /// subtree of our OWN pid; make `subtree` return just the roots (dropping the children BFS) and
    /// it would not. Linux-only (`/proc`). The child is killed and reaped before the assertion so it
    /// never leaks, even on failure.
    #[cfg(target_os = "linux")]
    #[test]
    fn read_children_map_links_a_real_child_into_our_subtree() {
        use std::process::Command;
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn a long-lived child");
        let child_pid = child.id();

        // The child exists in `/proc` (with our pid as its ppid) the moment `spawn` returns, so this
        // read is race-free — no polling needed.
        let found = subtree(&ProcScan::scan().children, &[std::process::id()]).contains(&child_pid);

        child.kill().ok();
        child.wait().ok();
        assert!(
            found,
            "our spawned child pid {child_pid} should appear in our /proc-derived subtree",
        );
    }

    /// `ports_for_inodes` maps inodes to ports, then DEDUPS and SORTS: an inherited listening fd
    /// recurs across a subtree (the same inode twice) and a dual-stack server is two inodes on one
    /// port — both collapse to one entry, ascending. Pure (no `/proc`), so it is driven directly.
    /// REVERT-PROOF: drop `dedup()` and `8080` doubles; drop `sort_unstable()` and the order breaks.
    #[test]
    fn ports_for_inodes_dedups_and_sorts() {
        let mut listen = HashMap::new();
        listen.insert(10, 8080u16); // one server
        listen.insert(11, 8080u16); // its IPv6 twin: same port, a different inode
        listen.insert(12, 3000u16); // a lower-numbered server
        let scan = ProcScan {
            listen,
            children: HashMap::new(),
        };
        // inode 10 twice (an inherited fd), plus 11 (dual-stack) and 12; 99 is not a listener.
        let ports = scan.ports_for_inodes([10, 10, 11, 12, 99].into_iter());
        assert_eq!(ports, vec![3000, 8080], "distinct ports, ascending");
    }

    /// An empty scan (what `scan()` yields off Linux) reports no ports for any pids — the honest
    /// no-`/proc` answer, and the early-return that keeps `listening_ports` from touching `/proc`
    /// when there is nothing to find.
    #[test]
    fn an_empty_scan_finds_no_ports() {
        assert_eq!(
            ProcScan::default().listening_ports(&[1, 2, 3]),
            Vec::<u16>::new()
        );
    }
}
