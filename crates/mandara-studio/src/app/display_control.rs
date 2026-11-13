//! Studio and CLI share the shell's display transaction and countdown.

use crate::perf::{QueryError, ShellQuery};
use gpui::{App, Task};
use mandara_config::outputs::OutputConfig;
use mandara_services::outputs;
use serde_json::{Value, json};

fn available(result: Result<Value, QueryError>) -> Result<Option<Value>, String> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(QueryError::Unavailable(_)) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn request(arguments: Vec<String>, cx: &mut App) -> Task<Result<Option<Value>, String>> {
    // A standalone test keeps its original owner if the shell starts later.
    let query = outputs::revert_remaining()
        .is_none()
        .then(|| cx.try_global::<ShellQuery>().cloned())
        .flatten();
    cx.spawn(async move |_| {
        let Some(query) = query else {
            return Ok(None);
        };
        available(mandara_services::runtime::unblock(move || (query.0)(arguments)).await)
    })
}

pub(super) struct Snapshot {
    pub heads: Vec<outputs::Head>,
    pub remaining: Option<std::time::Duration>,
}

pub(super) fn heads(cx: &mut App) -> Task<Result<Snapshot, String>> {
    let remote = request(vec!["display".into(), "list".into()], cx);
    cx.spawn(async move |_| {
        if let Some(value) = remote.await? {
            return Ok(Snapshot {
                heads: serde_json::from_value(value["heads"].clone())
                    .map_err(|error| error.to_string())?,
                remaining: value["revert_pending_secs"]
                    .as_u64()
                    .map(std::time::Duration::from_secs),
            });
        }
        outputs::jobs::run(|| {
            outputs::heads().map(|heads| Snapshot {
                heads,
                remaining: outputs::revert_remaining(),
            })
        })
        .await
    })
}

pub(super) fn test(
    before: Option<Vec<OutputConfig>>,
    outputs: Vec<OutputConfig>,
    seconds: u32,
    cx: &mut App,
) -> Task<Result<u32, String>> {
    let document = json!({"before":before,"outputs":outputs}).to_string();
    let remote = request(vec!["display".into(), "test".into(), document], cx);
    cx.spawn(async move |_| {
        if let Some(value) = remote.await? {
            return value["reverts_in_secs"]
                .as_u64()
                .and_then(|seconds| u32::try_from(seconds).ok())
                .ok_or_else(|| "invalid display countdown reply".into());
        }
        outputs::jobs::run(move || match before {
            Some(before) => outputs::apply_edits_with_revert(&before, &outputs, seconds),
            None => outputs::apply_with_revert(&outputs, seconds),
        })
        .await
    })
}

pub(super) fn keep(cx: &mut App) -> Task<Result<bool, String>> {
    let remote = request(vec!["display".into(), "confirm".into()], cx);
    cx.spawn(async move |cx| {
        if let Some(value) = remote.await? {
            return value["kept"]
                .as_bool()
                .ok_or_else(|| "invalid display confirmation reply".into());
        }
        cx.update(mandara_ui::plugins::displays::keep).await
    })
}

pub(super) fn revert(cx: &mut App) -> Task<Result<bool, String>> {
    let remote = request(vec!["display".into(), "revert".into()], cx);
    cx.spawn(async move |_| {
        if let Some(value) = remote.await? {
            return value["reverted"]
                .as_bool()
                .ok_or_else(|| "invalid display revert reply".into());
        }
        outputs::jobs::run(outputs::revert_now).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_an_unavailable_shell_allows_a_standalone_transaction() {
        assert!(
            available(Err(QueryError::Unavailable("not running".into())))
                .unwrap()
                .is_none()
        );
        assert!(available(Err(QueryError::Failed("timed out after sending".into()))).is_err());
        assert_eq!(
            available(Ok(json!({"kept":true}))).unwrap().unwrap()["kept"],
            true
        );
    }
}
