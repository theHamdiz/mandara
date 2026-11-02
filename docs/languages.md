# Languages

English · العربية · فارسی · اردو · Español · Français · Deutsch · Türkçe · Русский · हिन्दी · বাংলা · Bahasa Indonesia · 简体中文 · 日本語 · 한국어

Fifteen languages. The whole interface is translated: the shell, Studio and
the login screen. Pick one in Studio → General → Language, or leave it on
**System** to follow your desktop's language (`LANG`, `LC_MESSAGES`,
`LC_ALL`).

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

Korean, Chinese, Japanese, Hindi and Bengali use your system's fonts:
Noto Sans CJK (KR, SC or JP, matching the language), Noto Sans Devanagari
and Noto Sans Bengali, each at its closest weight. `mandara install`
installs them with the other fonts. To install them yourself:

| Distribution | Korean, Chinese, Japanese | Bengali |
|---|---|---|
| Arch | `noto-fonts-cjk` | `noto-fonts` |
| Debian, Ubuntu | `fonts-noto-cjk` | `fonts-noto-core` |
| Fedora | `google-noto-sans-cjk-fonts` | `google-noto-sans-bengali-fonts` |
| openSUSE | `noto-sans-cjk-fonts` | `noto-sans-bengali-fonts` |
| Void | `noto-fonts-cjk` | `noto-fonts-ttf` |
| Alpine | `font-noto-cjk` | `font-noto-bengali` |
| Gentoo | `media-fonts/noto-cjk` | `media-fonts/noto` |
| NixOS | `noto-fonts-cjk-sans` | `noto-fonts` |

## Dates and the rest

Dates are localized: month and day names are in your language, in its own
word order. You can also choose a different language for dates. Plugins
carry their own translations, and the shell translates their views
automatically.
