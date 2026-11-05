//! KWin's native ScreenShot2 D-Bus API. Pixel bytes travel over a private
//! descriptor; no Qt library, screenshot utility or temporary file is needed.

use std::collections::HashMap;
use std::io::Read as _;
use std::os::fd::AsFd as _;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use futures::future::{Either, select};
use zbus::zvariant::{Fd, OwnedValue, Value};

use super::{Capture, OutputInfo, RGBA_BYTES};

const INTERFACE: &str = "org.kde.KWin.ScreenShot2";
const OBJECT: &str = "/org/kde/KWin/ScreenShot2";
/// A compositor should return a still frame promptly; stalled calls and
/// incomplete descriptor transfers must not strand a capture worker.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Enough for a 16K RGBA output, with a firm allocation ceiling.
const MAX_FRAME_BYTES: usize = 1024 * 1024 * 1024;
/// QImage::Format values, from Qt's public image format contract.
const RGB32: u32 = 4;
const ARGB32: u32 = 5;
const ARGB32_PREMULTIPLIED: u32 = 6;
const RGBX8888: u32 = 16;
const RGBA8888: u32 = 17;
const RGBA8888_PREMULTIPLIED: u32 = 18;

type Metadata = HashMap<String, OwnedValue>;

struct Frame {
    width: u32,
    height: u32,
    stride: usize,
    format: u32,
    bytes: usize,
}

impl Frame {
    fn parse(meta: &Metadata) -> Result<Self> {
        if meta.get("type").and_then(|v| <&str>::try_from(v).ok()) != Some("raw") { bail!("KWin returned a non-raw frame"); }
        let number = |key| meta.get(key).and_then(|v| u32::try_from(v).ok()).ok_or_else(|| anyhow!("KWin frame has no {key}"));
        let (width, height, stride, format) = (number("width")?, number("height")?, number("stride")? as usize, number("format")?);
        let row = (width as usize).checked_mul(RGBA_BYTES).context("KWin frame width overflow")?;
        let bytes = stride.checked_mul(height as usize).context("KWin frame size overflow")?;
        if width == 0 || height == 0 || stride < row || bytes > MAX_FRAME_BYTES { bail!("invalid KWin frame dimensions"); }
        if !matches!(format, RGB32 | ARGB32 | ARGB32_PREMULTIPLIED | RGBX8888 | RGBA8888 | RGBA8888_PREMULTIPLIED) {
            bail!("unsupported QImage pixel format {format}");
        }
        Ok(Self { width, height, stride, format, bytes })
    }

    fn rgba(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() != self.bytes { bail!("incomplete KWin frame"); }
        let row = self.width as usize * RGBA_BYTES;
        let mut rgba = Vec::with_capacity(row * self.height as usize);
        for source in data.chunks_exact(self.stride) {
            for pixel in source[..row].as_chunks::<RGBA_BYTES>().0 {
                let mut p = if matches!(self.format, RGB32 | ARGB32 | ARGB32_PREMULTIPLIED) {
                    let value = u32::from_ne_bytes(*pixel);
                    [(value >> 16) as u8, (value >> 8) as u8, value as u8, (value >> 24) as u8]
                } else { *pixel };
                if matches!(self.format, RGB32 | RGBX8888) { p[3] = u8::MAX; }
                if matches!(self.format, ARGB32_PREMULTIPLIED | RGBA8888_PREMULTIPLIED) && p[3] != u8::MAX {
                    let alpha = u32::from(p[3]);
                    for channel in &mut p[..3] { *channel = (u32::from(*channel) * 255 + alpha / 2).checked_div(alpha).unwrap_or(0).min(255) as u8; }
                }
                rgba.extend_from_slice(&p);
            }
        }
        Ok(rgba)
    }
}

pub(super) fn capture(outputs: &[&OutputInfo], cursor: bool) -> Result<Vec<Capture>> {
    let conn = crate::runtime::block_on(crate::runtime::session())?;
    capture_on(&conn, outputs, cursor)
}

