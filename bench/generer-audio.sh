#!/usr/bin/env bash
# Génère l'audio de test du banc : texte français lu par la voix système « Eddy »,
# répété pour obtenir ~7 min, converti en WAV 16 kHz mono (format attendu par Parakeet).
set -euo pipefail
cd "$(dirname "$0")/audio"
[ -f long16k.wav ] && exit 0
for _ in $(seq 1 14); do cat texte.txt; done > long.txt
say -v "Eddy (Français (France))" -f long.txt -o long.aiff
afconvert -f WAVE -d LEI16@16000 -c 1 long.aiff long16k.wav
rm -f long.aiff
