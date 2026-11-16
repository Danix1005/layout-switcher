#!/usr/bin/env python3
"""Збирає dict/en.txt і dict/uk.txt — частотні списки слів (порядок рядків = частотність).

Джерело: https://github.com/hermitdave/FrequencyWords (OpenSubtitles 2018, CC-BY-SA 4.0),
файли content/2018/en/en_full.txt і content/2018/uk/uk_full.txt (формат «слово кількість»).

  python3 tools/build_freq_dicts.py en_full.txt uk_full.txt dict/

Пороги: англ. — від 50 входжень, укр. — від 2 (український корпус значно менший).
Лишаються тільки слова з літер своєї абетки (з одним апострофом усередині); усе — малими літерами.
У корпусі багато шуму: російські слова в українському списку, слова, набрані не в тій розкладці («ьуе» = met).
Його відсіює не цей скрипт, а детектор (поняття «надійного» слова в src/detect.rs).
"""
import re, sys

UA = "абвгґдеєжзиіїйклмнопрстуфхцчшщьюя"
RE_EN = re.compile(r"^[a-z]+(?:'[a-z]+)?$")
RE_UK = re.compile(rf"^[{UA}]+(?:'[{UA}]+)?$")


def norm(word):
    return word.lower().replace("’", "'").replace("ʼ", "'")


def read_pairs(path):
    for line in open(path, encoding="utf-8"):
        p = line.split()
        if len(p) == 2 and p[1].isdigit():
            yield norm(p[0]), int(p[1])


def build_en(src, dst, threshold=50):
    seen, n = set(), 0
    with open(dst, "w", encoding="utf-8", newline="\n") as f:
        for w, c in sorted(read_pairs(src), key=lambda x: -x[1]):
            if c >= threshold and RE_EN.match(w) and w not in seen:
                seen.add(w)
                f.write(w + "\n")
                n += 1
    return n


def build_uk(src, dst, threshold=2):
    counts = {}
    for w, c in read_pairs(src):          # після нормалізації апострофів варіанти одного слова зливаються
        counts[w] = counts.get(w, 0) + c
    words = [w for w, c in sorted(counts.items(), key=lambda x: -x[1]) if c >= threshold and RE_UK.match(w)]
    with open(dst, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(words) + "\n")
    return len(words)


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    en_src, uk_src, out = sys.argv[1], sys.argv[2], sys.argv[3].rstrip("/\\")
    print("en.txt:", build_en(en_src, f"{out}/en.txt"), "слів")
    print("uk.txt:", build_uk(uk_src, f"{out}/uk.txt"), "слів")
