//! Command replies are serialized only when a caller is waiting.

use async_channel::Sender;

pub(super) fn send(waiters: &[Sender<Result<String, String>>], serialize: impl FnOnce() -> String) {
    let Some((last, others)) = waiters.split_last() else { return };
    let json = serialize();
    for waiter in others {
        let _ = waiter.try_send(Ok(json.clone()));
    }
    let _ = last.try_send(Ok(json));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn background_polls_do_not_serialize_command_replies() {
        send(&[], || panic!("no command requested this snapshot"));
    }

    #[test]
    fn coalesced_callers_receive_one_serialized_snapshot() {
        let calls = Cell::new(0);
        let (first, a) = async_channel::bounded(1);
        let (second, b) = async_channel::bounded(1);
        send(&[first, second], || {
            calls.set(calls.get() + 1);
            "{\"running\":2}".into()
        });
        assert_eq!(calls.get(), 1);
        assert_eq!(a.try_recv().unwrap(), Ok("{\"running\":2}".into()));
        assert_eq!(b.try_recv().unwrap(), Ok("{\"running\":2}".into()));
    }
}
