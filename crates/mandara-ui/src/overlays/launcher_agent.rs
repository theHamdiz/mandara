//! The bang-prefixed search path shared by Classic, Grid and Beacon.

use super::*;
use mandara_services::command_agent::{self as agent, Event, Job, Request};

#[derive(Default)]
pub(super) struct Agent {
    pub(super) settings: mandara_config::agent::AgentConfig,
    job: Option<Job>,
    task: Option<gpui::Task<()>>,
    words: Option<Vec<Vec<String>>>,
}

/// Examples are queries, not precompiled commands; clicking still shows a preview.
const EXAMPLES: [&str; 3] = [tk("Switch to dark mode"), tk("Set volume to 40"), tk("Open the calendar")];

/// Build an inert launcher result; agent activation owns all dispatch decisions.
fn row(label: impl Into<String>, detail: impl Into<String>, key: &str) -> Item {
    Item::Hit(Hit {
        provider: "agent",
        key: key.into(),
        label: label.into(),
        detail: detail.into(),
        lead: Lead::Icon("zap"),
        badge: tk("AI Shell Agent"),
        keywords: String::new(),
        action: HitAction::Command(vec![]),
    })
}

/// Turn a command removed since compilation into an actionable error row.
fn resolution_error(words: &[Vec<String>], registry: &mandara_plugins::CommandRegistry) -> Option<Item> {
    words.iter().find_map(|w| registry.resolve(w).err()).map(|error| row(t("The local agent could not compile this command"), error, "setup"))
}

