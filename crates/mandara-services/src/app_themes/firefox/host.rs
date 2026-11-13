//! A browser-owned host sleeps on stdin and file events, and exits on disconnect.

use super::{Settings, css, wire};
use crate::app_themes::paths::Locations;
use anyhow::Result;
use rustix::event::{PollFd, PollFlags, poll};
use rustix::fs::{
    OFlags, fcntl_getfl, fcntl_setfl,
    inotify::{self, CreateFlags, ReadFlags},
};
use serde_json::{Value, json};
use std::io::Read as _;

const COMPATIBLE_PROTOCOL_VERSION: &str = "2.9.0";

fn mode(settings: &Settings) -> Value {
    json!({"action":"theme:mode","success":true,"data":settings.mode})
}

#[derive(Default)]
struct Protocol {
    mode_sent: bool,
}

fn palette(settings: Option<&Settings>) -> Value {
    settings
        .ok_or_else(|| anyhow::anyhow!("no browser app theme is enabled"))
        .and_then(super::colors)
        .unwrap_or_else(
            |error| json!({"action":"action:colors","success":false,"error":format!("{error:#}")}),
        )
}

fn handle(
    protocol: &mut Protocol,
    message: &Value,
    settings: Option<&Settings>,
    locations: &Locations,
    mut send: impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    let action = message["action"].as_str().unwrap_or("action:invalid");
    match action {
        "debug:version" => {
            send(json!({"action":action,"success":true,"data":COMPATIBLE_PROTOCOL_VERSION}))
        }
        "action:colors" => {
            let reply = palette(settings);
            let complete = reply["success"] == true;
            send(reply)?;
            if complete
                && !protocol.mode_sent
                && let Some(settings) = settings
            {
                protocol.mode_sent = true;
                send(mode(settings))?;
            }
            Ok(())
        }
        "css:enable" | "css:disable" | "css:font:size" => {
            let Some(target) = message["target"]
                .as_str()
                .filter(|target| !target.is_empty())
            else {
                return send(json!({"action":"action:invalid","success":false}));
            };
            let size = message["size"]
                .as_i64()
                .or_else(|| message["size"].as_str()?.parse().ok());
            if action == "css:font:size" && size.is_none() {
                return send(json!({"action":"action:invalid","success":false}));
            }
            let data = if action == "css:font:size" {
                json!(size)
            } else {
                json!(target)
            };
            let result = (|| {
                let settings =
                    settings.ok_or_else(|| anyhow::anyhow!("no browser app theme is enabled"))?;
                let lock = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(locations.config.join("mandara/generated/app-themes.lock"))?;
                lock.lock()?;
                css::apply(action, target, size, &settings.styles, locations)
            })();
            send(match result {
                Ok(()) => {
                    json!({"action":action,"success":true,"data":data,"message":mandara_i18n::t("Done")})
                }
                Err(error) => {
                    json!({"action":action,"success":false,"data":data,"error":format!("{error:#}")})
                }
            })
        }
        _ => send(json!({"action":"action:invalid","success":false})),
    }
}

