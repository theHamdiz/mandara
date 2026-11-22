//! The authentication worker: owns the greetd connection (or the demo's
//! fake daemon) on its own thread so PAM latency never blocks rendering.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::greetd::{Conversation, DemoGreetd, GreetdSocket, Notes, Step, Transport};

/// Fake PAM round-trip of the demo greeter (no real authentication).
const DEMO_AUTH_DELAY: std::time::Duration = std::time::Duration::from_millis(420);

pub enum AuthCommand {
    /// Create a session for `user` and answer the first secret prompt.
    Begin { user: String, secret: String },
    /// Answer a follow-up prompt.
    Respond(Option<String>),
    /// Launch the chosen session.
    Start { cmd: Vec<String>, env: Vec<String> },
    Cancel,
}

#[derive(Debug)]
pub enum AuthEvent {
    Step(Step, Notes),
    Started(Result<(), String>),
}

impl Transport for Box<dyn Transport> {
    fn roundtrip(&mut self, req: &crate::greetd::Request) -> std::io::Result<crate::greetd::Response> {
        (**self).roundtrip(req)
    }
}

/// The worker answers one command at a time, so an answer can arrive after
/// the person pressed Escape. Each command is stamped with the current
/// *attempt*, [`AuthWorker::send`] starts a new one on every
/// [`AuthCommand::Cancel`], and an event whose stamp is no longer current
/// belongs to the abandoned attempt ([`AuthWorker::is_current`]).
pub struct AuthWorker {
    tx: Sender<(u64, AuthCommand)>,
    attempt: Arc<AtomicU64>,
}

impl AuthWorker {
    /// Connects (real mode: `$GREETD_SOCK`) and starts the worker; events
    /// arrive on the returned channel.
    pub fn spawn(demo: bool) -> std::io::Result<(AuthWorker, async_channel::Receiver<(u64, AuthEvent)>)> {
        let transport: Box<dyn Transport> = if demo {
            Box::new(DemoGreetd::with_delay(DEMO_AUTH_DELAY))
        } else {
            Box::new(GreetdSocket::from_env()?)
        };
        let (tx, rx) = channel();
        let (etx, erx) = async_channel::unbounded();
        std::thread::Builder::new().name("mandara-greeter-auth".into()).spawn(move || worker(Conversation::new(transport), rx, etx))?;
        Ok((AuthWorker { tx, attempt: Arc::new(AtomicU64::new(0)) }, erx))
    }

    /// Queues `cmd` for the current attempt; a [`AuthCommand::Cancel`]
    /// first ends that attempt.
    pub fn send(&self, cmd: AuthCommand) {
        if matches!(cmd, AuthCommand::Cancel) {
            self.attempt.fetch_add(1, Ordering::SeqCst);
        }
        let _ = self.tx.send((self.attempt.load(Ordering::SeqCst), cmd));
    }

    /// Whether an event stamped `attempt` is for the attempt in progress,
    /// not one the person has since cancelled.
    pub fn is_current(&self, attempt: u64) -> bool {
        self.attempt.load(Ordering::SeqCst) == attempt
    }
}

fn worker(mut conv: Conversation<Box<dyn Transport>>, rx: Receiver<(u64, AuthCommand)>, tx: async_channel::Sender<(u64, AuthEvent)>) {
    while let Ok((attempt, cmd)) = rx.recv() {
        let event = match cmd {
            // The conversation takes the password and wipes every copy it makes.
            AuthCommand::Begin { user, secret } => {
                let (step, notes) = conv.begin(&user, Some(secret));
                AuthEvent::Step(step, notes)
            }
            AuthCommand::Respond(answer) => {
                let (step, notes) = conv.respond(answer);
                AuthEvent::Step(step, notes)
            }
            AuthCommand::Start { cmd, env } => AuthEvent::Started(conv.start(cmd, env)),
            AuthCommand::Cancel => {
                conv.cancel();
                continue;
            }
        };
        if tx.send_blocking((attempt, event)).is_err() {
            break;
        }
    }
}
