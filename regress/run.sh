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
# The run is stopped as soon as that line appears — the stages after it
# (round-trip, quality metrics) are not part of the gate and cost minutes per jar.
# Exit status 0 = every jar matched.
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(dirname "$HERE")"
CORPUS="${1:-$HOME/Загрузки/J2ME}"
BIN="${ARLECCHINO:-$REPO/target/release/arlecchino}"
CFG="$HERE/engines.6.toml"
EXP="$HERE/expected.txt"
WORK="$(mktemp -d "${TMPDIR:-/var/tmp}/arlecchino-regress.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

[ -x "$BIN" ] || { echo "no binary at $BIN (cargo build --release, or set ARLECCHINO)"; exit 2; }
[ -d "$CORPUS" ] || { echo "no corpus dir: $CORPUS"; exit 2; }

descend() { local p=$1; echo "$p"; for c in $(ps -o pid= --ppid "$p" 2>/dev/null); do descend "$c"; done; }

fail=0; seen=0
printf '%-38s %14s %14s\n' jar expected got
for jar in "$CORPUS"/*.jar "$CORPUS"/*.JAR; do
  [ -e "$jar" ] || continue
  stem="$(basename "$jar")"; stem="${stem%.*}"
  want="$(awk -v s="$stem" '$1==s {print $2"/"$3}' "$EXP")"
  [ -n "$want" ] || { printf '%-38s %14s %14s\n' "$stem" "-" "(no entry)"; continue; }
  seen=$((seen+1))
  log="$WORK/$stem.log"
  "$BIN" decompile "$jar" -o "$WORK/$stem" --config "$CFG" >"$log" 2>&1 &
  pid=$!
  waited=0
  while kill -0 "$pid" 2>/dev/null; do
    if grep -q 'RESULT merged: isolated-sum=' "$log" 2>/dev/null; then
      for p in $(descend "$pid"); do kill -TERM "$p" 2>/dev/null; done
      sleep 2
      for p in $(descend "$pid"); do kill -KILL "$p" 2>/dev/null; done
      break
    fi
    sleep 3; waited=$((waited+3))
    [ "$waited" -gt 5400 ] && { for p in $(descend "$pid"); do kill -KILL "$p" 2>/dev/null; done; break; }
  done
  wait "$pid" 2>/dev/null
  got="$(sed -n 's/.*isolated-sum=\([0-9]*\), whole-tree errors=\([0-9]*\).*/\1\/\2/p' "$log" | tail -1)"
  [ -n "$got" ] || got="(no RESULT)"
  mark=""; [ "$got" = "$want" ] || { mark="  <-- MISMATCH"; fail=$((fail+1)); }
  printf '%-38s %14s %14s%s\n' "$stem" "$want" "$got" "$mark"
  rm -rf "$WORK/$stem"
done
echo
echo "$seen jar(s) checked, $fail mismatch(es)"
[ "$fail" -eq 0 ]
