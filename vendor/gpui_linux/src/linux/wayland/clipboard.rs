use std::{
    io::{ErrorKind, Write},
    os::fd::{AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd},
};

use calloop::{LoopHandle, PostAction};
use filedescriptor::{FileDescriptor, Pipe};
use strum::IntoEnumIterator;
use wayland_client::{Connection, protocol::wl_data_offer::WlDataOffer};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1;

use crate::linux::platform::{PIPE_READ_TIMEOUT, read_fd_with_timeout};
use gpui::{ClipboardEntry, ClipboardItem, Image, ImageFormat, hash};
use gpui_util::ResultExt as _;

/// Text mime types that we'll offer to other programs.
pub(crate) const TEXT_MIME_TYPES: [&str; 3] =
    ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];
pub(crate) const FILE_LIST_MIME_TYPE: &str = "text/uri-list";

/// Text mime types that we'll accept from other programs.
pub(crate) const ALLOWED_TEXT_MIME_TYPES: [&str; 2] = ["text/plain;charset=utf-8", "UTF8_STRING"];

pub(crate) struct Clipboard {
    connection: Connection,
    loop_handle: LoopHandle<'static, ()>,
    self_mime: String,

    // Internal clipboard
    contents: Option<ClipboardItem>,
    primary_contents: Option<ClipboardItem>,

    // External clipboard
    cached_read: Option<ClipboardItem>,
    current_offer: Option<DataOffer<WlDataOffer>>,
    cached_primary_read: Option<ClipboardItem>,
    current_primary_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>,
}

pub(crate) trait ReceiveData {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>);
}

impl ReceiveData for WlDataOffer {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

impl ReceiveData for ZwpPrimarySelectionOfferV1 {
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

#[derive(Clone, Debug)]
/// Wrapper for `WlDataOffer` and `ZwpPrimarySelectionOfferV1`, used to help track mime types.
pub(crate) struct DataOffer<T: ReceiveData> {
    pub inner: T,
    mime_types: Vec<String>,
}

impl<T: ReceiveData> DataOffer<T> {
    pub fn new(offer: T) -> Self {
        Self {
            inner: offer,
            mime_types: Vec::new(),
        }
    }

    pub fn add_mime_type(&mut self, mime_type: String) {
        self.mime_types.push(mime_type)
    }

    /// Mandara patch 13: the offered MIME types, for data drops.
    pub(crate) fn mime_types(&self) -> &[String] {
        &self.mime_types
    }

    fn has_mime_type(&self, mime_type: &str) -> bool {
        self.mime_types.iter().any(|t| t == mime_type)
    }

    fn read_bytes(&self, connection: &Connection, mime_type: &str) -> Option<Vec<u8>> {
        let pipe = Pipe::new().unwrap();
        self.inner.receive_data(mime_type.to_string(), unsafe {
            BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
        });
        let fd = pipe.read;
        drop(pipe.write);

        // Mandara patch 5: a full socket or a dead connection is no reason to
        // panic on a paste; the read below then simply times out.
        connection.flush().log_err();

        match read_fd_with_timeout(fd, PIPE_READ_TIMEOUT) {
            Ok(bytes) => Some(bytes),
            Err(err) => {
                log::error!("error reading clipboard pipe: {err:?}");
                None
            }
        }
    }

    fn read_text(&self, connection: &Connection) -> Option<ClipboardItem> {
        let mime_type = self.mime_types.iter().find(|&mime_type| {
            ALLOWED_TEXT_MIME_TYPES
                .iter()
                .any(|&allowed| allowed == mime_type)
        })?;
        let bytes = self.read_bytes(connection, mime_type)?;
        let text_content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(e) => {
                log::error!("Failed to convert clipboard content to UTF-8: {}", e);
                return None;
            }
        };

        // Normalize the text to unix line endings, otherwise
        // copying from eg: firefox inserts a lot of blank
        // lines, and that is super annoying.
        let result = text_content.replace("\r\n", "\n");
        Some(ClipboardItem::new_string(result))
    }

    fn read_image(&self, connection: &Connection) -> Option<ClipboardItem> {
        for format in ImageFormat::iter() {
            let mime_type = format.mime_type();
            if !self.has_mime_type(mime_type) {
                continue;
            }

            if let Some(bytes) = self.read_bytes(connection, mime_type) {
                let id = hash(&bytes);
                return Some(ClipboardItem {
                    entries: vec![ClipboardEntry::Image(Image { format, bytes, id })],
                });
            }
        }
        None
    }
}

impl Clipboard {
    pub fn new(connection: Connection, loop_handle: LoopHandle<'static, ()>) -> Self {
        Self {
            connection,
            loop_handle,
            self_mime: format!("pid/{}", std::process::id()),

            contents: None,
            primary_contents: None,

            cached_read: None,
            current_offer: None,
            cached_primary_read: None,
            current_primary_offer: None,
        }
    }

    pub fn set(&mut self, item: ClipboardItem) {
        self.contents = Some(item);
    }

    pub fn set_primary(&mut self, item: ClipboardItem) {
        self.primary_contents = Some(item);
    }

    pub fn set_offer(&mut self, data_offer: Option<DataOffer<WlDataOffer>>) {
        self.cached_read = None;
        self.current_offer = data_offer;
    }

