//! The authentication worker: owns the greetd connection (or the demo's
//! fake daemon) on its own thread so PAM latency never blocks rendering.

use std::sync::mpsc::{Receiver, Sender, channel};

use crate::greetd::{Conversation, DemoGreetd, GreetdSocket, Notes, Step, Transport, wipe};

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

pub struct AuthWorker {
    tx: Sender<AuthCommand>,
}

impl AuthWorker {
    /// Connects (real mode: `$GREETD_SOCK`) and starts the worker; events
    /// arrive on the returned channel.
    pub fn spawn(demo: bool) -> std::io::Result<(AuthWorker, async_channel::Receiver<AuthEvent>)> {
        let transport: Box<dyn Transport> = if demo {
            Box::new(DemoGreetd::with_delay(DEMO_AUTH_DELAY))
        } else {
            Box::new(GreetdSocket::from_env()?)
        };
        let (tx, rx) = channel();
        let (etx, erx) = async_channel::unbounded();
        std::thread::Builder::new().name("mandara-greeter-auth".into()).spawn(move || worker(Conversation::new(transport), rx, etx))?;
        Ok((AuthWorker { tx }, erx))
    }

    pub fn send(&self, cmd: AuthCommand) {
        let _ = self.tx.send(cmd);
    }
}

fn worker(mut conv: Conversation<Box<dyn Transport>>, rx: Receiver<AuthCommand>, tx: async_channel::Sender<AuthEvent>) {
    while let Ok(cmd) = rx.recv() {
        let event = match cmd {
            AuthCommand::Begin { user, mut secret } => {
                let (step, notes) = conv.begin(&user, Some(secret.clone()));
                wipe(&mut secret);
                AuthEvent::Step(step, notes)
            }
            AuthCommand::Respond(mut answer) => {
                let (step, notes) = conv.respond(answer.clone());
                if let Some(a) = answer.as_mut() {
                    wipe(a);
                }
                AuthEvent::Step(step, notes)
            }
            AuthCommand::Start { cmd, env } => AuthEvent::Started(conv.start(cmd, env)),
            AuthCommand::Cancel => {
                conv.cancel();
                continue;
            }
        };
        if tx.send_blocking(event).is_err() {
            break;
        }
    }
}
