#!/usr/bin/env python3
"""Збирає dict/uk_forms.fst — індекс усіх українських словоформ.

Потрібно: Rust (cargo). `unmunch` більше НЕ потрібен — він пропускає частину прапорців
(наприклад, теперішній час «відображається»), тому є власний розгортач examples/expand_hunspell.rs.

  1) завантажити uk_UA.dic і uk_UA.aff з https://github.com/LibreOffice/dictionaries/tree/master/uk_UA
  2) cargo run --release --example expand_hunspell -- uk_UA.dic uk_UA.aff > forms_raw.txt
  3) python3 tools/build_uk_forms.py forms_raw.txt forms_clean.txt
  4) cargo run --release --example mkfst -- forms_clean.txt dict/uk_forms.fst

Словник uk_UA (Andriy Rysin, проєкт dict_uk) поширюється за ліцензією MPL 1.1.
"""
import re, sys

UA = "абвгґдеєжзиіїйклмнопрстуфхцчшщьюя"
rx = re.compile(rf"^[{UA}]+(?:'[{UA}]+)?$")

src, dst = sys.argv[1], sys.argv[2]
forms = set()
for line in open(src, encoding="utf-8"):
    w = line.strip()
    if w.startswith("+cs="):          # службові теги hunspell-словника
        w = w[4:]
    w = w.lower().replace("ʼ", "'").replace("’", "'")
    if rx.match(w):                   # лише слова з укр. літер (+ апостроф); слова з дефісом відкидаємо
        forms.add(w)
        if "'" in w:                  # у чатах апостроф часто пропускають: «зявляється», «привязуватись»
            forms.add(w.replace("'", ""))
with open(dst, "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(sorted(forms)) + "\n")
print(f"{len(forms)} слів -> {dst}")
