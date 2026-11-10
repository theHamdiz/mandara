//! Listening sockets for the `ports` plugin: TCP sockets in LISTEN and
//! unconnected UDP sockets, read from `/proc/net/{tcp,tcp6,udp,udp6}`.
//!
//! The kernel tables carry each socket's owner uid and inode. The owning
//! process is found by scanning `/proc/<pid>/fd` for a `socket:[inode]`
//! link, which only works for the user's own processes; sockets owned by
//! other users (root daemons, system services) keep their uid but get no
//! PID. The scan is limited to the user's sockets and processes, starts at
//! the PIDs found last time and stops as soon as every socket is matched.
//!
//! Everything here is blocking file IO: call it off the UI thread.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::unix::fs::MetadataExt as _;

use serde::Serialize;

/// The four kernel tables and the protocol / family each one lists.
const TABLES: [(&str, Proto); 4] = [("/proc/net/tcp", Proto::Tcp), ("/proc/net/tcp6", Proto::Tcp), ("/proc/net/udp", Proto::Udp), ("/proc/net/udp6", Proto::Udp)];
/// `st` of a listening TCP socket (`TCP_LISTEN`).
const TCP_LISTEN: &str = "0A";
/// `st` of an unconnected UDP socket (`TCP_CLOSE`, the kernel reuses it).
const UDP_UNCONNECTED: &str = "07";
/// Hex digits of an IPv4 / IPv6 address in the tables.
const V4_HEX: usize = 8;
const V6_HEX: usize = 32;
/// Hex digits of one 32-bit word of an address.
const WORD_HEX: usize = 8;
/// Command lines longer than this are cut (they are shown on one line).
const COMMAND_MAX_CHARS: usize = 240;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Proto {
    Tcp,
    Udp,
}

impl Proto {
    pub fn label(self) -> &'static str {
        match self {
            Proto::Tcp => "TCP",
            Proto::Udp => "UDP",
        }
    }
}

/// Who can reach a listener.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// Bound to 127.0.0.1 / ::1 only: this computer.
    Localhost,
    /// Bound to 0.0.0.0 / ::: every interface, so the network too.
    AllInterfaces,
    /// Bound to one specific address.
    Address,
}

/// One row of a `/proc/net` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Socket {
    pub proto: Proto,
    pub addr: IpAddr,
    pub port: u16,
    pub uid: u32,
    pub inode: u64,
}

/// A listening port: one or more sockets of one process (IPv4 and IPv6
/// sockets of the same server are merged).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Listener {
    pub proto: Proto,
    pub port: u16,
    pub scope: Scope,
    /// Every address it is bound to.
    pub addresses: Vec<IpAddr>,
    pub uid: u32,
    /// `None`: another user's process (not readable).
    pub pid: Option<u32>,
    pub process: Option<String>,
    pub command: Option<String>,
}

impl Listener {
    /// The host to connect to: `localhost` unless it is bound to one
    /// specific address.
    pub fn host(&self) -> String {
        match (self.scope, self.addresses.first()) {
            (Scope::Address, Some(IpAddr::V6(a))) => format!("[{a}]"),
            (Scope::Address, Some(a)) => a.to_string(),
            _ => "localhost".into(),
        }
    }

    /// `host:port`, ready to paste.
    pub fn address(&self) -> String {
        format!("{}:{}", self.host(), self.port)
    }

    /// `http://host:port` for a TCP listener.
    pub fn url(&self) -> Option<String> {
        (self.proto == Proto::Tcp).then(|| format!("http://{}", self.address()))
    }
}

/// An address in the tables: 8 hex digits (IPv4) or 32 (IPv6), each 32-bit
/// word in the kernel's (little-endian) byte order.
pub fn parse_addr(hex: &str) -> Option<IpAddr> {
    let word = |s: &str| u32::from_str_radix(s, 16).ok().map(u32::to_le_bytes);
    match hex.len() {
        V4_HEX => Some(IpAddr::V4(Ipv4Addr::from(word(hex)?))),
        V6_HEX => {
            let mut bytes = [0u8; 16];
            for (i, chunk) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                *chunk = word(hex.get(i * WORD_HEX..(i + 1) * WORD_HEX)?)?;
            }
            Some(IpAddr::V6(Ipv6Addr::from(bytes)))
        }
        _ => None,
    }
}

