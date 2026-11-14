//! Incremental UTF-8 JSON framing with independent frame, wire and reply ceilings.

use super::{MAX_REPLY_BYTES, protocol::Protocol};
use mandara_i18n::t;
use std::io::{BufRead as _, BufReader, Read};

/// A final AnythingLLM event may also contain document citations.
const MAX_FRAME_BYTES: usize = 64 * 1024;
/// Ignore neither endless keepalives nor unbounded hidden reasoning output.
const MAX_WIRE_BYTES: usize = 2 * 1024 * 1024;

pub(super) fn read(
    reader: impl Read,
    protocol: &dyn Protocol,
    cancelled: impl Fn() -> bool,
    sink: &mut impl FnMut(&str) -> bool,
) -> Result<(), String> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let mut event = String::new();
    let (mut wire, mut reply, mut complete) = (0usize, 0usize, false);
    loop {
        if cancelled() {
            return Ok(());
        }
        line.clear();
        let read = reader
            .by_ref()
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|_| t("The AI connection closed before the reply finished").to_string())?;
        if cancelled() {
            return Ok(());
        }
        if read == 0 {
            break;
        }
        wire = wire.saturating_add(read);
        if line.len() > MAX_FRAME_BYTES || wire > MAX_WIRE_BYTES {
            return Err(t("The AI response exceeded its size limit").into());
        }
        let text = std::str::from_utf8(&line)
            .map_err(|_| t("The AI server did not return valid text").to_string())?
            .trim_end_matches(['\r', '\n']);
        let payload = if protocol.sse() {
            if let Some(data) = text.strip_prefix("data:") {
                if !event.is_empty() {
                    event.push('\n');
                }
                event.push_str(data.strip_prefix(' ').unwrap_or(data));
                if event.len() > MAX_FRAME_BYTES {
                    return Err(t("The AI response exceeded its size limit").into());
                }
                continue;
            }
            if !text.is_empty() {
                continue;
            }
            if event.is_empty() {
                continue;
            }
            event.as_str()
        } else {
            if text.trim().is_empty() {
                continue;
            }
            text
        };
        if payload == "[DONE]" {
            complete = true;
            break;
        }
        let value = serde_json::from_str(payload)
            .map_err(|_| t("The AI server did not return valid JSON").to_string())?;
        let chunk = protocol.event(&value)?;
        reply = reply.saturating_add(chunk.text.len());
        if reply > MAX_REPLY_BYTES {
            return Err(t("The reply reached Mandara's size limit").into());
        }
        if !chunk.text.is_empty() && !sink(chunk.text) {
            return Ok(());
        }
        if chunk.done {
            complete = true;
            break;
        }
        event.clear();
    }
    if complete {
        Ok(())
    } else {
        Err(t("The AI connection closed before the reply finished").into())
    }
}
