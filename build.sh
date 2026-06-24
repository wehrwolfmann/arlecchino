#!/usr/bin/env bash
# Сборка midp-rt.jar (рантайм J2ME для Arlecchino).
# Требуется JDK 17+ (компиляция с --release 8). Maven upstream мёртв и не используется.
set -euo pipefail
cd "$(dirname "$0")"

JAVAC="${JAVAC:-javac}"
JAR="${JAR:-jar}"
OUT=build/classes
ASM=lib/asm-9.8.jar

rm -rf build
mkdir -p "$OUT"

MODULES=(
    microemu-cldc
    microemu-midp
    microemu-javase
    microemu-javase-swing
    microemu-extensions/microemu-nokiaui
    microemu-extensions/microemu-siemensapi
)

# Список исходников
: > build/sources.txt
for m in "${MODULES[@]}"; do
    find "$m/src/main/java" -name '*.java' >> build/sources.txt
done
echo "Исходников: $(wc -l < build/sources.txt)"

"$JAVAC" --release 8 -encoding UTF-8 -nowarn \
    -cp "$ASM" \
    -d "$OUT" \
    @build/sources.txt

# Ресурсы (device.xml, png устройств default/resizable и т.п.)
for m in "${MODULES[@]}"; do
    if [ -d "$m/src/main/resources" ]; then
        cp -r "$m/src/main/resources/." "$OUT/"
    fi
done

"$JAR" --create --file build/midp-rt.jar \
    --main-class org.microemu.app.Headless \
    -C "$OUT" .

echo "OK: build/midp-rt.jar ($(du -h build/midp-rt.jar | cut -f1))"
echo "Запуск: java -cp build/midp-rt.jar:$ASM org.microemu.app.Headless <game.jar>"