/// `ADDR:PORT` (the port is plain big-endian hex).
fn parse_endpoint(field: &str) -> Option<(IpAddr, u16)> {
    let (addr, port) = field.split_once(':')?;
    Some((parse_addr(addr)?, u16::from_str_radix(port, 16).ok()?))
}

/// Parses one table, keeping listening TCP / unconnected UDP sockets.
pub fn parse_table(text: &str, proto: Proto) -> Vec<Socket> {
    let listening = match proto {
        Proto::Tcp => TCP_LISTEN,
        Proto::Udp => UDP_UNCONNECTED,
    };
    text.lines()
        .skip(1)
        .filter_map(|line| {
            // sl local remote st tx:rx tr:when retrnsmt uid timeout inode …
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || f[3] != listening {
                return None;
            }
            let (addr, port) = parse_endpoint(f[1])?;
            let (_, remote_port) = parse_endpoint(f[2])?;
            if port == 0 || (proto == Proto::Udp && remote_port != 0) {
                return None;
            }
            Some(Socket { proto, addr, port, uid: f[7].parse().ok()?, inode: f[9].parse().ok()? })
        })
        .collect()
}

/// Every listening socket on the system.
pub fn sockets() -> Vec<Socket> {
    TABLES.iter().flat_map(|(path, proto)| std::fs::read_to_string(path).map(|t| parse_table(&t, *proto)).unwrap_or_default()).collect()
}

fn scope_of(addresses: &[IpAddr]) -> Scope {
    let mapped = |a: &IpAddr| match a {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(*a, IpAddr::V4),
        _ => *a,
    };
    if addresses.iter().any(|a| mapped(a).is_unspecified()) {
        Scope::AllInterfaces
    } else if addresses.iter().all(|a| mapped(a).is_loopback()) {
        Scope::Localhost
    } else {
        Scope::Address
    }
}

/// Process details for a listener.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Owner {
    pub pid: u32,
    pub name: String,
    pub command: String,
}

/// Merges sockets into listeners: one per protocol, port and owner (the
/// IPv4 and IPv6 sockets of one server become one row). Sorted by port.
pub fn merge(sockets: &[Socket], owners: &HashMap<u64, Owner>) -> Vec<Listener> {
    let mut index: HashMap<(Proto, u16, u32, Option<u32>), usize> = HashMap::new();
    let mut out: Vec<Listener> = Vec::new();
    for s in sockets {
        let owner = owners.get(&s.inode);
        let key = (s.proto, s.port, s.uid, owner.map(|o| o.pid));
        match index.get(&key) {
            Some(&i) => {
                if !out[i].addresses.contains(&s.addr) {
                    out[i].addresses.push(s.addr);
                }
            }
            None => {
                index.insert(key, out.len());
                out.push(Listener {
                    proto: s.proto,
                    port: s.port,
                    scope: Scope::Address,
                    addresses: vec![s.addr],
                    uid: s.uid,
                    pid: owner.map(|o| o.pid),
                    process: owner.map(|o| o.name.clone()),
                    command: owner.map(|o| o.command.clone()).filter(|c| !c.is_empty()),
                });
            }
        }
    }
    for l in &mut out {
        l.addresses.sort();
        l.scope = scope_of(&l.addresses);
    }
    out.sort_by(|a, b| a.port.cmp(&b.port).then(a.proto.cmp(&b.proto)));
    out
}

/// Distinct listening TCP ports (the bar chip's count).
pub fn tcp_port_count(sockets: &[Socket]) -> usize {
    sockets.iter().filter(|s| s.proto == Proto::Tcp).map(|s| s.port).collect::<HashSet<_>>().len()
}

