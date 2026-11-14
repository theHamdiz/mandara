//! Catalog and categories from is-suzart/nandomo-clipboard (27e6eb2).
//! The bundled data is MIT-licensed by kaomojikan (see data/kaomoji.LICENSE).

use std::collections::HashMap;
use std::sync::OnceLock;

use gpui::{AnyElement, IntoElement, ParentElement, Styled, div, px};
use mandara_i18n::tk;
use serde::Deserialize;

use super::Item;
use crate::launcher_ext::{Hit, HitAction, Lead};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mood {
    Happy,
    Sad,
    Surprise,
    Angry,
    Crying,
    Cats,
    Other,
}

impl Mood {
    #[cfg(test)]
    const ALL: [Self; 7] = [
        Self::Happy,
        Self::Sad,
        Self::Surprise,
        Self::Angry,
        Self::Crying,
        Self::Cats,
        Self::Other,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Happy => tk("Happy"),
            Self::Sad => tk("Sad"),
            Self::Surprise => tk("Surprise"),
            Self::Angry => tk("Angry"),
            Self::Crying => tk("Crying"),
            Self::Cats => tk("Cats"),
            Self::Other => tk("Other kaomojis"),
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

#[derive(Deserialize)]
struct Source<'a> {
    text: String,
    slug: &'a str,
    categories: Vec<&'a str>,
    tags: Vec<String>,
    reading: Vec<String>,
}

#[derive(Default)]
struct Entry {
    moods: u8,
    keywords: String,
}

fn aliases(category: &str) -> &str {
    match category {
        "aa" => "arte ascii art",
        "animal" => "animal gato cachorro cat dog",
        "bikkuri" => "surpresa surprise",
        "chira" => "espiar peek",
        "copype" => "texto text",
        "cry" => "triste chorar sad cry",
        "cute" | "ryosangata" => "fofo cute",
        "egao" => "sorriso smile",
        "kirakira" => "brilho sparkles",
        "mogumogu" => "comer eating",
        "nadenade" => "carinho abraço pet hug",
        "nageru" => "jogar throw",
        "nemui" => "sono sleepy",
        "okoru" => "raiva angry",
        "ouen" => "torcida cheer",
        "suki" => "amor love",
        "taoreru" => "desmaio faint",
        "tehepero" => "travesso playful",
        "tereru" => "vergonha shy",
        "ureshii" => "feliz happy",
        _ => category,
    }
}

struct Kaomoji {
    text: String,
    slug: &'static str,
    entry: Entry,
}

fn catalog() -> &'static [Kaomoji] {
    static CATALOG: OnceLock<Vec<Kaomoji>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let source: Vec<Source<'_>> = serde_json::from_str(include_str!("../data/kaomoji.json"))
            .expect("valid bundled kaomoji catalog");
        let mut seen = HashMap::new();
        let mut items: Vec<Kaomoji> = Vec::new();
        for row in source {
            let index = *seen.entry(row.text.clone()).or_insert_with(|| {
                items.push(Kaomoji {
                    text: row.text,
                    slug: row.slug,
                    entry: Entry::default(),
                });
                items.len() - 1
            });
            let entry = &mut items[index].entry;
            for category in &row.categories {
                entry.moods |= match *category {
                    "ureshii" | "egao" => Mood::Happy.bit(),
                    "bikkuri" => Mood::Surprise.bit(),
                    "okoru" => Mood::Angry.bit(),
                    "cry" => Mood::Crying.bit(),
                    _ => 0,
                };
            }
            if row.tags.iter().any(|tag| {
                matches!(
                    tag.as_str(),
                    "しょんぼり" | "うつむき" | "がっくり" | "悲しい"
                )
            }) {
                entry.moods |= Mood::Sad.bit();
            }
            if row
                .tags
                .iter()
                .chain(&row.reading)
                .any(|term| term == "ねこ")
            {
                entry.moods |= Mood::Cats.bit();
            }
            for word in row
                .tags
                .iter()
                .chain(&row.reading)
                .map(String::as_str)
                .chain(row.categories.into_iter().map(aliases))
            {
                entry.keywords.push(' ');
                entry.keywords.push_str(&word.to_lowercase());
            }
        }
        items.sort_by_key(|item| item.text.contains('\n'));
        items
    })
}

pub(super) fn results(query: &str, mood: Option<Mood>) -> Vec<Item> {
    let query = query.to_lowercase();
    catalog()
        .iter()
        .filter(|item| {
            mood.is_none_or(|mood| {
                if mood == Mood::Other {
                    item.entry.moods == 0
                } else {
                    item.entry.moods & mood.bit() != 0
                }
            })
        })
        .filter(|item| {
            query
                .split_whitespace()
                .all(|word| item.entry.keywords.contains(word) || item.text.contains(word))
        })
        .map(|item| {
            Item::Hit(Hit {
                provider: "kaomoji",
                key: item.slug.into(),
                label: item.text.clone(),
                detail: String::new(),
                lead: Lead::Glyph(item.text.clone()),
                badge: tk("Kaomoji"),
                keywords: String::new(),
                action: HitAction::Copy(item.text.clone()),
            })
        })
        .collect()
}

/// Keep the original line breaks and indentation, including in ASCII art.
pub(super) fn tile(label: &str, height: f32) -> AnyElement {
    div()
        .w_full()
        .h(px(height))
        .px(px(6.0))
        .flex()
        .flex_col()
        .justify_center()
        .text_center()
        .text_size(px(if label.contains('\n') { 13.0 } else { 17.0 }))
        .line_height(px(16.0))
        .children(label.lines().map(|line| {
            div()
                .w_full()
                .truncate()
                .child(line.replace(' ', "\u{00a0}"))
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keeps_unique_text_categories_and_exact_clipboard_content() {
        assert_eq!(catalog().len(), 1708);
        assert!(
            Mood::ALL
                .into_iter()
                .all(|mood| !results("", Some(mood)).is_empty())
        );
        let sample = catalog()
            .iter()
            .find(|item| item.text.contains('\n'))
            .unwrap();
        let hits = results("arte", None);
        assert!(hits.iter().any(|item| matches!(item, Item::Hit(hit) if hit.action == HitAction::Copy(sample.text.clone()))));
        assert!(!results("gato", Some(Mood::Cats)).is_empty());
    }
}