    pub fn set_primary_offer(&mut self, data_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>) {
        self.cached_primary_read = None;
        self.current_primary_offer = data_offer;
    }

    pub fn self_mime(&self) -> String {
        self.self_mime.clone()
    }

    pub fn send(&self, _mime_type: String, fd: OwnedFd) {
        if let Some(text) = self.contents.as_ref().and_then(|contents| contents.text()) {
            self.send_bytes(fd, text.as_bytes().to_owned());
        }
    }

    pub fn send_primary(&self, _mime_type: String, fd: OwnedFd) {
        if let Some(text) = self
            .primary_contents
            .as_ref()
            .and_then(|contents| contents.text())
        {
            self.send_bytes(fd, text.as_bytes().to_owned());
        }
    }

    pub fn read(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_offer.as_ref()?;
        if let Some(cached) = self.cached_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.contents.clone();
        }

        let item = offer
            .read_text(&self.connection)
            .or_else(|| offer.read_image(&self.connection))?;

        self.cached_read = Some(item.clone());
        Some(item)
    }

    pub fn read_primary(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_primary_offer.as_ref()?;
        if let Some(cached) = self.cached_primary_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.primary_contents.clone();
        }

        let item = offer
            .read_text(&self.connection)
            .or_else(|| offer.read_image(&self.connection))?;

        self.cached_primary_read = Some(item.clone());
        Some(item)
    }

    /// Mandara patch 13: the connection, flushed after a data drop's receive.
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Writes `bytes` to `fd` from the event loop as the receiver drains the
    /// pipe; see [`write_when_writable`].
    pub fn send_bytes(&self, fd: OwnedFd, bytes: impl AsRef<[u8]> + 'static) {
        write_when_writable(&self.loop_handle, fd, bytes);
    }
}

/// Mandara patch 5: writes `bytes` to `fd` as the receiver drains the pipe.
/// The fd is made non-blocking first, so a large or stalled receiver never
/// blocks the event loop in `write` (a blocking pipe would hold it until the
/// receiver had read every byte), and the bytes are shared (`Arc<[u8]>`), never
/// copied per request.
pub(crate) fn write_when_writable<D: 'static>(
    loop_handle: &LoopHandle<'static, D>,
    fd: OwnedFd,
    bytes: impl AsRef<[u8]> + 'static,
) {
    // SAFETY: `into_raw_fd` hands over sole ownership of an open fd.
    let mut fd = unsafe { FileDescriptor::from_raw_fd(fd.into_raw_fd()) };
    if let Err(err) = fd.set_non_blocking(true) {
        log::error!("cannot make the data pipe non-blocking: {err:#}");
        return;
    }
    let mut written = 0;
    if let Err(err) = loop_handle.insert_source(
        calloop::generic::Generic::new(fd, calloop::Interest::WRITE, calloop::Mode::Level),
        move |_, fd, _| {
            let bytes = bytes.as_ref();
            // SAFETY: the descriptor is only written to, never replaced.
            let fd = unsafe { fd.get_mut() };
            loop {
                if written == bytes.len() {
                    break Ok(PostAction::Remove);
                }
                match fd.write(&bytes[written..]) {
                    Ok(0) => break Ok(PostAction::Remove),
                    Ok(n) => written += n,
                    Err(err) if err.kind() == ErrorKind::Interrupted => {}
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {
                        break Ok(PostAction::Continue);
                    }
                    Err(_) => break Ok(PostAction::Remove),
                }
            }
        },
    ) {
        log::error!("cannot send data through the pipe: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, os::fd::FromRawFd as _, sync::Arc, time::Duration};

    /// Mandara patch 5: a payload far larger than the pipe's buffer is
    /// written as the reader drains it, and a reader that has not read yet
    /// never blocks the event loop.
    #[test]
    fn a_large_payload_never_blocks_the_event_loop() {
        let payload: Arc<[u8]> = (0..2 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
        let mut event_loop = calloop::EventLoop::<()>::try_new().unwrap();
        let pipe = Pipe::new().unwrap();
        // SAFETY: the pipe's write end is owned and handed over here.
        let write = unsafe { OwnedFd::from_raw_fd(pipe.write.into_raw_fd()) };
        write_when_writable(&event_loop.handle(), write, payload.clone());

        // Nobody reads yet: the loop fills the pipe, then returns instead of blocking.
        for _ in 0..4 {
            event_loop.dispatch(Some(Duration::from_millis(10)), &mut ()).unwrap();
        }

        let mut read = pipe.read;
        read.set_non_blocking(true).unwrap();
        let mut received = Vec::with_capacity(payload.len());
        let mut chunk = [0u8; 16 * 1024];
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while received.len() < payload.len() {
            assert!(std::time::Instant::now() < deadline, "the payload never arrived");
            event_loop.dispatch(Some(Duration::from_millis(10)), &mut ()).unwrap();
            loop {
                match read.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => received.extend_from_slice(&chunk[..n]),
                    Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                    Err(err) => panic!("read failed: {err}"),
                }
            }
        }
        assert_eq!(received.as_slice(), &payload[..]);
        // The writer is done and closed its end: the reader sees the end.
        event_loop.dispatch(Some(Duration::from_millis(10)), &mut ()).unwrap();
        assert_eq!(read.read(&mut chunk).unwrap(), 0);
    }
}
