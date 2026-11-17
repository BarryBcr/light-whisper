#!/usr/bin/env python3
"""Exactitude d'une transcription du banc face au texte lu par `say`.

Pourquoi : le découpage en morceaux ne doit pas perdre de mots aux coupures. On aligne
les mots transcrits sur le texte source (difflib) et on rapporte :
- couverture : part des mots source (jusqu'au dernier mot reconnu) retrouvés ;
- precision  : part des mots transcrits qui existent dans la source, à leur place.

Usage : exactitude.py audio/long.txt resultats/morceaux-180s.txt [...]
"""
import difflib
import re
import sys


def mots(texte):
    return re.findall(r"[\w']+", texte.lower())


def mesurer(source, hypothese):
    s, h = mots(source), mots(hypothese)
    m = difflib.SequenceMatcher(None, s, h, autojunk=False)
    blocs = m.get_matching_blocks()
    trouves = sum(b.size for b in blocs)
    fin_source = max((b.a + b.size for b in blocs if b.size), default=0)
    couverture = trouves / fin_source if fin_source else 0.0
    precision = trouves / len(h) if h else 0.0
    return couverture, precision, len(h)


def main():
    source = open(sys.argv[1], encoding="utf-8").read()
    for chemin in sys.argv[2:]:
        hyp = open(chemin, encoding="utf-8").read()
        c, p, n = mesurer(source, hyp)
        print(f"{chemin}\tmots={n}\tcouverture={c:.3f}\tprecision={p:.3f}")


if __name__ == "__main__":
    main()
