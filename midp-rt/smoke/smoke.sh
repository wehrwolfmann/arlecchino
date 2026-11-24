#!/usr/bin/env bash
# Smoke test: headless MIDlet launch with ASM instrumentation ON (ASM 9),
# a frame dump to PNG in two independent runs and a sha256 comparison
# (rendering determinism matters for the checkpoint diffs of Arlecchino's Block A).
#
# Phase 1: a test MIDlet built with javac --release 8 (class file major 52 —
#          modern bytecode, on which ASM 3.1 used to fail).
# Phase 2 (optional): an original phone jar (old bytecode, major 45-49),
#          the path comes from $ARLECCHINO_SMOKE_JAR; if it is unset or the file is
#          missing, the phase is skipped with a message.
set -euo pipefail
cd "$(dirname "$0")"

RT=../build/midp-rt.jar
ASM=../lib/asm-9.8.jar
[ -f "$RT" ] || { echo "Run ../build.sh first"; exit 1; }

PHONE_JAR="${ARLECCHINO_SMOKE_JAR:-}"

rm -rf out && mkdir -p out/classes

# 1. Test MIDlet → test.jar (major 52)
javac --release 8 -nowarn -cp "$RT" -d out/classes TestMidlet.java
jar --create --file out/test.jar --manifest mf.txt -C out/classes .

# 2. Smoke launcher
javac --release 8 -nowarn -cp "$RT" -d out SmokeLaunch.java

run_pair() {
    # run_pair <label> <jar> [extra Common arguments...]
    local label="$1" jar="$2"; shift 2
    for i in 1 2; do
        java -Djava.awt.headless=true -cp "$RT:$ASM:out" SmokeLaunch \
            "out/${label}_frame$i.png" "$jar" "$@"
    done
    sha256sum "out/${label}_frame1.png" "out/${label}_frame2.png"
    local h1 h2
    h1=$(sha256sum "out/${label}_frame1.png" | cut -d' ' -f1)
    h2=$(sha256sum "out/${label}_frame2.png" | cut -d' ' -f1)
    if [ "$h1" = "$h2" ]; then
        echo "SMOKE OK [$label]: the frames of two runs are bit-for-bit identical"
    else
        echo "SMOKE FAIL [$label]: the frames differ!"
        exit 1
    fi
}

# 3. Phase 1: modern bytecode (major 52), instrumentation ON
run_pair modern "$(pwd)/out/test.jar"

# 4. Phase 2: an original phone jar (old bytecode), instrumentation ON
if [ -z "$PHONE_JAR" ]; then
    echo "SMOKE SKIP [phone]: no phone jar given;"
    echo "  set the path with ARLECCHINO_SMOKE_JAR=<game.jar>"
elif [ -f "$PHONE_JAR" ]; then
    run_pair phone "$PHONE_JAR"
else
    echo "SMOKE SKIP [phone]: phone jar not found ($PHONE_JAR);"
    echo "  set the path with ARLECCHINO_SMOKE_JAR=<game.jar>"
fi

# 5. Phase 3: headless frame capture (--frames-dir + index.json + --inputs):
#    autostart from the manifest, 12 frames; NUM5 at t=600 ms must give a NEW
#    screen state (TestMidlet draws the last keyCode as a coloured bar)
printf '[{"t_ms": 600, "key": "NUM5", "action": "tap"}]' > out/inputs.json
java -Djava.awt.headless=true -cp "$RT:$ASM" org.microemu.app.Headless \
    --frames-dir out/cap --frames 12 --period-ms 100 --boot-wait-ms 1500 \
    --inputs out/inputs.json --rms out/cap_rms "$(pwd)/out/test.jar" >out/cap.log 2>&1
[ -f out/cap/index.json ] || { echo "SMOKE FAIL [capture]: no index.json"; cat out/cap.log; exit 1; }
N_PNG=$(ls out/cap/frame_*.png | wc -l)
N_STATES=$(grep -o '"sha256": "[0-9a-f]*"' out/cap/index.json | sort -u | wc -l)
N_NONBLANK=$(grep -c '"blank": false' out/cap/index.json || true)
if [ "$N_PNG" -eq 12 ] && [ "$N_STATES" -ge 2 ] && [ "$N_NONBLANK" -ge 1 ]; then
    echo "SMOKE OK [capture]: 12 PNG, $N_STATES states (>=2: the input arrived), $N_NONBLANK non-blank"
else
    echo "SMOKE FAIL [capture]: png=$N_PNG states=$N_STATES nonblank=$N_NONBLANK"
    cat out/cap.log
    exit 1
fi

echo "SMOKE OK: rendering is deterministic (ASM instrumentation on)"
