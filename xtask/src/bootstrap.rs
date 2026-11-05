//! Keep the small download command's language and public keys in sync.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use mandara_i18n::{Lang, tk};

use crate::{Result, flag, root};

const BEGIN: &str = "# Generated messages: cargo xtask bootstrap\n";
const END: &str = "# End generated messages\n";

pub(super) const MESSAGES: &[(&str, &str)] = &[
    ("welcome", tk("Your Wayland desktop. Let’s get it ready.")),
    ("signature", tk("Checking the release signature…")),
    ("download", tk("Downloading Mandara…")),
    ("verify", tk("Verifying the download…")),
    ("unpack", tk("Unpacking the installer…")),
    ("start", tk("Ready. Opening Mandara’s installer…")),
    ("missing_tool", tk("Install this tool and try again:")),
    ("unsupported", tk("Prebuilt releases require Linux on an x86_64 or aarch64 CPU.")),
    ("https", tk("The release mirror must use HTTPS.")),
    ("temporary", tk("Could not create a temporary download folder.")),
    ("failed_download", tk("Download failed. Check your connection and try again.")),
    ("bad_signature", tk("The release signature could not be verified. Nothing was installed.")),
    ("openssl", tk("This command needs OpenSSL 3 or newer.")),
    ("missing_checksum", tk("The signed release has no unique checksum for this archive.")),
    ("bad_checksum", tk("The download did not match the signed release. Nothing was installed; please try again.")),
    ("failed_unpack", tk("Could not unpack the verified release.")),
    ("missing_installer", tk("The release does not contain exactly one executable installer.")),
    ("manual", tk("Manual downloads:")),
    ("nixos", tk("On NixOS, install Mandara through your flake.")),
];

fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn messages() -> Result<String> {
    let mut out = format!("{BEGIN}message() {{\n    case \"$language:$1\" in\n");
    for lang in Lang::ALL.into_iter().filter(|lang| *lang != Lang::En) {
        let path = root().join("crates/mandara-i18n/locales").join(format!("{}.toml", lang.code()));
        let catalog: BTreeMap<String, String> = toml::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        for &(id, english) in MESSAGES {
            let value = catalog.get(english).ok_or_else(|| format!("{}: missing {english:?}", lang.code()))?;
            writeln!(out, "        {}:{id}) printf '%s\\n' {} ;;", lang.code(), quoted(value)).unwrap();
        }
    }
    for &(id, english) in MESSAGES {
        writeln!(out, "        *:{id}) printf '%s\\n' {} ;;", quoted(english)).unwrap();
    }
    write!(out, "    esac\n}}\n{END}").unwrap();
    Ok(out)
}

pub(super) fn refreshed(script: &str) -> Result<String> {
    let begin = script.find(BEGIN).ok_or("bootstrap message start marker is missing")?;
    let end = script[begin..].find(END).map(|i| begin + i + END.len()).ok_or("bootstrap message end marker is missing")?;
    let script = format!("{}{}{}", &script[..begin], messages()?, &script[end..]);
    let keys = mandara_delta::signing::trusted_keys().iter().map(|key| mandara_delta::hex(key)).collect::<Vec<_>>().join(" ");
    if keys.is_empty() { return Err("no public release keys".into()); }
    let mut out = String::new();
    for line in script.lines() {
        if line.starts_with("release_keys=") { writeln!(out, "release_keys={}", quoted(&keys)).unwrap(); }
        else { writeln!(out, "{line}").unwrap(); }
    }
    Ok(out)
}

pub fn generate(args: &[String]) -> Result<()> {
    let path = root().join("install.sh");
    let script = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let updated = refreshed(&script)?;
    if flag(args, "--check") {
        if script != updated { return Err("install.sh is stale; run cargo xtask bootstrap".into()); }
    } else { std::fs::write(path, updated).map_err(|e| e.to_string())?; }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translations_and_keys_match_the_installer_sources() {
        let script = std::fs::read_to_string(root().join("install.sh")).unwrap();
        assert_eq!(refreshed(&script).unwrap(), script, "run cargo xtask bootstrap");
    }

    #[test]
    fn translated_punctuation_stays_shell_data() {
        let output = std::process::Command::new("sh").args(["-c", &format!("printf '%s' {}", quoted("l'utilisateur $(exit 7) `exit 8`"))]).output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"l'utilisateur $(exit 7) `exit 8`");
    }
}
