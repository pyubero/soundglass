# Translating soundglass

soundglass speaks 28 languages. Every text goes through `tr` or `trf` in `src/i18n.rs`, with the
English text as its key, and each language's translations are one file in `locales/`. All of
them are machine translations, partly reused from lightview, so corrections from native speakers
are the most useful help.

## The file

Each language is one file in `locales/`, named by its language code (`es.tsv`, `de.tsv`…). Every
line is the English text, a tab, and the translation:

```
Open…	Abrir…
Couldn't save {name}: {e}	No se pudo guardar {name}: {e}
```

- **Keep the English side exactly as it is.** It's how soundglass finds the line. A test fails if a
  line is missing, extra or changed.
- **Keep every `{placeholder}`.** They're filled in when the text is shown (a file name, a
  number…). You can move them, but not rename or drop them.
- **`\n` is a line break.** Write it as the two characters `\` and `n`.
- **Counts.** soundglass has no plural forms. Where a number counts something, write it so any
  number reads correctly, for example "files: {count}" rather than "{count} files".
- **One text, one translation.** The same English text can appear in several places (a menu item
  and a setting, for instance); it gets one translation.
- **Left in English on purpose:** units (dB, Hz, kb/s), note names, the AI models' own error messages
  and the licence texts.
- **Characters.** The UI font is Roboto, which covers Latin, Greek and Cyrillic, but not ♯ or ♭
  (write C#, Db). Chinese, Japanese and Korean are drawn with the system's CJK font, in the regional
  style of the interface language (`cjk_tag()` in `src/i18n.rs`). Right-to-left scripts (Arabic,
  Hebrew) and scripts that need shaping (Hindi, Thai) aren't supported.

## Adding a language

1. Make `locales/<code>.tsv` with one line for each text in the code. The test below lists any that
   are missing.
2. In `src/i18n.rs`: add the language to `Language` and to `Language::ALL`, its own name in
   `name()`, its locale code in `from_locale()`, the file in `source()`
   (`include_str!("../locales/<code>.tsv")`), and its fontconfig tag in `cjk_tag()` if it needs a
   CJK font. Add it to `TRANSLATED` and to `locale_names_map_to_languages` in the tests.
3. In `soundglass.desktop`, add `GenericName[<code>]` and `Comment[<code>]`.
4. Run `cargo test i18n`; it fails until the file has exactly the texts in the code.
5. Start soundglass and pick the language in File → Settings…. Settings are saved when soundglass
   closes, so to keep your own untouched, start it with a throwaway data folder:
   `XDG_DATA_HOME=$(mktemp -d) cargo run --release`.

## Correcting a translation

Edit the line in `locales/<code>.tsv`, run `cargo test i18n`, and send a pull request.
