# Ліцензії та сторонні матеріали

**Код програми** (`src/`, `examples/`, `tools/`, `build.rs`, `assets/app.*`) — ліцензія MIT, див. [LICENSE](LICENSE).

**Дані у теці `dict/` мають власні ліцензії** (вони не під MIT):

| Файл | Походження | Ліцензія |
|---|---|---|
| `dict/en.txt`, `dict/uk.txt` | Відфільтровані й упорядковані за частотністю списки з [hermitdave/FrequencyWords](https://github.com/hermitdave/FrequencyWords) (за корпусом OpenSubtitles 2018). Зміни: відсіяно слова з малою кількістю входжень і сміття, лишено лише потрібні символи. Скрипт: `tools/build_freq_dicts.py`. | [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) — ці два файли поширюються на тих самих умовах |
| `dict/uk_forms.fst` | Індекс українських словоформ, розгорнутих зі словника `uk_UA` для LibreOffice (автор Andriy Rysin, 2007–2017, проєкт [dict_uk](https://github.com/brown-uk/dict_uk)). Зміни: розгортання афіксів (`examples/expand_hunspell.rs`), відсів, варіанти без апострофа, стиснення в FST. Скрипт: `tools/build_uk_forms.py`. | [MPL 1.1](https://www.mozilla.org/en-US/MPL/1.1/) |
| `dict/en_it.txt`, `dict/*_user.txt` | Власний перелік ІТ-термінів і шаблони для власних слів | MIT, як код |

**Іконка.** `assets/icon_dual_source.png` — малюнок, наданий власником проєкту; з нього `tools/make_icon.py` збирає
великі розміри `assets/icon*.ico`, а для малих розмірів сам малює спрощений варіант. Запасні стилі іконки (`lime`, `neon`, `flag`)
намальовані тим самим скриптом; літери в них виведено шрифтом DejaVu Sans Bold (вільна ліцензія шрифтів DejaVu / Bitstream Vera,
яка дозволяє таке використання).

**Бібліотеки Rust**: [`fst`](https://crates.io/crates/fst) (MIT або Unlicense), [`windows-sys`](https://crates.io/crates/windows-sys)
(MIT або Apache-2.0), під час збирання [`embed-resource`](https://crates.io/crates/embed-resource) (MIT).