pub(super) fn run(locations: &Locations) -> Result<()> {
    let path = super::settings_path(locations);
    let directory = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("browser settings have no parent directory"))?;
    std::fs::create_dir_all(directory)?;
    let events = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
    let mut settings_watch = super::watch::Parent::default();
    let mut colors_watch = super::watch::Parent::default();
    let mut current = super::settings(locations).ok();
    let stdin = std::io::stdin();
    fcntl_setfl(&stdin, fcntl_getfl(&stdin)? | OFlags::NONBLOCK)?;
    let mut input = stdin.lock();
    let mut output = std::io::stdout().lock();
    let mut decoder = wire::Decoder::default();
    let mut protocol = Protocol::default();
    let mut bytes = [0; 4096];
    let mut event_buffer = [std::mem::MaybeUninit::<u8>::uninit(); 4096];
    loop {
        settings_watch.sync(&events, Some(directory))?;
        colors_watch.sync(
            &events,
            current
                .as_ref()
                .and_then(|settings| settings.colors.parent())
                .filter(|parent| *parent != directory),
        )?;
        let mut descriptors = [
            PollFd::new(&stdin, PollFlags::IN),
            PollFd::new(&events, PollFlags::IN),
        ];
        match poll(&mut descriptors, None) {
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        }
        if descriptors[0]
            .revents()
            .intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR)
        {
            loop {
                match input.read(&mut bytes) {
                    Ok(0) => return Ok(()),
                    Ok(count) => decoder.push(&bytes[..count], |message| {
                        handle(
                            &mut protocol,
                            &message,
                            current.as_ref(),
                            locations,
                            |value| wire::write(&mut output, &value),
                        )
                    })?,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error.into()),
                }
            }
        }
        if descriptors[1].revents().contains(PollFlags::IN) {
            let mut relevant = false;
            let mut reader = inotify::Reader::new(&events, &mut event_buffer);
            while let Ok(event) = reader.next() {
                settings_watch.observe(&events, event.wd(), event.events())?;
                colors_watch.observe(&events, event.wd(), event.events())?;
                relevant |= event.events().intersects(
                    ReadFlags::DELETE_SELF
                        | ReadFlags::MOVE_SELF
                        | ReadFlags::IGNORED
                        | ReadFlags::QUEUE_OVERFLOW,
                ) || settings_watch.recovering(directory)
                    || current
                        .as_ref()
                        .and_then(|settings| settings.colors.parent())
                        .is_some_and(|path| colors_watch.recovering(path));
                relevant |= event.file_name().is_some_and(|name| {
                    Some(name.to_bytes())
                        == path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .map(str::as_bytes)
                        || current.as_ref().is_some_and(|settings| {
                            Some(name.to_bytes())
                                == settings
                                    .colors
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .map(str::as_bytes)
                        })
                });
            }
            if relevant {
                current = super::settings(locations).ok();
                if let Some(settings) = &current {
                    wire::write(&mut output, &palette(Some(settings)))?;
                    wire::write(&mut output, &mode(settings))?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_size_replies_return_the_applied_number_for_extension_controls() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations {
            home: root.path().into(),
            config: root.path().join("config"),
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            state: root.path().join("state"),
        };
        std::fs::create_dir_all(locations.config.join("mandara/generated")).unwrap();
        let profiles = root.path().join(".mozilla/firefox");
        let profile = profiles.join("default/chrome");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            profiles.join("profiles.ini"),
            "[Profile0]\nPath=default\nIsRelative=1\n",
        )
        .unwrap();
        std::fs::write(
            profile.join("userChrome.css"),
            ":root {\n --pywalfox-font-size: 14px;\n}\n",
        )
        .unwrap();
        let settings = Settings {
            colors: root.path().join("unused"),
            mode: "dark".into(),
            styles: root.path().into(),
            owner: "browser".into(),
            _lease: None,
        };
        let mut replies = Vec::new();
        handle(
            &mut Protocol::default(),
            &json!({"action":"css:font:size","target":"userChrome","size":"22"}),
            Some(&settings),
            &locations,
            |reply| {
                replies.push(reply);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(replies[0]["success"], true);
        assert_eq!(replies[0]["data"], 22);
        assert!(replies[0]["message"].is_string());
        assert!(
            std::fs::read_to_string(profile.join("userChrome.css"))
                .unwrap()
                .contains("--pywalfox-font-size: 22px;")
        );
    }

    #[test]
    fn palette_requests_send_initial_mode_once_and_bad_updates_have_an_error_reply() {
        let root = tempfile::tempdir().unwrap();
        let colors = root.path().join("colors.json");
        std::fs::write(
            &colors,
            mandara_theme::templates::pywal_json(&mandara_theme::engine::default_palette(true), ""),
        )
        .unwrap();
        let settings = Settings {
            colors: colors.clone(),
            mode: "dark".into(),
            styles: root.path().into(),
            owner: "browser".into(),
            _lease: None,
        };
        let locations = Locations {
            home: root.path().into(),
            config: root.path().join("config"),
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            state: root.path().join("state"),
        };
        let mut protocol = Protocol::default();
        let mut replies = Vec::new();
        for _ in 0..2 {
            handle(
                &mut protocol,
                &json!({"action":"action:colors"}),
                Some(&settings),
                &locations,
                |reply| {
                    replies.push(reply);
                    Ok(())
                },
            )
            .unwrap();
        }
        assert_eq!(
            replies
                .iter()
                .map(|reply| reply["action"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["action:colors", "theme:mode", "action:colors"]
        );
        std::fs::write(colors, "{}").unwrap();
        let reply = palette(Some(&settings));
        assert_eq!(reply["success"], false);
        assert!(reply["error"].is_string());
    }

    #[test]
    fn protocol_errors_are_explicit_and_do_not_write_browser_preferences() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations {
            home: root.path().into(),
            config: root.path().join("config"),
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            state: root.path().join("state"),
        };
        let mut replies = Vec::new();
        handle(
            &mut Protocol::default(),
            &json!({"action":"action:colors"}),
            None,
            &locations,
            |message| {
                replies.push(message);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(replies[0]["success"], false);
        handle(
            &mut Protocol::default(),
            &json!({"action":"debug:version"}),
            None,
            &locations,
            |message| {
                replies.push(message);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(replies[1]["data"], COMPATIBLE_PROTOCOL_VERSION);
        assert!(!locations.config.exists());
    }
}