impl LauncherView {
    /// Returns before standard search, so agent queries cannot become web searches.
    pub(super) fn agent_refilter(&mut self, cx: &mut Context<Self>) -> bool {
        if agent::query(&self.query).is_none() {
            self.agent = None;
            return false;
        }
        self.results.clear();
        self.sections.clear();
        self.selected = 0;
        let query = agent::query(&self.query).unwrap_or_default().to_owned();
        let cfg = config(cx).read(cx).get().launcher.agent.clone();
        self.agent = Some(Agent { settings: cfg.clone(), ..Agent::default() });
        if !cfg.enabled {
            self.results.push(row(t("Enable local shell commands in Studio"), t("AI Shell Agent"), "setup"));
            return true;
        }
        if query.is_empty() {
            self.results.push(row(t("Type a natural language shell command…"), t("Everything runs locally on your CPU"), "hint"));
            for (i, example) in EXAMPLES.into_iter().enumerate() {
                self.results.push(row(tr(example), format!("!{example}"), &i.to_string()));
            }
            let engine = match cfg.engine {
                mandara_config::agent::AgentEngine::FunctionGemma => agent::Engine::FunctionGemma,
                mandara_config::agent::AgentEngine::Qwen => agent::Engine::Qwen,
            };
            let task = cx.spawn(async move |this, cx| {
                let installed =
                    cx.background_executor().spawn(async move { agent::models::ROUTER.installed() && agent::models::compiler(engine).installed() }).await;
                if !installed {
                    let _ = this.update(cx, |this, cx| {
                        if this.closing {
                            return;
                        }
                        let size = agent::models::ROUTER.download_size() + agent::models::compiler(engine).download_size();
                        this.results.insert(
                            0,
                            row(
                                t("Local AI Agent models are not installed"),
                                format!("{} · {}", t("Open Studio to download the required models"), mandara_services::self_update::human_size(size)),
                                "setup",
                            ),
                        );
                        cx.notify();
                    });
                }
            });
            if let Some(state) = self.agent.as_mut() {
                state.task = Some(task);
            }
            return true;
        }
        if query.len() > agent::MAX_QUERY_BYTES {
            self.results.push(row(t("This command is too long"), "", "hint"));
            return true;
        }
        self.results.push(row(t("Preparing local commands…"), t("Esc cancels and releases the agent"), "hint"));
        let catalog = cx.global::<mandara_plugins::PluginRegistry>().commands.catalog_json();
        let request = match serde_json::from_value(catalog) {
            Ok(catalog) => Request {
                query,
                catalog,
                prepare_index: false,
                engine: match cfg.engine {
                    mandara_config::agent::AgentEngine::FunctionGemma => agent::Engine::FunctionGemma,
                    mandara_config::agent::AgentEngine::Qwen => agent::Engine::Qwen,
                },
            },
            Err(error) => {
                self.results = vec![row(error.to_string(), "", "hint")];
                return true;
            }
        };
        let job = Job::default();
        let cancel = job.cancellation();
        let delay =
            std::time::Duration::from_millis(cfg.debounce_ms.clamp(mandara_config::agent::DEBOUNCE_RANGE_MS.0, mandara_config::agent::DEBOUNCE_RANGE_MS.1));
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let (tx, rx) = async_channel::bounded(2);
            let work = cx.background_executor().spawn(async move {
                if !agent::models::ROUTER.installed() || !agent::models::compiler(request.engine).installed() {
                    let _ = tx.try_send(Event::Error { message: "models-missing".into() });
                    return;
                }
                if let Err(error) = agent::run(request, cancel, tx.clone()) {
                    let _ = tx.try_send(Event::Error { message: error.to_string() });
                }
            });
            while let Ok(event) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if this.closing {
                            return;
                        }
                        match event {
                            Event::Matches { matches, .. } => {
                                this.results = matches.into_iter().map(|m| row(tr(&m.command.summary), m.command.path, "hint")).collect();
                            }
                            Event::Ready { words, inference_ms, .. } => {
                                this.results =
                                    vec![row(t("Run these shell actions"), tf("Compiled in {time} ms", &[("time", &format!("{inference_ms:.0}"))]), "run")];
                                for action in &words {
                                    this.results.push(row(action.join(" "), agent::catalog::cli(action), "hint"));
                                }
                                if let Some(state) = this.agent.as_mut() {
                                    state.words = Some(words);
                                }
                            }
                            Event::Error { message } if message == "models-missing" => {
                                this.results =
                                    vec![row(t("Local AI Agent models are not installed"), t("Open Studio to download the required models"), "setup")];
                            }
                            Event::Error { message } => {
                                this.results = vec![row(t("The local agent could not compile this command"), message, "setup")];
                            }
                        }
                        this.selected = 0;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
            work.await;
        });
        if let Some(state) = self.agent.as_mut() {
            state.job = Some(job);
            state.task = Some(task);
        }
        true
    }

    /// Open setup, select an example or validate and execute the compiled preview.
    pub(super) fn activate_agent(&mut self, index: usize, cx: &mut Context<Self>) {
        let key = match self.results.get(index) {
            Some(Item::Hit(hit)) => hit.key.as_str(),
            _ => return,
        };
        if key == "setup" {
            self.close(cx);
            execute(Effect::Commands(vec![vec!["studio".into(), "ai-shell-agent".into()]]), cx);
        } else if let Ok(i) = key.parse::<usize>() {
            if let Some(example) = EXAMPLES.get(i) {
                self.set_query(format!("!{example}"), cx);
            }
        } else if matches!(key, "run" | "hint")
            && let Some(words) = self.agent.as_mut().and_then(|state| state.words.take())
        {
            // All steps are validated before the first one runs. The ordinary
            // dispatcher still checks live plugin and compositor availability.
            let registry = &cx.global::<mandara_plugins::PluginRegistry>().commands;
            if let Some(error) = resolution_error(&words, registry) {
                self.results = vec![error];
                self.selected = 0;
            } else {
                self.close(cx);
                execute(Effect::Commands(words), cx);
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    /// Replace the live catalog after compilation and require an error action for the old preview.
    fn a_removed_command_produces_feedback_instead_of_a_dead_run_button() {
        let mut registry = mandara_plugins::CommandRegistry::default();
        registry.register("core", mandara_plugins::CommandSpec::new("fixture", tk("AI Shell Agent"), |_, _| Ok(serde_json::Value::Null)));
        let words = vec![vec!["fixture".into()]];
        assert!(resolution_error(&words, &registry).is_none());
        let missing = resolution_error(&words, &mandara_plugins::CommandRegistry::default()).unwrap();
        assert!(matches!(missing,Item::Hit(Hit {key,detail,..}) if key=="setup" && !detail.is_empty()));
    }

    #[test]
    fn every_hint_is_an_inert_provider_result() {
        let item = row("hint", "detail", "hint");
        assert!(matches!(item, Item::Hit(Hit { action:HitAction::Command(ref words), .. }) if words.is_empty()));
        assert_eq!(EXAMPLES.len(), 3);
    }
}
