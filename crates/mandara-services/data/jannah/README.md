# Jannah content: provenance

This folder holds the scripture behind the Jannah plugin. It is sacred
text, so accuracy comes first: every verse and hadith was copied by a
program from the sources below. Nothing was retyped, paraphrased,
shortened, "fixed" or reconstructed from memory.

| File | What it holds |
|---|---|
| `quran.json` | 114 surahs (metadata) and the 123 verses that mention Paradise |
| `hadith.json` | the 584 hadith of Sahih al-Bukhari (311) and Sahih Muslim (273) that mention Paradise |
| `hadith-ai-<lang>.json` | added separately: AI renderings of the hadith meanings in German, Spanish, Hindi, Chinese and Japanese (see below) |

The paths (deeds and manners that lead to Jannah) are in
`src/jannah/paths.rs`; each cites only items in these two files.

## Sources and licenses

### Qur'an

- **Text:** Tanzil Quran Text (Uthmani), by the Tanzil Project,
  <https://tanzil.net>, fetched through
  `https://api.alquran.cloud/v1/quran/quran-uthmani` on 2026-11-02.
  Tanzil's terms allow verbatim copies with attribution to the Tanzil
  Project and a link to tanzil.net; changing the text is not allowed. The
  text is kept exactly, including the small Uthmani marks (for example the
  dagger alif in `جَنَّٰتٍۢ`). The interface must show the attribution and
  the link (they are in `quran.json` → `source`).
- **Surah metadata** (Arabic name, transliteration, meaning, number of
  ayahs): `https://api.alquran.cloud/v1/meta`. The transliteration and
  meaning are kept as the source spells them (`Al-Baqara`, `The Cow`). The
  Arabic name is the source's name with `سورة`, the diacritics, the dagger
  alif and the tatweel removed (`آلِ` written as `آل`).
- **No Qur'an translation is bundled.** The plugin fetches translations at
  runtime. `en.sahih` (Saheeh International) was read only while curating.
- alquran.cloud puts the basmala in front of ayah 1 of every surah except
  1 and 9. No selected verse is an ayah 1, which the build checked, so no
  verse carries that prefix.

### Hadith

