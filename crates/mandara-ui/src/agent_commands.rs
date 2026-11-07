//! The same dry-run and model setup paths used by Studio, over documented IPC.
use mandara_i18n::tk;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::command_agent::{self as agent, Event, Job, Request};
use serde_json::json;

pub(crate) fn register(registry: &mut PluginRegistry) {
    registry.command(
        "core",
        CommandSpec::new("agent purge", tk("Purge agent from RAM now"), |_, _| {
            agent::purge();
            Ok(json!({"cancellation_requested":true}))
        }),
    );
    registry.command("core",CommandSpec::new_output("agent status",tk("Agent worker activity and installed model files"),|_,_| {
        let (tx,rx)=async_channel::bounded(1);
        mandara_services::runtime::spawn("agent-status",async move {
            let status=mandara_services::runtime::unblock(|| json!({"active_requests":agent::active_requests(),"models":agent::models::ALL.map(|m| json!({"id":m.id,"installed":m.installed(),"bytes":m.download_size()}))})).await;
            let _=tx.send(Ok(status.to_string())).await;
        });
        Ok(rx.into())
    }));
    for (path, prepare) in [("agent preview", false), ("agent prepare", true)] {
        let mut spec = CommandSpec::new_output(
            path,
            if prepare { tk("Build the semantic index for the current command catalog") } else { tk("Compile a local shell command without executing it") },
            move |args, cx| {
                let cfg = crate::state::config(cx).read(cx).get().launcher.agent.clone();
                let engine = match cfg.engine {
                    mandara_config::agent::AgentEngine::FunctionGemma => agent::Engine::FunctionGemma,
                    mandara_config::agent::AgentEngine::Qwen => agent::Engine::Qwen,
                };
                let query = args.rest(0);
                let query = agent::query(&query).unwrap_or(query.trim()).to_owned();
                let catalog = serde_json::from_value(cx.global::<PluginRegistry>().commands.catalog_json()).map_err(|e| e.to_string())?;
                let (tx, rx) = async_channel::bounded(1);
                mandara_services::runtime::spawn("agent-preview", async move {
                    let result = mandara_services::runtime::unblock(move || {
                        let job = Job::default();
                        let (events, replies) = async_channel::bounded(2);
                        agent::run(Request { query, catalog, engine, prepare_index: prepare }, job.cancellation(), events).map_err(|e| e.to_string())?;
                        let mut received = Vec::new();
                        while let Ok(event) = replies.try_recv() {
                            if let Event::Error { message } = event {
                                return Err(message);
                            }
                            received.push(event);
                        }
                        serde_json::to_string(&received).map_err(|e| e.to_string())
                    })
                    .await;
                    let _ = tx.send(result).await;
                });
                Ok(rx.into())
            },
        );
        if !prepare {
            spec = spec.arg(ArgSpec::required("query", ArgKind::String, tk("Try a shell command…")));
        }
        registry.command("core", spec);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_is_documented_and_requires_a_query() {
        let mut registry = PluginRegistry::default();
        register(&mut registry);
        let words = vec!["agent".into(), "preview".into()];
        assert!(registry.commands.resolve(&words).is_err());
        assert!(registry.commands.resolve(&[words, vec!["dark mode".into()]].concat()).is_ok());
        assert!(registry.commands.resolve(&["agent".into(), "prepare".into()]).is_ok());
    }
}
