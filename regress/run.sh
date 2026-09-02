#!/usr/bin/env bash
# Reproduce the numbers in regress/expected.txt.
#
#   regress/run.sh [CORPUS_DIR]        # default: ~/Загрузки/J2ME
#
# Builds nothing: point ARLECCHINO at a binary, or let the script use
# target/release/arlecchino.  Every jar is decompiled with regress/engines.6.toml
# (see that file for why six and not eight) and the two headline numbers of the
# "RESULT merged:" line are compared with expected.txt.
#
# The quality-metrics stage is off (--no-metrics): it is not part of the gate and
# costs minutes per jar.  Exit status 0 = every jar matched.
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(dirname "$HERE")"
CORPUS="${1:-$HOME/Загрузки/J2ME}"
BIN="${ARLECCHINO:-$REPO/target/release/arlecchino}"
CFG="$HERE/engines.6.toml"
EXP="$HERE/expected.txt"
# Optional private overlay: same format, not tracked (see .gitignore).  Jars whose
# corpus is not ours to name live here, so the published gate stays publishable
# while the local one still covers the whole corpus.
EXP_LOCAL="$HERE/expected.local.txt"
WORK="$(mktemp -d "${TMPDIR:-/var/tmp}/arlecchino-regress.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

[ -x "$BIN" ] || { echo "no binary at $BIN (cargo build --release, or set ARLECCHINO)"; exit 2; }
[ -d "$CORPUS" ] || { echo "no corpus dir: $CORPUS"; exit 2; }

fail=0; seen=0
printf '%-38s %14s %14s\n' jar expected got
for jar in "$CORPUS"/*.jar "$CORPUS"/*.JAR; do
  [ -e "$jar" ] || continue
  stem="$(basename "$jar")"; stem="${stem%.*}"
  want="$(awk -v s="$stem" '$1==s {print $2"/"$3}' "$EXP")"
  if [ -z "$want" ] && [ -f "$EXP_LOCAL" ]; then
    want="$(awk -v s="$stem" '$1==s {print $2"/"$3}' "$EXP_LOCAL")"
  fi
  [ -n "$want" ] || { printf '%-38s %14s %14s\n' "$stem" "-" "(no entry)"; continue; }
  seen=$((seen+1))
  log="$WORK/$stem.log"
  "$BIN" decompile "$jar" -o "$WORK/$stem" --config "$CFG" --no-metrics >"$log" 2>&1
  got="$(sed -n 's/.*isolated-sum=\([0-9]*\), whole-tree errors=\([0-9]*\).*/\1\/\2/p' "$log" | tail -1)"
  [ -n "$got" ] || got="(no RESULT)"
  mark=""; [ "$got" = "$want" ] || { mark="  <-- MISMATCH"; fail=$((fail+1)); }
  printf '%-38s %14s %14s%s\n' "$stem" "$want" "$got" "$mark"
  rm -rf "$WORK/$stem"
done
echo
echo "$seen jar(s) checked, $fail mismatch(es)"
[ "$fail" -eq 0 ]