- **Dataset:** [fawazahmed0/hadith-api](https://github.com/fawazahmed0/hadith-api),
  released under the Unlicense (public domain). Editions `ara-bukhari`,
  `ara-muslim`, `eng-`, `ind-`, `tur-`, `rus-` and `fra-` of both
  collections, from
  `https://cdn.jsdelivr.net/gh/fawazahmed0/hadith-api@1/editions/<edition>.min.json`
  (with `editions.json` and `info.json`), fetched on 2026-11-02.
- **Arabic:** the dataset's diacritized text, verbatim, isnad included.
- **Translations,** as the dataset's `editions.json` credits them:
  - English: Bukhari by Muhammad Muhsin Khan; Muslim by Abdul Hamid Siddiqui.
  - Russian: Bukhari by Abdullah Nirsha and the editors of Daura.com
    (source isnad.link); Muslim unnamed (source isnad.link).
  - Indonesian, Turkish, French: the dataset names no translator.
- Languages are matched by the dataset's `hadithnumber`. Before relying on
  that, 18 random numbers per collection were compared by meaning between
  the Arabic and the English (they line up; one English entry carries
  extra text from a neighbouring hadith), and the Paradise passage of the
  English of all 613 raw matches was then read beside the Arabic while
  tagging themes. Every
  selected hadith's translations were checked for a word for Paradise, and
  each miss was read (see "Translations left out").
- **AI renderings.** German, Spanish, Hindi, Chinese and Japanese have no
  translation in the dataset. Their texts are AI renderings of the meaning,
  stored in `hadith-ai-<lang>.json` (added separately), and the interface
  always labels them as such. They are never presented as a translation.

## Selection rules

### Qur'an: every verse that mentions Paradise

1. **The janna family with a fatha on the jīm.** A word counts when its
   jīm carries a fatha and its nūn a shadda, followed by `ة` or `ت`
   (optionally through an alif): `جَنَّة`, `ٱلْجَنَّة`, `جَنَّٰت`,
   `ٱلْجَنَّات`, `جَنَّتَان`, `جَنَّتَيْن`, with any proclitic. The
   comparison is done on the diacritized text, because the bare letters
   `جنة` also spell `جِنَّة` (jinn, madness) and `جُنَّة` (shield). This
   finds 147 occurrences in 148 verses (the same 147 a diacritic-free count
   gives once the 11 `جِنَّة`/`أَجِنَّة` and 2 `جُنَّة` are taken out).
2. **The names of Paradise:** `الفردوس` (18:107, 23:11), `دار السلام`
   (6:127, 10:25), `دار المقامة` (35:35), `دار المتقين` (16:30), compared
   without diacritics.
3. **Earthly gardens are excluded** (below), leaving **123 verses**.

#### Excluded: other words spelled جنة

| Verses | Word | Meaning |
|---|---|---|
| 114:6, 7:184, 11:119, 23:25, 23:70, 32:13, 34:8, 34:46, 37:158 | `جِنَّة` (kasra) | jinn, or madness |
| 53:32 | `أَجِنَّة` | embryos |
| 58:16, 63:2 | `جُنَّة` (damma) | a shield (their oaths) |

#### Excluded: earthly gardens

| Verse | Why |
|---|---|
| 2:265, 2:266 | parables: a garden on a height; a garden burnt by a whirlwind |
| 6:99, 6:141 | gardens grown by rain; trellised gardens and crops |
| 13:4 | gardens of grapevines on earth |
| 17:91 | the Quraysh demand a garden of palms |
| 18:32, 18:33, 18:35, 18:39, 18:40 | the parable of the two gardens (18:34, 18:36–38, 18:41–42 do not use the word) |
| 23:19 | gardens of palms and grapevines |
| 25:8 | "does he not have a garden to eat from?" |
| 25:10 | **borderline:** "gardens beneath which rivers flow, and palaces" that Allah could have given the Prophet ﷺ instead of what they demanded. Most commentators (al-Tabari via Mujahid, Ibn Kathir) read it as this world, so it is left out. |
| 26:57, 26:134, 26:147 | the gardens of Pharaoh's people, of 'Ad, of Thamud |
| 34:15, 34:16 | the two gardens of Saba' |
| 36:34 | gardens of palms on the revived earth |
| 44:25 | the gardens Pharaoh's people left behind |
| 50:9 | gardens grown by blessed rain |
| 68:17 | the owners of the garden (68:18–33 do not use the word) |
| 71:12 | Noah promises gardens in this world |
| 78:16 | gardens of entwined growth |

#### The garden of Adam

The garden of Adam and Hawwa' is included with the theme `adam`: the
majority view is that it was Paradise itself. The verses that use the word
are 2:35, 7:19, 7:22, 7:27, 20:117 and 20:121; the rest of 7:19–27 and
20:117–121 do not, so they are not included. Hadith about it (Adam and
Moses, Friday, the intercession) carry the theme too.

#### Other decisions

- Included although they are not descriptions of reward: 2:111 (the claim
  that only Jews or Christians enter), 7:46–50 (the people of the
  Elevations), 42:7 (a party in Paradise, a party in the Blaze), 70:38
  (the disbelievers' hope to enter), 26:85 (Ibrahim's prayer), 36:26
  ("Enter Paradise") and 53:15 (Jannat al-Ma'wa by the Lote Tree). Each
  mentions Paradise itself.

### Hadith: every hadith of Bukhari and Muslim that mentions Paradise

1. **The same janna family with a fatha on the jīm**, matched on the
   dataset's diacritized Arabic, or `الفردوس` (every hadith with
   `الفردوس` also has a janna form). The word must start the token or follow
   a proclitic (`ال`, `بال`, `وال`, `فال`, `لل`, `و`, `ب`, `أ`, `أو`), which
   leaves out `مَجَنَّة` (Majanna, a pre-Islamic market).
2. **Not matched, by the vowel rule:** `جُنَّة` / `جُنَّتَان` as a shield
   ("fasting is a shield", "the imam is a shield", the cloaks of the miser
   and the spender: Bukhari 1444, 1894, 1904, 2956, 2957, 7492; Muslim 416,
   1021a–c, 1151c–d, 1841), `وَجْنَتَاه` / `الوَجْنَتَيْن` (cheeks),
   `سَجَنَتْهَا` (she imprisoned it), `سُجْنَة`, `شِجْنَة`, `دَاجِنَة`.
3. **Excluded by reading:**
   - Bukhari 4538: its only janna is a quotation of 2:266, the parable of
     an earthly garden.
   - Muslim 2934a: "the Dajjal has a garden and a fire with him": the
     Dajjal's garden, not Paradise. (Bukhari 3338 and Muslim 2936 stay:
     they speak of "the likeness of Paradise" he brings, naming Paradise.)
   - Muslim 2956: "the world is the prison of the believer and the
     paradise of the disbeliever": figurative, about this world.
4. **Combined entries.** Where sunnah.com shows several numbers as one
   hadith ("1363, 1364"), the dataset repeats the same Arabic under each
   number. It is kept once, under the first number, with each language's
   distinct texts joined in order: Bukhari 1156 (with 1157–1158), 1363
   (1364), 1791 (1792), 2615 (2616), 2805 (2806), 2965 (2966), 3024 (3025),
   3159 (3160), 3252 (3253), 3450 (3451–3452), 3623 (3624), 4326 (4327),
   4879 (4880), 6567 (6568), 6766 (6767), 7015 (7016), 7437 (7438); Muslim
   904a (904b), 963a (963b), 2301a (2301b), 2395a (2395b), 2565a (2565b),
   2827 (2828), 2830 (2831a).

That leaves **584 hadith: 311 from Bukhari and 273 from Muslim** (from 613
raw matches: 26 repeats of combined entries and 3 exclusions).

## Themes

Every verse and hadith carries one or more of these fixed ids. They were
assigned by reading each item (the Arabic and an English translation), not
by keywords alone.

