use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    #[default]
    Ini,
    Toml,
    Xresources,
}

pub trait Document {
    fn get(&self, section: &str, key: &str) -> Result<Option<String>>;
    fn set(&mut self, section: &str, key: &str, value: Option<&str>) -> Result<()>;
    fn finish(self) -> String;
}

pub enum Content {
    Ini(String),
    Toml(toml_edit::DocumentMut),
    Xresources(String),
}

impl Format {
    pub fn parse(self, body: &str) -> Result<Content> {
        Ok(match self {
            Self::Ini => Content::Ini(body.into()),
            Self::Toml => Content::Toml(body.parse()?),
            Self::Xresources => Content::Xresources(body.into()),
        })
    }
}

impl Document for Content {
    fn get(&self, section: &str, key: &str) -> Result<Option<String>> {
        match self {
            Self::Ini(body) => Ok(crate::icons::apply::ini_get(body, section, key)),
            Self::Toml(body) => {
                let table:Option<&dyn toml_edit::TableLike> = if section.is_empty() { Some(body.as_table()) } else { body.get(section).and_then(toml_edit::Item::as_table_like) };
                let value = table.and_then(|table|table.get(key));
                value.map(|value|value.as_str().map(str::to_owned).ok_or_else(||anyhow::anyhow!("theme preference {section}.{key} is not a string"))).transpose()
            }
            Self::Xresources(body) => Ok(body.lines().filter_map(|line|line.split_once(':')).filter(|(name,_)|name.trim() == key).map(|(_,value)|value.trim().into()).next_back()),
        }
    }

    fn set(&mut self, section: &str, key: &str, value: Option<&str>) -> Result<()> {
        match self {
            Self::Ini(body) => *body = crate::icons::apply::ini_set(body,section,key,value),
            Self::Toml(body) => {
                if !section.is_empty() && !body.contains_key(section) && value.is_some() {
                    body[section] = toml_edit::Item::Table(toml_edit::Table::new());
                }
                let table:Option<&mut dyn toml_edit::TableLike> = if section.is_empty() { Some(body.as_table_mut()) } else { body.get_mut(section).and_then(toml_edit::Item::as_table_like_mut) };
                if let Some(table) = table {
                    match value {
                        Some(value) => {
                            let mut replacement = toml_edit::Value::from(value);
                            if let Some(old) = table.get(key).and_then(toml_edit::Item::as_value) { *replacement.decor_mut() = old.decor().clone(); }
                            table.insert(key,toml_edit::Item::Value(replacement));
                        }
                        None => { table.remove(key); }
                    }
                } else { ensure!(value.is_none(),"theme preference group {section} is not a table"); }
            }
            Self::Xresources(body) => {
                let mut next = String::with_capacity(body.len());
                let mut found = false;
                for line in body.lines() {
                    if line.split_once(':').is_some_and(|(name,_)|name.trim() == key) {
                        if !found && let Some(value) = value { next.push_str(key);next.push_str(": ");next.push_str(value);next.push('\n'); }
                        found = true;
                    } else { next.push_str(line);next.push('\n'); }
                }
                if !found && let Some(value) = value { next.push_str(key);next.push_str(": ");next.push_str(value);next.push('\n'); }
                *body = next;
            }
        }
        Ok(())
    }

    fn finish(self) -> String {
        match self { Self::Ini(body)|Self::Xresources(body) => body,Self::Toml(body) => body.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_edits_preserve_comments_and_other_fields() {
        let mut document = Format::Toml.parse("# personal\n[theme]\nborder-color = '#123456' # border\nfont = 'Keep'\n").unwrap();
        assert_eq!(document.get("theme","border-color").unwrap().as_deref(),Some("#123456"));
        document.set("theme","border-color",Some("#abcdef")).unwrap();
        let body = document.finish();
        assert!(body.contains("# personal") && body.contains("# border") && body.contains("font = 'Keep'"));
        assert_eq!(toml::from_str::<toml::Value>(&body).unwrap()["theme"]["border-color"].as_str(),Some("#abcdef"));
        assert!(Format::Toml.parse("[theme\n").is_err());
        let mut inline = Format::Toml.parse("theme = {border-color = '#123456', border-width = 3}\n").unwrap();
        inline.set("theme","border-color",Some("#abcdef")).unwrap();
        let value:toml::Value = toml::from_str(&inline.finish()).unwrap();
        assert_eq!(value["theme"]["border-width"].as_integer(),Some(3));
    }

    #[test]
    fn resource_edits_remove_duplicate_owned_keys_and_keep_unrelated_lines() {
        let mut document = Format::Xresources.parse("# personal\nwindow.active.border.color: #111111\nborder.width: 4\nwindow.active.border.color: #222222\n").unwrap();
        assert_eq!(document.get("","window.active.border.color").unwrap().as_deref(),Some("#222222"));
        document.set("","window.active.border.color",Some("#abcdef")).unwrap();
        let body = document.finish();
        assert_eq!(body.matches("window.active.border.color:").count(),1);
        assert!(body.contains("# personal\n") && body.contains("border.width: 4\n"));
    }
}
