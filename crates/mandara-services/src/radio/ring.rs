//! The jitter buffer between the network thread and the decoder: a byte
//! ring of fixed capacity, allocated once per session and sized from the
//! station's bitrate. The writer waits while it is full (the server then
//! slows down through TCP), the reader waits while it is empty, so it can
//! never grow. Either side gives up as soon as the session is cancelled.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// Smallest and largest ring (bytes).
pub const MIN_CAPACITY: usize = 32 * 1024;
pub const MAX_CAPACITY: usize = 1024 * 1024;
/// Bitrate assumed when the server doesn't say (kbit/s).
pub const DEFAULT_KBPS: u32 = 192;
/// How long a blocked side sleeps before looking at the cancel flag again.
const WAIT: Duration = Duration::from_millis(100);

/// Ring capacity for `seconds` of a `kbps` stream, within the bounds.
pub fn capacity_for(kbps: Option<u32>, seconds: f32) -> usize {
    let kbps = kbps.filter(|k| (8..=1_000).contains(k)).unwrap_or(DEFAULT_KBPS);
    let bytes = f64::from(kbps) * 1000.0 / 8.0 * f64::from(seconds.clamp(0.5, 30.0));
    (bytes as usize).clamp(MIN_CAPACITY, MAX_CAPACITY)
}

struct State {
    /// Index of the oldest byte.
    head: usize,
    len: usize,
    /// The reader ran dry and waits for [`Ring::refill`] bytes before it
    /// goes on (so a slow network rebuffers instead of stuttering).
    starved: bool,
    /// The writer finished this connection: the reader sees end of stream
    /// once it drained what is left, and the writer waits for [`Ring::reopen`].
    ended: bool,
    /// The session is over: both sides stop now.
    closed: bool,
}

/// A bounded single-producer, single-consumer byte ring.
pub struct Ring {
    buf: Mutex<(Box<[u8]>, State)>,
    cond: Condvar,
    cancel: AtomicBool,
    /// Mirror of `State::starved` for lock-free readers (the UI).
    starved: AtomicBool,
    refill: usize,
}

impl Ring {
    /// A ring of `capacity` bytes whose reader starts (and restarts after
    /// running dry) once `refill` bytes are in.
    pub fn new(capacity: usize, refill: usize) -> Self {
        let capacity = capacity.clamp(MIN_CAPACITY, MAX_CAPACITY);
        let state = State { head: 0, len: 0, starved: true, ended: false, closed: false };
        Self { buf: Mutex::new((vec![0u8; capacity].into_boxed_slice(), state)), cond: Condvar::new(), cancel: AtomicBool::new(false), starved: AtomicBool::new(true), refill: refill.clamp(1, capacity) }
    }

    /// Bytes the reader waits for before it starts or after running dry.
    pub fn refill(&self) -> usize {
        self.refill
    }

    /// The reader is waiting for the buffer to fill (start or underrun).
    pub fn starved(&self) -> bool {
        self.starved.load(Ordering::Relaxed)
    }

    fn lock(&self) -> MutexGuard<'_, (Box<[u8]>, State)> {
        self.buf.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn capacity(&self) -> usize {
        self.lock().0.len()
    }