/// This user's uid.
pub fn my_uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// `socket:[12345]` → 12345.
fn socket_inode(link: &std::path::Path) -> Option<u64> {
    link.to_str()?.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok()
}

/// Socket inodes `pid` holds that are in `wanted`.
fn scan_fds(pid: u32, wanted: &HashSet<u64>, found: &mut HashMap<u64, u32>) {
    let Ok(dir) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { return };
    for entry in dir.flatten() {
        if let Some(inode) = std::fs::read_link(entry.path()).ok().as_deref().and_then(socket_inode)
            && wanted.contains(&inode)
        {
            found.entry(inode).or_insert(pid);
        }
    }
}

/// Finds which of the user's processes hold the sockets in `wanted`,
/// trying the PIDs of `hint` (last scan's inode → PID) first.
pub fn find_owners(wanted: &HashSet<u64>, hint: &HashMap<u64, u32>) -> HashMap<u64, u32> {
    let mut found: HashMap<u64, u32> = HashMap::new();
    let mut scanned: HashSet<u32> = HashSet::new();
    for pid in wanted.iter().filter_map(|i| hint.get(i)) {
        if scanned.insert(*pid) {
            scan_fds(*pid, wanted, &mut found);
        }
    }
    if found.len() == wanted.len() {
        return found;
    }
    let me = my_uid();
    let Ok(proc_dir) = std::fs::read_dir("/proc") else { return found };
    for entry in proc_dir.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if scanned.contains(&pid) || entry.metadata().map(|m| m.uid()).ok() != Some(me) {
            continue;
        }
        scan_fds(pid, wanted, &mut found);
        if found.len() == wanted.len() {
            break;
        }
    }
    found
}

/// Name and command line of `pid`.
pub fn owner(pid: u32) -> Option<Owner> {
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_string();
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let command: String = raw.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s)).collect::<Vec<_>>().join(" ").chars().take(COMMAND_MAX_CHARS).collect();
    Some(Owner { pid, name, command })
}