fn capture_on(conn: &zbus::Connection, outputs: &[&OutputInfo], cursor: bool) -> Result<Vec<Capture>> {
    let mut captures = Vec::with_capacity(outputs.len());
    for info in outputs {
        if info.name.is_empty() || info.logical.2 <= 0 || info.logical.3 <= 0 { bail!("KWin output geometry is incomplete"); }
        let (mut reader, writer) = UnixStream::pair()?;
        let metadata: Metadata = crate::runtime::block_on(async {
            let proxy = crate::actor::proxy(conn, INTERFACE, OBJECT, INTERFACE).await?;
            let options = HashMap::from([
                ("include-cursor", Value::from(cursor)),
                ("native-resolution", Value::from(true)),
                // Mandara has already hidden its capture UI. Keep its bar
                // and desktop visible in screenshots, as on other backends.
                ("hide-caller-windows", Value::from(false)),
            ]);
            let body = (info.name.as_str(), options, Fd::from(writer.as_fd()));
            match select(Box::pin(proxy.call("CaptureScreen", &body)), Box::pin(async_io::Timer::after(TIMEOUT))).await {
                Either::Left((result, _)) => result,
                Either::Right(_) => Err(zbus::Error::Failure("KWin screenshot timed out".into())),
            }
        }).context("KWin ScreenShot2")?;
        drop(writer);
        let frame = Frame::parse(&metadata)?;
        let mut data = vec![0; frame.bytes];
        let deadline = Instant::now() + TIMEOUT;
        let mut offset = 0;
        while offset < data.len() {
            let remaining = deadline.checked_duration_since(Instant::now()).context("KWin screenshot transfer timed out")?;
            reader.set_read_timeout(Some(remaining))?;
            let read = reader.read(&mut data[offset..])?;
            if read == 0 { bail!("incomplete KWin screenshot transfer"); }
            offset += read;
        }
        captures.push(Capture { output: info.name.clone(), logical: info.logical, width: frame.width, height: frame.height, rgba: frame.rgba(&data)? });
    }
    Ok(captures)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(format: u32) -> Metadata {
        HashMap::from([
            ("type".into(), Value::from("raw").try_into().unwrap()),
            ("width".into(), 2u32.into()), ("height".into(), 1u32.into()),
            ("stride".into(), 12u32.into()), ("format".into(), format.into()),
        ])
    }

    #[test]
    fn raw_qimage_pixels_keep_channels_alpha_and_row_padding() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0xff112233u32.to_ne_bytes());
        bytes.extend_from_slice(&0x80402010u32.to_ne_bytes());
        bytes.extend_from_slice(&[9; 4]);
        assert_eq!(Frame::parse(&metadata(ARGB32)).unwrap().rgba(&bytes).unwrap(), [17, 34, 51, 255, 64, 32, 16, 128]);
        assert_eq!(Frame::parse(&metadata(ARGB32_PREMULTIPLIED)).unwrap().rgba(&bytes).unwrap(), [17, 34, 51, 255, 128, 64, 32, 128]);
        let rgba = [1, 2, 3, 0, 4, 5, 6, 255, 9, 9, 9, 9];
        assert_eq!(Frame::parse(&metadata(RGBX8888)).unwrap().rgba(&rgba).unwrap(), [1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(Frame::parse(&metadata(RGBA8888_PREMULTIPLIED)).unwrap().rgba(&rgba).unwrap(), [0, 0, 0, 0, 4, 5, 6, 255]);
    }

    #[test]
    fn malformed_or_excessive_frames_are_rejected_before_allocation() {
        for (key, value) in [("width", 0), ("height", u32::MAX), ("stride", 7), ("format", u32::MAX)] {
            let mut meta = metadata(RGB32);
            meta.insert(key.into(), value.into());
            assert!(Frame::parse(&meta).is_err(), "{key}");
        }
        assert!(Frame::parse(&metadata(RGB32)).unwrap().rgba(&[0; 8]).is_err());
    }

    struct Screenshot;

    #[zbus::interface(name = "org.kde.KWin.ScreenShot2")]
    impl Screenshot {
        fn capture_screen(&self, name: &str, options: Metadata, pipe: zbus::zvariant::OwnedFd) -> Metadata {
            use std::io::Write as _;
            assert_eq!(name, "fixture");
            assert_eq!(options.get("include-cursor").and_then(|v| bool::try_from(v).ok()), Some(true));
            assert_eq!(options.get("native-resolution").and_then(|v| bool::try_from(v).ok()), Some(true));
            assert_eq!(options.get("hide-caller-windows").and_then(|v| bool::try_from(v).ok()), Some(false));
            let fd: std::os::fd::OwnedFd = pipe.into();
            let mut writer = std::fs::File::from(fd);
            writer.write_all(&[1, 2, 3, 255, 4, 5, 6, 255, 0, 0, 0, 0]).unwrap();
            metadata(RGBA8888)
        }
    }

    #[test]
    fn native_dbus_capture_transfers_pixels_over_the_supplied_descriptor() {
        let (client, server) = UnixStream::pair().unwrap();
        let (client, _server) = crate::runtime::block_on(async {
            let server = zbus::connection::Builder::async_io_unix_stream(server).server(zbus::Guid::generate()).unwrap().p2p().serve_at(OBJECT, Screenshot).unwrap();
            let client = zbus::connection::Builder::async_io_unix_stream(client).p2p();
            let (client, server) = futures::join!(crate::runtime::connect(client), crate::runtime::connect(server));
            (client.unwrap(), server.unwrap())
        });
        let output = OutputInfo { name: "fixture".into(), logical: (-2, 5, 2, 1) };
        let captures = capture_on(&client, &[&output], true).unwrap();
        assert_eq!(captures[0].logical, output.logical);
        assert_eq!(captures[0].rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    #[ignore = "requires the isolated KWin container and its marker"]
    fn native_kwin_screenshot_uses_only_the_isolated_session() {
        let runtime = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
        assert!(runtime.starts_with("/tmp") || runtime.starts_with("/var/tmp"));
        assert_eq!(std::fs::read_to_string(runtime.join(".mandara-kwin-test")).unwrap().trim(), "mandara isolated kwin test");
        assert_eq!(std::env::var("DBUS_SYSTEM_BUS_ADDRESS").as_deref(), Ok("unix:path=/nonexistent"));
        let frames = super::super::capture_all().unwrap();
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().any(|frame| frame.rgba.as_chunks::<RGBA_BYTES>().0.iter().any(|p| p[..3] != [0, 0, 0])), "start a colored fixture window before this test");
        for (index, frame) in frames.into_iter().enumerate() {
            assert_eq!(frame.rgba.len(), frame.width as usize * frame.height as usize * RGBA_BYTES);
            assert!(frame.width > 0 && frame.height > 0);
            let png = crate::screenshot::encode_png(&frame.to_image()).unwrap();
            std::fs::write(runtime.join(format!("capture-{index}.png")), png).unwrap();
        }
    }
}