    /// Bytes waiting to be decoded.
    pub fn len(&self) -> usize {
        self.lock().1.len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fill level, 0–1.
    pub fn fill(&self) -> f32 {
        let g = self.lock();
        g.1.len as f32 / g.0.len() as f32
    }

    /// Ends the session: every waiting side returns.
    pub fn close(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.lock().1.closed = true;
        self.cond.notify_all();
    }

    pub fn is_closed(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// The writer's connection ended: the reader gets end of stream after
    /// the bytes already here.
    pub fn end(&self) {
        self.lock().1.ended = true;
        self.cond.notify_all();
    }

    /// The reader restarts after an end of stream (a new connection).
    pub fn reopen(&self) {
        let mut g = self.lock();
        g.1.ended = false;
        g.1.starved = true;
        self.starved.store(true, Ordering::Relaxed);
        drop(g);
        self.cond.notify_all();
    }

    /// Appends all of `data`, waiting for room (and for a reopened ring
    /// after [`Ring::end`]). `false` once the session is closed.
    pub fn write_all(&self, mut data: &[u8]) -> bool {
        let mut g = self.lock();
        while !data.is_empty() {
            let (buf, s) = &mut *g;
            if s.closed {
                return false;
            }
            let cap = buf.len();
            let room = cap - s.len;
            if room == 0 || s.ended {
                g = self.cond.wait_timeout(g, WAIT).unwrap_or_else(std::sync::PoisonError::into_inner).0;
                continue;
            }
            let n = room.min(data.len());
            let tail = (s.head + s.len) % cap;
            let first = n.min(cap - tail);
            buf[tail..tail + first].copy_from_slice(&data[..first]);
            buf[..n - first].copy_from_slice(&data[first..n]);
            s.len += n;
            data = &data[n..];
            self.cond.notify_all();
        }
        true
    }

    /// Reads up to `out.len()` bytes. Waits while empty and, after running
    /// dry, until [`Ring::refill`] bytes are in again. `0` at the end of a
    /// connection or once closed.
    pub fn read_into(&self, out: &mut [u8]) -> usize {
        if out.is_empty() {
            return 0;
        }
        let mut g = self.lock();
        loop {
            let (buf, s) = &mut *g;
            if s.closed {
                return 0;
            }
            if s.starved && s.len >= self.refill.min(buf.len()) || s.starved && s.ended {
                s.starved = false;
                self.starved.store(false, Ordering::Relaxed);
            }
            if s.len > 0 && !s.starved {
                let cap = buf.len();
                let n = s.len.min(out.len());
                let first = n.min(cap - s.head);
                out[..first].copy_from_slice(&buf[s.head..s.head + first]);
                out[first..n].copy_from_slice(&buf[..n - first]);
                s.head = (s.head + n) % cap;
                s.len -= n;
                self.cond.notify_all();
                return n;
            }
            if s.len == 0 {
                if s.ended {
                    return 0;
                }
                if !s.starved {
                    s.starved = true;
                    self.starved.store(true, Ordering::Relaxed);
                }
            }
            g = self.cond.wait_timeout(g, WAIT).unwrap_or_else(std::sync::PoisonError::into_inner).0;
        }
    }
}

/// The decoder's view of the ring (`Read`, never seeks). For AAC it
/// rewrites MPEG-2 ADTS headers as MPEG-4 ones on the way (see [`Adts`]).
pub struct SharedReader {
    ring: std::sync::Arc<Ring>,
    adts: Option<Adts>,
}

impl SharedReader {
    pub fn new(ring: std::sync::Arc<Ring>, aac: bool) -> Self {
        Self { ring, adts: aac.then(Adts::default) }
    }
}

impl Read for SharedReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.ring.read_into(out);
        if let Some(a) = &mut self.adts {
            a.fix(&mut out[..n]);
        }
        Ok(n)
    }
}

/// Many AAC stations send MPEG-2 ADTS frames (sync `FFF9`); the decoder
/// only syncs on MPEG-4 ones (`FFF1`), and the two differ only in that ID
/// bit. This follows the frames through the byte stream (by their length
/// field, across reads) and clears the bit in each header, in place, so
/// audio bytes that happen to look like a sync word are never touched.
#[derive(Debug, Default)]
pub struct Adts {
    /// Payload bytes left in the current frame.
    skip: usize,
    /// Header bytes seen so far (0 = looking for a sync word).
    have: usize,
    hdr: [u8; 6],
}

