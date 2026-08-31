#!/usr/bin/env bash
# Smoke-тест: headless-запуск MIDlet с ВКЛЮЧЁННОЙ ASM-инструментацией (ASM 9),
# дамп кадра в PNG в двух независимых прогонах и сравнение sha256
# (детерминизм рендера — важен для checkpoint-диффов Блока A Arlecchino).
#
# Фаза 1: тестовый MIDlet, собранный javac --release 8 (class file major 52 —
#         современный байткод, на котором падал ASM 3.1).
# Фаза 2 (опциональная): оригинальный телефонный jar (старый байткод, major 45-49),
#         путь берётся из $SMOKE_PHONE_JAR или дефолта; если файла нет — фаза
#         пропускается с предупреждением.
set -euo pipefail
cd "$(dirname "$0")"

RT=../build/midp-rt.jar
ASM=../lib/asm-9.8.jar
[ -f "$RT" ] || { echo "Сначала ../build.sh"; exit 1; }

PHONE_JAR="${SMOKE_PHONE_JAR:-$HOME/Загрузки/allods_rage_of_mages_mobportalnet_176x220_ru.jar}"

rm -rf out && mkdir -p out/classes

# 1. Тестовый MIDlet → test.jar (major 52)
javac --release 8 -nowarn -cp "$RT" -d out/classes TestMidlet.java
jar --create --file out/test.jar --manifest mf.txt -C out/classes .

# 2. Smoke-лаунчер
javac --release 8 -nowarn -cp "$RT" -d out SmokeLaunch.java

run_pair() {
    # run_pair <метка> <jar> [доп. аргументы Common...]
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
        echo "SMOKE OK [$label]: кадры двух прогонов бит-в-бит идентичны"
    else
        echo "SMOKE FAIL [$label]: кадры различаются!"
        exit 1
    fi
}

# 3. Фаза 1: современный байткод (major 52), инструментация ВКЛ
run_pair modern "$(pwd)/out/test.jar"

# 4. Фаза 2: оригинальный телефонный jar (старый байткод), инструментация ВКЛ
if [ -f "$PHONE_JAR" ]; then
    run_pair phone "$PHONE_JAR"
else
    echo "SMOKE SKIP [phone]: телефонный jar не найден ($PHONE_JAR);"
    echo "  укажите путь через SMOKE_PHONE_JAR=<game.jar>"
fi

# 5. Фаза 3: headless-захват кадров (--frames-dir + index.json + --inputs):
#    автостарт по манифесту, 12 кадров; NUM5 на t=600мс должен дать НОВОЕ
#    состояние экрана (TestMidlet рисует последний keyCode цветной полосой)
printf '[{"t_ms": 600, "key": "NUM5", "action": "tap"}]' > out/inputs.json
java -Djava.awt.headless=true -cp "$RT:$ASM" org.microemu.app.Headless \
    --frames-dir out/cap --frames 12 --period-ms 100 --boot-wait-ms 1500 \
    --inputs out/inputs.json --rms out/cap_rms "$(pwd)/out/test.jar" >out/cap.log 2>&1
[ -f out/cap/index.json ] || { echo "SMOKE FAIL [capture]: нет index.json"; cat out/cap.log; exit 1; }
N_PNG=$(ls out/cap/frame_*.png | wc -l)
N_STATES=$(grep -o '"sha256": "[0-9a-f]*"' out/cap/index.json | sort -u | wc -l)
N_NONBLANK=$(grep -c '"blank": false' out/cap/index.json || true)
if [ "$N_PNG" -eq 12 ] && [ "$N_STATES" -ge 2 ] && [ "$N_NONBLANK" -ge 1 ]; then
    echo "SMOKE OK [capture]: 12 PNG, состояний $N_STATES (>=2: ввод дошёл), непустых $N_NONBLANK"
else
    echo "SMOKE FAIL [capture]: png=$N_PNG states=$N_STATES nonblank=$N_NONBLANK"
    cat out/cap.log
    exit 1
fi

echo "SMOKE OK: рендер детерминирован (инструментация ASM включена)"
