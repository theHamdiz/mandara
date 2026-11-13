//! Preference edits shared by Studio, IPC and concurrent file reloads.

use crate::ShellConfig;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Fields are patched individually. Named list entries are addressed by their
/// stable identity, so an old slider edit cannot recreate a removed widget.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConfigPatch {
    edits: Vec<Edit>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum Part {
    Key(String),
    Item { field: String, id: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Order {
    field: String,
    ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Edit {
    path: Vec<Part>,
    value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    order: Option<Order>,
}

fn identities<'a>(items: &'a [Value], field: &str) -> Option<Vec<&'a str>> {
    let ids = items
        .iter()
        .map(|item| item.get(field)?.as_str().filter(|id| !id.is_empty()))
        .collect::<Option<Vec<_>>>()?;
    let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
    (unique.len() == ids.len()).then_some(ids)
}

impl ConfigPatch {
    pub fn between(before: &ShellConfig, after: &ShellConfig) -> Result<Self> {
        let mut patch = Self::default();
        patch.diff(
            &serde_json::to_value(before)?,
            &serde_json::to_value(after)?,
            &mut Vec::new(),
        );
        Ok(patch)
    }

    fn edit(&mut self, path: &[Part], value: Option<Value>) {
        self.edits.push(Edit {
            path: path.into(),
            value,
            order: None,
        });
    }

    fn diff(&mut self, before: &Value, after: &Value, path: &mut Vec<Part>) {
        if before == after {
            return;
        }
        if let (Value::Array(a), Value::Array(b)) = (before, after) {
            for field in ["id", "name"] {
                if a.is_empty() && b.is_empty() {
                    break;
                }
                if let (Some(old), Some(new)) = (identities(a, field), identities(b, field)) {
                    for (id, item) in new.iter().zip(b) {
                        path.push(Part::Item {
                            field: field.into(),
                            id: (*id).into(),
                        });
                        if let Some(index) = old.iter().position(|previous| previous == id) {
                            self.diff(&a[index], item, path);
                        } else {
                            self.edit(path, Some(item.clone()));
                        }
                        path.pop();
                    }
                    for id in old.iter().filter(|id| !new.contains(id)) {
                        path.push(Part::Item {
                            field: field.into(),
                            id: (*id).into(),
                        });
                        self.edit(path, None);
                        path.pop();
                    }
                    if old != new && new.len() > 1 {
                        self.edits.push(Edit {
                            path: path.clone(),
                            value: None,
                            order: Some(Order {
                                field: field.into(),
                                ids: new.into_iter().map(str::to_string).collect(),
                            }),
                        });
                    }
                    return;
                }
            }
        }
        if let (Value::Object(a), Value::Object(b)) = (before, after) {
            // Changing an enum variant replaces that variant's fields together.
            if a.get("type") != b.get("type") {
                self.edit(path, Some(after.clone()));
                return;
            }
            for (key, value) in b {
                path.push(Part::Key(key.clone()));
                match a.get(key) {
                    Some(old) => self.diff(old, value, path),
                    None => self.edit(path, Some(value.clone())),
                }
                path.pop();
            }
            for key in a.keys().filter(|key| !b.contains_key(*key)) {
                path.push(Part::Key(key.clone()));
                self.edit(path, None);
                path.pop();
            }
        } else {
            self.edit(path, Some(after.clone()));
        }
    }

    /// Apply atomically to the latest config, preserving unrelated edits.
    pub fn apply(&self, config: &ShellConfig) -> Result<ShellConfig> {
        let mut root = serde_json::to_value(config)?;
        for edit in &self.edits {
            if let Some(order) = &edit.order {
                if let Some(array) = at(&mut root, &edit.path)?.and_then(Value::as_array_mut) {
                    array.sort_by_key(|item| {
                        item.get(&order.field)
                            .and_then(Value::as_str)
                            .and_then(|id| order.ids.iter().position(|expected| expected == id))
                            .unwrap_or(order.ids.len())
                    });
                }
                continue;
            }
            let Some((part, parents)) = edit.path.split_last() else {
                anyhow::bail!("a preference edit must name a field");
            };
            let Some(parent) = at(&mut root, parents)? else {
                continue;
            };
            match part {
                Part::Key(key) => {
                    let object = parent
                        .as_object_mut()
                        .ok_or_else(|| anyhow::anyhow!("preference parent is not an object"))?;
                    if let Some(value) = &edit.value {
                        object.insert(key.clone(), value.clone());
                    } else {
                        object.remove(key);
                    }
                }
                Part::Item { field, id } => {
                    let array = parent
                        .as_array_mut()
                        .ok_or_else(|| anyhow::anyhow!("preference parent is not a list"))?;
                    let index = array
                        .iter()
                        .position(|item| item.get(field).and_then(Value::as_str) == Some(id));
                    match (&edit.value, index) {
                        (Some(value), Some(index)) => {
                            ensure!(
                                value.get(field).and_then(Value::as_str) == Some(id),
                                "preference item identity changed"
                            );
                            array[index] = value.clone();
                        }
                        (Some(value), None) => {
                            ensure!(
                                value.get(field).and_then(Value::as_str) == Some(id),
                                "preference item identity changed"
                            );
                            array.push(value.clone());
                        }
                        (None, Some(index)) => {
                            array.remove(index);
                        }
                        (None, None) => {}
                    }
                }
            }
        }
        let config: ShellConfig = serde_json::from_value(root)?;
        config.integrations.app_themes.validate()?;
        Ok(config)
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Retain only the latest edit of each field, item or ordering.
    pub fn extend(&mut self, newer: Self) {
        for edit in newer.edits {
            self.edits.retain(|old| {
                if edit.order.is_some() {
                    old.path != edit.path || old.order.is_none()
                } else {
                    !old.path.starts_with(&edit.path)
                }
            });
            self.edits.push(edit);
        }
    }
}

fn at<'a>(mut value: &'a mut Value, path: &[Part]) -> Result<Option<&'a mut Value>> {
    for part in path {
        match part {
            Part::Key(key) => {
                value = value
                    .as_object_mut()
                    .ok_or_else(|| anyhow::anyhow!("preference parent is not an object"))?
                    .entry(key)
                    .or_insert_with(|| Value::Object(Default::default()));
            }
            Part::Item { field, id } => {
                let array = value
                    .as_array_mut()
                    .ok_or_else(|| anyhow::anyhow!("preference parent is not a list"))?;
                let Some(item) = array
                    .iter_mut()
                    .find(|item| item.get(field).and_then(Value::as_str) == Some(id))
                else {
                    return Ok(None);
                };
                value = item;
            }
        }
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_widgets::{DesktopWidget, MonitorWidget, WidgetKind};

    #[test]
    fn stale_widget_edits_do_not_resurrect_removed_widgets_or_edit_a_different_index() {
        let mut base = ShellConfig::default();
        base.desktop.widgets.items = ["clock-one", "clock-two"]
            .map(|id| DesktopWidget {
                id: id.into(),
                ..Default::default()
            })
            .into();
        let mut studio = base.clone();
        studio.desktop.widgets.items[0].place.scale = 1.5;
        let edit = ConfigPatch::between(&base, &studio).unwrap();
        let mut shell = base.clone();
        shell.desktop.widgets.items.remove(1);
        let merged = edit.apply(&shell).unwrap();
        assert_eq!(merged.desktop.widgets.items.len(), 1);
        assert_eq!(merged.desktop.widgets.items[0].place.scale, 1.5);
        shell.desktop.widgets.items.remove(0);
        assert!(edit.apply(&shell).unwrap().desktop.widgets.items.is_empty());
        shell = base.clone();
        shell.desktop.widgets.items.reverse();
        let merged = edit.apply(&shell).unwrap();
        assert_eq!(merged.desktop.widgets.items[1].id, "clock-one");
        assert_eq!(merged.desktop.widgets.items[1].place.scale, 1.5);
    }

    #[test]
    fn reorders_keep_concurrent_new_widgets_and_never_recreate_removed_ones() {
        let mut base = ShellConfig::default();
        base.desktop.widgets.items = ["one", "two", "three"]
            .map(|id| DesktopWidget {
                id: id.into(),
                ..Default::default()
            })
            .into();
        let mut studio = base.clone();
        studio.desktop.widgets.items.reverse();
        let patch = ConfigPatch::between(&base, &studio).unwrap();
        let mut shell = base.clone();
        shell.desktop.widgets.items.remove(1);
        shell.desktop.widgets.items.push(DesktopWidget {
            id: "new".into(),
            ..Default::default()
        });
        let merged = patch.apply(&shell).unwrap();
        let ids: Vec<_> = merged
            .desktop
            .widgets
            .items
            .iter()
            .map(|widget| widget.id.as_str())
            .collect();
        assert_eq!(ids, ["three", "one", "new"]);
    }

    #[test]
    fn widget_edits_do_not_restore_stale_display_preferences() {
        let mut base = ShellConfig::default();
        base.desktop.widgets.items.push(DesktopWidget {
            id: "monitor-1".into(),
            kind: WidgetKind::Monitor(MonitorWidget::default()),
            ..Default::default()
        });
        let mut studio = base.clone();
        studio.desktop.widgets.items.clear();
        let mut shell = base.clone();
        shell.compositor.outputs.push(crate::outputs::OutputConfig {
            name: "DP-1".into(),
            scale: Some(1.25),
            ..Default::default()
        });
        let merged = ConfigPatch::between(&base, &studio)
            .unwrap()
            .apply(&shell)
            .unwrap();
        assert!(merged.desktop.widgets.items.is_empty());
        assert_eq!(merged.compositor.outputs, shell.compositor.outputs);
    }

    #[test]
    fn coalescing_keeps_edits_to_different_fields_and_the_latest_list() {
        let a = ShellConfig::default();
        let mut b = a.clone();
        b.bar.height = 39.0;
        b.desktop.widgets.items.push(DesktopWidget::default());
        let mut c = b.clone();
        c.desktop.widgets.items.clear();
        c.theme.mode = crate::ThemeMode::Light;
        let mut patch = ConfigPatch::between(&a, &b).unwrap();
        patch.extend(ConfigPatch::between(&b, &c).unwrap());
        assert_eq!(patch.apply(&a).unwrap(), c);
    }
}
