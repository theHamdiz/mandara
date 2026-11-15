//! Keeps warnings and errors when a compositor discards the process's stderr.

use std::{
    io::{self, Write},
    os::unix::net::UnixDatagram,
    path::Path,
};
use tracing::Metadata;
use tracing_subscriber::fmt::MakeWriter;

/// Caps foreground formatting storage and keeps each journal datagram small.
const RECORD_BYTES: usize = 8 * 1024;
/// Build identity is metadata; reserve the rest of the record for the error.
const BUILD_BYTES: usize = 512;
const TRUNCATED: &[u8] = b"\n[message truncated]";
const SOCKET: &str = "/run/systemd/journal/socket";

pub struct Journal {
    socket: UnixDatagram,
    build: &'static str,
}

impl Journal {
    pub fn connect(build: &'static str) -> Option<Self> {
        Self::connect_to(Path::new(SOCKET), build).ok()
    }

    fn connect_to(path: &Path, build: &'static str) -> io::Result<Self> {
        let socket = UnixDatagram::unbound()?;
        socket.set_nonblocking(true)?;
        socket.connect(path)?;
        Ok(Self { socket, build })
    }
}

impl<'a> MakeWriter<'a> for Journal {
    type Writer = Record<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        Record::new(self, b'3')
    }

    fn make_writer_for(&'a self, metadata: &Metadata<'_>) -> Self::Writer {
        Record::new(
            self,
            if *metadata.level() == tracing::Level::WARN {
                b'4'
            } else {
                b'3'
            },
        )
    }
}

pub struct Record<'a> {
    socket: &'a UnixDatagram,
    bytes: [u8; RECORD_BYTES],
    len: usize,
    length_offset: usize,
    message_offset: usize,
    truncated: bool,
}

impl<'a> Record<'a> {
    fn new(journal: &'a Journal, priority: u8) -> Self {
        let mut record = Self {
            socket: &journal.socket,
            bytes: [0; RECORD_BYTES],
            len: 0,
            length_offset: 0,
            message_offset: 0,
            truncated: false,
        };
        let _ = record.write_all(b"SYSLOG_IDENTIFIER=mandara\nPRIORITY=");
        let _ = record.write_all(&[priority]);
        let _ = record.write_all(b"\nMANDARA_BUILD=");
        let build = journal.build.as_bytes();
        let _ = record.write_all(&build[..build.len().min(BUILD_BYTES)]);
        let _ = record.write_all(b"\nMESSAGE\n");
        record.length_offset = record.len;
        record.len += size_of::<u64>();
        record.message_offset = record.len;
        record
    }
}

impl Write for Record<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let capacity = RECORD_BYTES - TRUNCATED.len() - 1;
        let count = bytes.len().min(capacity.saturating_sub(self.len));
        self.bytes[self.len..self.len + count].copy_from_slice(&bytes[..count]);
        self.len += count;
        self.truncated |= count < bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Record<'_> {
    fn drop(&mut self) {
        if self.truncated {
            self.bytes[self.len..self.len + TRUNCATED.len()].copy_from_slice(TRUNCATED);
            self.len += TRUNCATED.len();
        }
        let length = (self.len - self.message_offset) as u64;
        self.bytes[self.length_offset..self.message_offset].copy_from_slice(&length.to_le_bytes());
        self.bytes[self.len] = b'\n';
        self.len += 1;
        // A full or unavailable journal must never stall the foreground thread.
        let _ = self.socket.send(&self.bytes[..self.len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal() -> (Journal, UnixDatagram) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.sock");
        let receiver = UnixDatagram::bind(&path).unwrap();
        let journal = Journal::connect_to(&path, "test build").unwrap();
        receiver.set_nonblocking(true).unwrap();
        (journal, receiver)
    }

    fn message(record: &[u8]) -> &[u8] {
        let offset = record
            .windows(b"MESSAGE\n".len())
            .position(|part| part == b"MESSAGE\n")
            .unwrap()
            + b"MESSAGE\n".len();
        let length = u64::from_le_bytes(
            record[offset..offset + size_of::<u64>()]
                .try_into()
                .unwrap(),
        ) as usize;
        assert_eq!(&record[offset + size_of::<u64>() + length..], b"\n");
        &record[offset + size_of::<u64>()..offset + size_of::<u64>() + length]
    }

    #[test]
    fn multiline_errors_keep_their_message_and_build_identity() {
        let (journal, receiver) = journal();
        {
            let mut writer = journal.make_writer();
            writer.write_all(b"first line\nsecond\0line").unwrap();
        }
        let mut bytes = [0; RECORD_BYTES];
        let count = receiver.recv(&mut bytes).unwrap();
        assert!(
            bytes[..count]
                .starts_with(b"SYSLOG_IDENTIFIER=mandara\nPRIORITY=3\nMANDARA_BUILD=test build\n")
        );
        assert_eq!(message(&bytes[..count]), b"first line\nsecond\0line");
    }

    #[test]
    fn oversized_messages_stay_inside_one_bounded_datagram() {
        let (journal, receiver) = journal();
        {
            let mut writer = journal.make_writer();
            writer.write_all(&[b'x'; RECORD_BYTES * 2]).unwrap();
        }
        let mut bytes = [0; RECORD_BYTES];
        let count = receiver.recv(&mut bytes).unwrap();
        assert_eq!(count, RECORD_BYTES);
        assert!(message(&bytes[..count]).ends_with(TRUNCATED));
        assert_eq!(
            receiver.recv(&mut bytes).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn oversized_build_metadata_leaves_the_error_message_intact() {
        static BUILD: [u8; RECORD_BYTES * 2] = [b'v'; RECORD_BYTES * 2];
        let (mut journal, receiver) = journal();
        journal.build = std::str::from_utf8(&BUILD).unwrap();
        journal
            .make_writer()
            .write_all(b"the actual failure")
            .unwrap();
        let mut bytes = [0; RECORD_BYTES];
        let count = receiver.recv(&mut bytes).unwrap();
        assert!(count < RECORD_BYTES);
        assert_eq!(message(&bytes[..count]), b"the actual failure");
        let build = bytes[..count]
            .split(|&byte| byte == b'\n')
            .find(|line| line.starts_with(b"MANDARA_BUILD="))
            .unwrap();
        assert_eq!(build.len(), b"MANDARA_BUILD=".len() + BUILD_BYTES);
    }

    #[test]
    fn a_full_or_closed_journal_does_not_block_or_panic() {
        use std::os::fd::AsRawFd as _;
        let (journal, receiver) = journal();
        let socket = &journal.socket;
        // SAFETY: the journal owns this descriptor; F_GETFL only reads its flags.
        let flags = unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0 && flags & libc::O_NONBLOCK != 0);
        let error = loop {
            if let Err(error) = socket.send(&[0; RECORD_BYTES]) {
                break error;
            }
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        journal.make_writer().write_all(b"journal is full").unwrap();
        drop(receiver);
        journal
            .make_writer()
            .write_all(b"journal is closed")
            .unwrap();
    }

    #[test]
    fn the_journal_filter_keeps_warning_priority_and_skips_info() {
        let (journal, receiver) = journal();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(journal)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("routine message");
            tracing::warn!("crash warning");
        });
        let mut bytes = [0; RECORD_BYTES];
        let count = receiver.recv(&mut bytes).unwrap();
        assert!(
            bytes[..count]
                .windows(b"PRIORITY=4\n".len())
                .any(|part| part == b"PRIORITY=4\n")
        );
        assert!(String::from_utf8_lossy(message(&bytes[..count])).contains("crash warning"));
        assert_eq!(
            receiver.recv(&mut bytes).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
}
