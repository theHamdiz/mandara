//! Native app actions that do not need an imported setup command.

mod kde;

use super::{paths::Locations, preferences};
use anyhow::{Result, ensure};
use std::path::Path;

pub struct Context<'a> {
    pub owner: &'a str,
    pub output: &'a Path,
    pub locations: &'a Locations,
}

pub trait Action {
    fn apply(&self, context: &Context<'_>) -> Result<()>;
}

pub fn apply(name: &str, context: &Context<'_>) -> Result<()> {
    match name {
        "" => Ok(()),
        "kde-color-scheme" => kde::Kde.apply(context),
        other => anyhow::bail!("unknown app theme action: {other}"),
    }
}

pub fn undo(owner: &str, locations: &Locations) -> Result<Vec<std::path::PathBuf>> {
    preferences::undo(owner, locations)
}

fn single_output(outputs: &[std::path::PathBuf]) -> Result<&Path> {
    ensure!(outputs.len() == 1, "app theme action needs one output file");
    Ok(&outputs[0])
}

pub fn run(
    name: &str,
    owner: &str,
    outputs: &[std::path::PathBuf],
    locations: &Locations,
) -> Result<()> {
    if name.is_empty() {
        return Ok(());
    }
    apply(
        name,
        &Context {
            owner,
            output: single_output(outputs)?,
            locations,
        },
    )
}
