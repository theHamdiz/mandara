//! Counts online CPU rows without keeping the complete procfs table.

use std::io::{self, Read};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A read window, not a limit: the parser continues until EOF regardless of
/// the number of CPUs or the length of other rows in `/proc/stat`.
const READ_BYTES: usize = 1024;

struct Capacity(AtomicUsize);
static CAPACITY: Capacity = Capacity(AtomicUsize::new(0));

impl Capacity {
    fn update(&self, count: io::Result<usize>) -> usize {
        let capacity = count.unwrap_or_else(|_| self.0.load(Ordering::Relaxed)).max(1);
        self.0.store(capacity, Ordering::Relaxed);
        capacity
    }
}

pub(super) fn get() -> usize {
    let cached = CAPACITY.0.load(Ordering::Relaxed);
    if cached == 0 { refresh() } else { cached }
}

/// Refreshes the host-wide count on a background sample, including hotplug.
pub(super) fn refresh() -> usize {
    CAPACITY.update(std::fs::File::open("/proc/stat").and_then(count))
}

#[derive(Default)]
struct CpuRow {
    prefix: u8,
    digit: bool,
    invalid: bool,
    ended: bool,
}

impl CpuRow {
    fn push(&mut self, byte: u8) {
        if self.ended {
            return;
        }
        if byte.is_ascii_whitespace() {
            self.ended = self.prefix > 0 || self.invalid;
        } else if self.prefix < 3 {
            self.invalid |= byte != b"cpu"[usize::from(self.prefix)];
            self.prefix += 1;
        } else {
            self.invalid |= !byte.is_ascii_digit();
            self.digit = true;
        }
    }

    fn matches(&self) -> bool {
        self.prefix == 3 && self.digit && !self.invalid
    }
}

fn count(mut reader: impl Read) -> io::Result<usize> {
    let mut buffer = [0; READ_BYTES];
    let mut row = CpuRow::default();
    let mut total = 0;
    loop {
        let size = match reader.read(&mut buffer) {
            Ok(0) => return Ok(total + usize::from(row.matches())),
            Ok(size) => size,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        for &byte in &buffer[..size] {
            if byte == b'\n' {
                total += usize::from(row.matches());
                row = CpuRow::default();
            } else {
                row.push(byte);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_follows_online_cpus_and_retains_its_last_count_on_failure() {
        let capacity = Capacity(AtomicUsize::new(0));
        let missing = || Err(io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(capacity.update(missing()), 1);
        for (text, expected) in [
            ("cpu 1 2 3 4\ncpu0 1 2 3 4\ncpu1 1 2 3 4\n", 2),
            ("cpu 1 2 3 4\ncpu0 1 2 3 4\n", 1),
            ("cpu 1 2 3 4\ncpu0 1 2 3 4\ncpu7 1 2 3 4\n", 2),
        ] {
            assert_eq!(capacity.update(count(text.as_bytes())), expected);
        }
        assert_eq!(capacity.update(missing()), 2);
    }

    #[test]
    fn counts_only_complete_cpu_names_in_the_first_field() {
        let stat = "cpu 1\ncpu0 1\n cpu12\n\tcpu4\t2\ncpu 2\ncpux 1\ncpu1x 1\nxcpu1 1\nintr cpu2\ncpu8";
        assert_eq!(count(stat.as_bytes()).unwrap(), 4);
        assert_eq!(count(b"cpu 100 200\n".as_slice()).unwrap(), 0);
        assert_eq!(count(b"".as_slice()).unwrap(), 0);
    }

    struct Chunks<'a> {
        bytes: &'a [u8],
        size: usize,
        interrupt: bool,
    }

    impl Read for Chunks<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if std::mem::take(&mut self.interrupt) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let size = self.size.min(output.len()).min(self.bytes.len());
            output[..size].copy_from_slice(&self.bytes[..size]);
            self.bytes = &self.bytes[size..];
            Ok(size)
        }
    }

    #[test]
    fn table_and_lines_can_cross_any_number_of_read_windows() {
        use std::fmt::Write as _;
        let mut stat = "intr ".to_string();
        stat.extend(std::iter::repeat_n('0', READ_BYTES * 3));
        stat.push('\n');
        for index in 0..2048 {
            writeln!(stat, "cpu{index} 1 2 3 4").unwrap();
        }
        for size in [1, 2, 3, 7, READ_BYTES] {
            assert_eq!(count(Chunks { bytes: stat.as_bytes(), size, interrupt: true }).unwrap(), 2048);
        }
    }

    #[test]
    fn failed_read_does_not_publish_a_partial_count() {
        struct Failing(bool);

        impl Read for Failing {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                if std::mem::replace(&mut self.0, false) {
                    let bytes = b"cpu0 1\n";
                    output[..bytes.len()].copy_from_slice(bytes);
                    Ok(bytes.len())
                } else {
                    Err(io::ErrorKind::PermissionDenied.into())
                }
            }
        }
        let capacity = Capacity(AtomicUsize::new(9));
        assert_eq!(capacity.update(count(Failing(true))), 9);
    }
}