| Id | Meaning |
|---|---|
| `description` | what Jannah is like: gardens, palaces, shade, rivers, food, drink, garments |
| `deeds` | actions that lead there |
| `barriers` | what keeps a person from it |
| `people` | who enters, who enters first, its people's traits |
| `mercy` | Allah's mercy, hope, forgiveness, intercession |
| `levels` | degrees, gates, al-Firdaws |
| `vision` | seeing Allah (including 10:26, "the best and more") |
| `eternity` | forever, no death, no sorrow |
| `family` | parents, children, spouses |
| `adam` | the garden of Adam |

## Numbering and how to verify

Each hadith has:

- `id`: the collection and the **sunnah.com reference**: `bukhari-3245`,
  `bukhari-774b`, `muslim-2699a`.
- `number`: the standard number. Bukhari as in Fath al-Bari (the
  dataset's `hadithnumber`); Muslim as numbered by Muhammad Fuad Abdul
  Baqi. The dataset's Muslim `hadithnumber` is its own running count
  (1–7563), which no one cites, so the Abdul Baqi number comes from the
  dataset's `arabicnumber` (`2699.01` → `2699a`, `2699.02` → `2699b`;
  Bukhari's `774.2` → `774b`).
- `book`: the book (kitab) number, as sunnah.com's in-book reference gives it.
- `url`: the hadith on sunnah.com.

**URL pattern:** `https://sunnah.com/<collection>:<reference>`, i.e. the id
with `-` replaced by `:` (`bukhari-3245` → `https://sunnah.com/bukhari:3245`,
`muslim-2699a` → `https://sunnah.com/muslim:2699a`). Two combined Muslim
pages answer only on their in-book address, so their `url` is that
address: `muslim-2827` → `https://sunnah.com/muslim/53/9` and `muslim-2830`
→ `https://sunnah.com/muslim/53/11`.

The four combined Muslim entries that carry no `arabicnumber` in the
dataset (904a, 2301a, 2395a, 2565a) take the reference sunnah.com shows
for them; each was checked on sunnah.com.

Every `url` was fetched on 2026-11-02 and the English text on the page was
compared with the dataset's English. All 584 open the same hadith: 579
matched the dataset's opening words exactly, and the other 5 (Bukhari 1053,
2809, 4437, 6408; Muslim 2750a) differ only in spelling or spacing
("Asma' bint Al Bakr", "Allah 's Apostle") and were read to confirm.

To check a verse: `quran-2:25` is surah 2, ayah 25, on
<https://tanzil.net/#2:25> or <https://quran.com/2/25>.

## Translations left out

A translation is left out, never replaced, when it is a different hadith
or misses or contradicts the Paradise content of the Arabic:

| Hadith | Language | Why |
|---|---|---|
| Bukhari 1363 | tr | covers Thabit's hadith only; Jundub's ("I have forbidden Paradise for him") is missing |
| Bukhari 1374 | tr | leaves out the believer's seat in Paradise |
| Bukhari 1791 | tr | stops before the glad tidings to Khadija |
| Bukhari 2826 | tr | leaves out that both men enter Paradise |
| Bukhari 3450 | tr | covers only the Dajjal part |
| Bukhari 3623 | tr | a different hadith (Abu Musa's dream) |
| Bukhari 6306 | tr | cut off after the opening line |
| Bukhari 6956 | tr | translates only the trailing fiqh remark |
| Bukhari 7463 | tr | says "to put him in Hell" where the Arabic says "admit him to Paradise" |
| Muslim 963a | tr | a commentary-laden text without the prayer for Paradise |
| Muslim 2565a | tr | a different narration (deeds shown on Monday and Thursday) |
| Muslim 11b, 12a, 17b | ru | cut off before the mention of Paradise |
| Muslim 216a | ru | a different hadith |
| Muslim 27b | fr | says "will never enter Hell" where the Arabic says "will not be kept from Paradise" |

Kept as the dataset has them, although a reader may notice:

- Some Turkish texts end with the translator's notes or cross-references
  ("Diğer tahric: …", "Bu hadis … numara ile gelecektir", "AÇIKLAMA …",
  and in 3 texts a "click here for the commentary" line), and a few have
  typing slips (`cenn€te`).
- Some English (and French) texts render `الجنة` as "the gates of mercy"
  (Muslim 1079a) or leave the word out (Bukhari 5227); a few Muslim
  entries in English say only "narrated through another chain"
  (for example Muslim 2834b), as the original translation does.
- Indonesian texts put narrator names in square brackets.

Coverage after this: English 584, French 579, Turkish 567, Indonesian 509,
Russian 442 of 584 (the dataset's Russian Muslim is largely empty).

## Counts

- Qur'an: 123 verses in 61 surahs.
- Hadith: 584 (Bukhari 311, Muslim 273).
- Paths: 39 (worship 10, character 6, family 4, others 6, heart 7, avoid 6).

## Rebuilding

The build scripts are throwaway and are not kept in the repository. To
rebuild, fetch the same sources, apply the rules above (the excluded verse
list, the hadith exclusions, the combined entries and the translations
left out are all listed here) and compare with these files; the tests in
`src/jannah/data.rs` guard the result (counts, uniqueness, ranges, the
fatha rule on every hadith, the excluded verses, the path evidence).