impl Adts {
    pub fn fix(&mut self, data: &mut [u8]) {
        let mut i = 0;
        while i < data.len() {
            if self.skip > 0 {
                let n = self.skip.min(data.len() - i);
                self.skip -= n;
                i += n;
                continue;
            }
            let b = data[i];
            match self.have {
                0 => {
                    if b == 0xFF {
                        self.hdr[0] = b;
                        self.have = 1;
                    }
                }
                1 => {
                    if b & 0xF6 == 0xF0 {
                        // Sync, layer 00: an ADTS header. Clear the ID bit.
                        data[i] = b & !0x08;
                        self.hdr[1] = data[i];
                        self.have = 2;
                    } else {
                        self.have = usize::from(b == 0xFF);
                    }
                }
                n => {
                    self.hdr[n] = b;
                    self.have += 1;
                    if self.have == 6 {
                        let len = (usize::from(self.hdr[3] & 0x03) << 11) | (usize::from(self.hdr[4]) << 3) | usize::from(self.hdr[5] >> 5);
                        self.have = 0;
                        // A nonsense length: look for the next sync word.
                        self.skip = if len >= 7 { len - 6 } else { 0 };
                    }
                }
            }
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// An ADTS frame: sync with `id` (MPEG-2 = 1), the length field set,
    /// a payload of `fill` bytes (which may look like sync words).
    fn adts_frame(id: bool, payload: &[u8]) -> Vec<u8> {
        let len = payload.len() + 7;
        let mut f = vec![0xFF, if id { 0xF9 } else { 0xF1 }, 0x50, 0x80 | ((len >> 11) & 0x03) as u8, ((len >> 3) & 0xFF) as u8, (((len & 0x07) << 5) as u8) | 0x1F, 0xFC];
        f.extend_from_slice(payload);
        f
    }

    #[test]
    fn mpeg2_adts_headers_become_mpeg4_and_payloads_stay() {
        let payload = [0xFFu8, 0xF9, 0x12, 0xFF, 0xF1, 0x00, 0x42];
        let mut stream = Vec::new();
        stream.extend_from_slice(b"\x00\x01junk");
        for i in 0..20 {
            stream.extend(adts_frame(i % 3 != 0, &payload));
        }
        let mut expected = Vec::new();
        expected.extend_from_slice(b"\x00\x01junk");
        for _ in 0..20 {
            expected.extend(adts_frame(false, &payload));
        }
        for piece in [1usize, 2, 5, 7, 13, 1000] {
            let mut data = stream.clone();
            let mut fix = Adts::default();
            for chunk in data.chunks_mut(piece) {
                fix.fix(chunk);
            }
            assert_eq!(data, expected, "piece {piece}");
        }
    }

    #[test]
    fn capacity_follows_the_bitrate_within_bounds() {
        assert_eq!(capacity_for(Some(128), 4.0), 64_000);
        assert_eq!(capacity_for(Some(320), 10.0), 400_000);
        assert_eq!(capacity_for(None, 4.0), 96_000, "unknown: 192 kbit/s");
        assert_eq!(capacity_for(Some(0), 4.0), 96_000, "nonsense bitrates are unknown");
        assert_eq!(capacity_for(Some(32), 1.0), MIN_CAPACITY);
        assert_eq!(capacity_for(Some(1_000), 30.0), MAX_CAPACITY);
        assert_eq!(capacity_for(Some(128), f32::NAN.max(100.0)), 480_000, "seconds clamp to 30");
    }

    #[test]
    fn the_ring_never_grows_and_keeps_order() {
        let ring = Arc::new(Ring::new(MIN_CAPACITY, 1));
        assert_eq!(ring.capacity(), MIN_CAPACITY);
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let writer = {
            let (ring, data) = (ring.clone(), data.clone());
            std::thread::spawn(move || {
                for chunk in data.chunks(7_001) {
                    assert!(ring.write_all(chunk));
                    assert!(ring.len() <= MIN_CAPACITY);
                }
                ring.end();
            })
        };
        let mut got = Vec::new();
        let mut buf = [0u8; 4093];
        loop {
            let n = ring.read_into(&mut buf);
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        writer.join().unwrap();
        assert_eq!(got, data);
        assert_eq!(ring.capacity(), MIN_CAPACITY);
    }

    #[test]
    fn end_and_reopen_separate_connections() {
        let ring = Ring::new(MIN_CAPACITY, 1);
        assert!(ring.write_all(b"first"));
        ring.end();
        let mut buf = [0u8; 16];
        assert_eq!(ring.read_into(&mut buf), 5);
        assert_eq!(ring.read_into(&mut buf), 0, "end of stream after the old bytes");
        ring.reopen();
        assert!(ring.write_all(b"second"));
        assert_eq!(ring.read_into(&mut buf), 6);
        assert_eq!(&buf[..6], b"second");
    }

    #[test]
    fn closing_releases_both_sides() {
        let ring = Arc::new(Ring::new(MIN_CAPACITY, 1));
        assert!(ring.write_all(&vec![0u8; MIN_CAPACITY]));
        let blocked_writer = {
            let ring = ring.clone();
            std::thread::spawn(move || ring.write_all(b"more"))
        };
        let empty = Arc::new(Ring::new(MIN_CAPACITY, 1));
        let blocked_reader = {
            let empty = empty.clone();
            std::thread::spawn(move || empty.read_into(&mut [0u8; 8]))
        };
        std::thread::sleep(Duration::from_millis(50));
        ring.close();
        empty.close();
        assert!(!blocked_writer.join().unwrap());
        assert_eq!(blocked_reader.join().unwrap(), 0);
        assert!(ring.is_closed());
        assert_eq!(ring.read_into(&mut [0u8; 4]), 0);
    }

    #[test]
    fn a_dry_reader_rebuffers_before_going_on() {
        let ring = Arc::new(Ring::new(MIN_CAPACITY, 1000));
        assert!(ring.starved(), "starts by filling");
        assert!(ring.write_all(&[1u8; 999]));
        let reader = {
            let ring = ring.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let first = ring.read_into(&mut buf);
                // Drain, then run dry: the next read waits for a refill.
                let second = ring.read_into(&mut buf);
                (first, second)
            })
        };
        std::thread::sleep(Duration::from_millis(60));
        assert!(!reader.is_finished(), "999 bytes are below the refill level");
        assert!(ring.write_all(&[2u8; 1]));
        std::thread::sleep(Duration::from_millis(150));
        assert!(ring.write_all(&[3u8; 10]));
        std::thread::sleep(Duration::from_millis(150));
        // The reader took 1000, then 10 more arrived and it took them too, or
        // it is waiting for a refill; either way it never sees a partial refill.
        assert!(ring.write_all(&[4u8; 1000]));
        let (first, second) = reader.join().unwrap();
        assert_eq!(first, 1000);
        assert!(second == 10 || second == 1010, "{second}");
    }
}