/// Every listener with its owning process where readable. `hint` carries
/// inode → PID from the previous call and is updated in place.
pub fn listeners(hint: &mut HashMap<u64, u32>) -> Vec<Listener> {
    let sockets = sockets();
    let me = my_uid();
    let wanted: HashSet<u64> = sockets.iter().filter(|s| s.uid == me).map(|s| s.inode).collect();
    let pids = find_owners(&wanted, hint);
    let mut owners: HashMap<u64, Owner> = HashMap::new();
    let mut by_pid: HashMap<u32, Option<Owner>> = HashMap::new();
    for (inode, pid) in &pids {
        if let Some(o) = by_pid.entry(*pid).or_insert_with(|| owner(*pid)).clone() {
            owners.insert(*inode, o);
        }
    }
    *hint = pids;
    merge(&sockets, &owners)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:7B0B 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 57184 1 0000000000000000 100 0 0 10 0
   1: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 23410 1 0000000000000000 100 0 0 10 0
   2: 0501A8C0:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 99100 1 0000000000000000 100 0 0 10 0
   3: 0100007F:B5C2 0100007F:7B0B 01 00000000:00000000 00:00000000 00000000  1000        0 61111 1 0000000000000000 20 4 30 10 -1";

    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:0016 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 23411 1 0000000000000000 100 0 0 10 0
   1: 00000000000000000000000001000000:1F40 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 63682 2 0000000000000000 100 0 0 10 0";

    const UDP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops
  199: 00000000:14E9 00000000:0000 07 00000000:00000000 00:00000000 00000000   974        0 15614 2 0000000000000000 0
  653: 0100007F:ACA3 0100007F:0035 01 00000000:00000000 00:00000000 00000000  1000        0 22944586 2 0000000000000000 0";

    #[test]
    fn parses_ipv4_hex_little_endian() {
        assert_eq!(parse_addr("0100007F"), Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert_eq!(parse_addr("0501A8C0"), Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5))));
        assert_eq!(parse_addr("00000000"), Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        assert_eq!(parse_addr("XYZ"), None);
    }

    #[test]
    fn parses_ipv6_words_little_endian() {
        assert_eq!(parse_addr("00000000000000000000000001000000"), Some(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert_eq!(parse_addr("00000000000000000000000000000000"), Some(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
        // fe80::1 → first word 0x80fe little-endian, last word 0x01000000.
        assert_eq!(parse_addr("000080FE000000000000000001000000"), Some("fe80::1".parse().unwrap()));
        // ::ffff:127.0.0.1 (IPv4-mapped).
        assert_eq!(parse_addr("0000000000000000FFFF00000100007F"), Some("::ffff:127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn keeps_only_listening_tcp_and_unconnected_udp() {
        let tcp = parse_table(TCP, Proto::Tcp);
        assert_eq!(tcp.iter().map(|s| s.port).collect::<Vec<_>>(), [31499, 22, 8080], "the established socket is skipped");
        assert_eq!(tcp[0], Socket { proto: Proto::Tcp, addr: IpAddr::V4(Ipv4Addr::LOCALHOST), port: 0x7B0B, uid: 1000, inode: 57184 });
        let udp = parse_table(UDP, Proto::Udp);
        assert_eq!(udp.len(), 1, "a connected UDP socket is not a listener");
        assert_eq!((udp[0].port, udp[0].uid), (5353, 974));
        let tcp6 = parse_table(TCP6, Proto::Tcp);
        assert_eq!(tcp6[1].addr, IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(tcp6[1].port, 8000);
    }

    #[test]
    fn merges_dual_stack_and_classifies_scope() {
        let mut sockets = parse_table(TCP, Proto::Tcp);
        sockets.extend(parse_table(TCP6, Proto::Tcp));
        let mut owners = HashMap::new();
        owners.insert(57184, Owner { pid: 4242, name: "node".into(), command: "node server.js".into() });
        let ls = merge(&sockets, &owners);
        let ssh = ls.iter().find(|l| l.port == 22).unwrap();
        assert_eq!(ssh.addresses.len(), 2, "IPv4 and IPv6 sshd sockets are one row");
        assert_eq!((ssh.scope, ssh.pid), (Scope::AllInterfaces, None));
        assert_eq!(ssh.address(), "localhost:22");
        let node = ls.iter().find(|l| l.port == 0x7B0B).unwrap();
        assert_eq!((node.scope, node.pid, node.process.as_deref()), (Scope::Localhost, Some(4242), Some("node")));
        assert_eq!(node.url().as_deref(), Some("http://localhost:31499"));
        let lan = ls.iter().find(|l| l.port == 8080).unwrap();
        assert_eq!((lan.scope, lan.address()), (Scope::Address, "192.168.1.5:8080".to_string()));
        assert!(ls.windows(2).all(|w| w[0].port <= w[1].port), "sorted by port");
        assert_eq!(tcp_port_count(&sockets), 4);
    }

    #[test]
    fn reads_socket_links() {
        assert_eq!(socket_inode(std::path::Path::new("socket:[57184]")), Some(57184));
        assert_eq!(socket_inode(std::path::Path::new("pipe:[57184]")), None);
        assert_eq!(socket_inode(std::path::Path::new("/dev/null")), None);
    }

    #[test]
    // FreeBSD has no /proc/net; the Ports plugin is locked there.
    #[cfg(not(target_os = "freebsd"))]
    fn finds_its_own_listening_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut hint = HashMap::new();
        let ls = listeners(&mut hint);
        let mine = ls.iter().find(|l| l.port == port && l.proto == Proto::Tcp).expect("the test's listener is listed");
        assert_eq!(mine.pid, Some(std::process::id()));
        assert_eq!(mine.scope, Scope::Localhost);
        assert!(!hint.is_empty(), "the hint remembers inode → PID");
    }
}
