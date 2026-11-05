# Languages

English · العربية · فارسی · اردو · Español · Português · Français · Deutsch · Türkçe · Русский · हिन्दी · मराठी · বাংলা · తెలుగు · Bahasa Indonesia · Kiswahili · Tiếng Việt · 简体中文 · 日本語 · 한국어

Twenty languages. The whole interface is translated: the shell, Studio and
the login screen. Pick one in Studio → General → Language, or leave it on
**System** to follow your desktop's language (`LANG`, `LC_MESSAGES`,
`LC_ALL`).

## Language packs

Translations live outside the binary, one **language pack** per language
and Mandara version (`<code>.mlang`). English needs none: it's what every
string is written in, and anything without a translation shows in English.

- **Packages and tarballs** ship every pack, so most installs never
  download one.
- **Picking a language** in Studio (or with `mandara msg`, the language
  plugin, a plugin's own language or the login screen's) downloads its
  pack when it's missing, or when the release now publishes a newer one.
  The language's own card (in Studio, and in the bar's language panel)
  fills with the download as it comes in ("Downloading Arabic… 180 KB"),
  from the reading start, then turns green with a check, and the interface
  switches as soon as the pack is in. If it can't be downloaded (offline
  mode, no network, a release that isn't signed), the card turns red with
  the reason and **Retry**; meanwhile everything stays in English.
- **Packs belong to one version.** Mandara only reads packs made for the
  version that is running, and downloads them from that version's own
  release, checked against its signed `SHA256SUMS`. An update fetches the
  new version's packs for every language you use before it restarts the
  shell, and keeps the previous version's for `mandara update rollback`.

Where they live, first match wins:
`~/.local/share/mandara/locales/<version>/` (downloads and
`mandara install`), `share/mandara/locales/<version>/` next to the binary
(an unpacked tarball), then `/usr/local/share` and `/usr/share`
(packages, and the copy Mandara Login uses). Packs are memory-mapped and
read in place: switching to a language takes well under a millisecond and
costs no extra memory for its strings. The format is described in
[releasing.md](releasing.md#language-packs).

## Right to left

Arabic, Persian and Urdu are laid out right to left. Studio mirrors every
page (sidebar, rows, sliders, switches, chevrons), while code, clock times
and screen arrangements stay left to right. Right-to-left paragraphs wrap
correctly everywhere (dialogs, notifications, notes, Studio help and plugin
descriptions), each line ordered on its own. In Arabic the brand reads
«ستوديو المندرة».

## Fonts

Arabic, Persian and Urdu text renders in your Arabic font (Cairo by
default, then Alexandria or Noto Sans Arabic) at every weight, including the
variable `wght` axis of Alexandria and Inter. These fonts cover the Persian
and Urdu letters (پ چ ژ گ ک ی, ٹ ڈ ڑ ں ے ھ ۃ) and Persian digits.

Korean, Chinese, Japanese, Hindi, Marathi, Bengali and Telugu use your
system's fonts: Noto Sans CJK (KR, SC or JP, matching the language), Noto
Sans Devanagari (Hindi and Marathi), Noto Sans Bengali and Noto Sans
Telugu, each at its closest weight. Portuguese, Vietnamese and Swahili use
the same Latin faces as English. `mandara install` installs the extra
faces with the other fonts. To install them yourself:

| Distribution | Korean, Chinese, Japanese | Devanagari, Bengali, Telugu |
|---|---|---|
| Arch | `noto-fonts-cjk` | `noto-fonts` |
| Debian, Ubuntu | `fonts-noto-cjk` | `fonts-noto-core` |
| Fedora | `google-noto-sans-cjk-fonts` | `google-noto-sans-devanagari-fonts`, `google-noto-sans-bengali-fonts`, `google-noto-sans-telugu-fonts` |
| openSUSE | `noto-sans-cjk-fonts` | `noto-sans-devanagari-fonts`, `noto-sans-bengali-fonts`, `noto-sans-telugu-fonts` |
| Void | `noto-fonts-cjk` | `noto-fonts-ttf` |
| Alpine | `font-noto-cjk` | `font-noto-devanagari`, `font-noto-bengali`, `font-noto-telugu` |
| Gentoo | `media-fonts/noto-cjk` | `media-fonts/noto` |
| NixOS | `noto-fonts-cjk-sans` | `noto-fonts` |

## Dates and the rest

Dates are localized: month and day names are in your language, in its own
word order. You can also choose a different language for dates. Plugins
carry their own translations, and the shell translates their views
automatically.
