#!/usr/bin/env bash
# Builds midp-rt.jar (the J2ME runtime for Arlecchino).
# Requires JDK 17+. Compiled with --release 17: the ShellFrameBridge frame bridge
# (org.microemu.app.shell) uses a Unix-domain SocketChannel / UnixDomainSocketAddress
# (Java 16+). The rest of the runtime is Java 8 compatible; the target JDK is 17.
# The upstream Maven build is dead and not used.
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

# List of sources
: > build/sources.txt
for m in "${MODULES[@]}"; do
    find "$m/src/main/java" -name '*.java' >> build/sources.txt
done
echo "Sources: $(wc -l < build/sources.txt)"

"$JAVAC" --release 17 -encoding UTF-8 -nowarn \
    -cp "$ASM" \
    -d "$OUT" \
    @build/sources.txt

# Resources (device.xml, device PNGs for default/resizable etc.)
for m in "${MODULES[@]}"; do
    if [ -d "$m/src/main/resources" ]; then
        cp -r "$m/src/main/resources/." "$OUT/"
    fi
done

"$JAR" --create --file build/midp-rt.jar \
    --main-class org.microemu.app.Headless \
    -C "$OUT" .

echo "OK: build/midp-rt.jar ($(du -h build/midp-rt.jar | cut -f1))"
echo "Run: java -cp build/midp-rt.jar:$ASM org.microemu.app.Headless <game.jar>"
