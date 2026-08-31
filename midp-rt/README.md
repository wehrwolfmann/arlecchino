# arlecchino-midp-rt

LGPL-форк [MicroEmulator](https://github.com/barteo/microemu) — рантайм J2ME (MIDP 2.0 / CLDC 1.1)
для запуска мобильных игр на десктопной JVM. Используется проектом **Arlecchino** как
**отдельный процесс** (отдельный артефакт `midp-rt.jar`; в Apache-2.0-код Arlecchino
код этого репозитория не попадает — граница LGPL = граница процесса).

## Происхождение

- **База:** [barteo/microemu](https://github.com/barteo/microemu)
  @ `e47b656d983deaa67810c776662444483bdbf134` (MicroEmulator 3.0.0-SNAPSHOT,
  последнее состояние upstream-кода ~2010 г.).
- **Фиксы:** 5 коммитов из форка
  [nikita36078/microemu](https://github.com/nikita36078/microemu)
  @ `eb7e7d38bf842fc51777a671d425b03eb7e7535e` (автор J2ME-Loader), перенесены вручную:
  - Remove ClosedConnection (`6575842`)
  - Fix GameCanvas drawing (`70c8503`)
  - Improve exceptions handling (`e7c9da6`)
  - Delete unused classloaders (`5b36135`)
  - Fix Nokia games loading issue (`eb7e7d3`)

Полный список изменений относительно upstream — в [NOTICE](NOTICE) и истории git.

## Лицензия

**GNU LGPL 2.1** (см. [LICENSE](LICENSE)). Бо́льшая часть upstream-файлов
дуально лицензирована LGPL 2.1 / Apache 2.0 (см. `COPYING`, `COPYING-AL-2.0`),
но ~35 файлов (весь `javax.microedition.lcdui.game.*`, часть lcdui-виджетов,
`device/impl/Polygon.java`) — только LGPL 2.1, поэтому совокупный артефакт
распространяется на условиях LGPL 2.1. Исходники форка публикуются.

## Состав (что взято из upstream)

| Модуль | Содержимое |
|---|---|
| `microemu-cldc` | `javax.microedition.io` (Connector, *Connection) |
| `microemu-midp` | ядро: `javax.microedition.{lcdui, lcdui.game, midlet, rms, io, pki}`, `org.microemu.*` |
| `microemu-javase` | запуск jar/jad (`org.microemu.app.Common`), classloader+ASM-инструментация, RMS-менеджеры, ресурсы устройств (default/resizable) |
| `microemu-javase-swing` | рендер Java2D (`org.microemu.device.j2se.*`, BufferedImage), headless-запуск (`org.microemu.app.Headless`, `NoUiDisplayComponent`), Swing-GUI (`org.microemu.app.Main`) |
| `microemu-extensions/microemu-nokiaui` | Nokia UI API (`com.nokia.mid.*`) |
| `microemu-extensions/microemu-siemensapi` | Siemens API (`com.siemens.mp.*`) |

Выкинуто из upstream: applet, SWT, webstart, android, iphone, скины устройств,
JSR-75/120/82, examples/tests, Maven-сборка. Звук JSR-135 (MMAPI) в форке
написан с нуля по публичной спецификации JSR-135:
`javax.microedition.media[.control]` (`Manager`/`Player`/`Controllable`/
`VolumeControl`/`ToneControl` и т.д.) + реальные плееры в
`org.microemu.media`: **MIDI** → `MidiPlayer` (javax.sound.midi, громкость
перехватом CC7), **WAV/AU/AIFF** → `SampledPlayer` (javax.sound.sampled,
громкость MASTER_GAIN), `Manager.playTone` → `ToneGenerator`. Тип носителя
определяется по content-type или сигнатуре данных (MThd/RIFF/.snd/FORM);
неподдержанные форматы (AMR, MP3 и т.п.) получают безопасный no-op
`NoopPlayer` с корректной state-machine — игра работает без звука, без
`NoClassDefFoundError`. Системное свойство `org.microemu.media.noop=true`
принудительно отключает звук; headless-режим (`org.microemu.app.Headless`)
включает его по умолчанию — diff-тесты кадров детерминированы и не зависят
от аудиоустройства.
`microemu-injected` не копировался: его единственный класс `org.microemu.Injected`
дублируется (другой вариант того же FQCN) в `microemu-javase` — взят вариант javase.

## Сборка

Требуется JDK 17 (компиляция с `--release 8`). Maven upstream мёртв и не используется.

```sh
./build.sh        # → build/midp-rt.jar
```

## Запуск

```sh
# headless (по умолчанию Main-Class), RMS в памяти
java -cp build/midp-rt.jar:lib/asm-9.8.jar org.microemu.app.Headless <game.jar|game.jad|MIDletClass>

# Swing-окно
java -cp build/midp-rt.jar:lib/asm-9.8.jar org.microemu.app.Main <game.jar>
```

Полезные флаги CLI: `--rms (file|memory)`, `--resizableDevice W H`,
`--appclassloader system` (для MIDlet-класса с classpath),
`--no-instrument` (отключить ASM-инструментацию классов MIDlet).

## Headless-захват кадров (для difftest Arlecchino)

`org.microemu.app.Headless` умеет режим захвата (свои флаги форка, снимаются
до передачи остатка в `Common`):

```sh
java -Djava.awt.headless=true -cp build/midp-rt.jar:lib/asm-9.8.jar \
    org.microemu.app.Headless \
    --frames-dir out/cap --frames 30 --period-ms 100 --boot-wait-ms 2000 \
    --inputs script.json --rms out/rms game.jar
```

- `--frames-dir DIR` — включает захват: PNG-кадры `frame_NNNN.png` +
  `index.json` (sha256 сырых RGB24-байтов кадра, размеры, признак `blank` =
  все пиксели одного цвета); по концу захвата процесс завершается сам;
- `--frames N` / `--period-ms MS` / `--boot-wait-ms MS` — сколько кадров,
  период сэмплирования и пауза после старта MIDlet до первого кадра;
- `--inputs FILE` — JSON-скрипт ввода `[{"t_ms":4500,"key":"FIRE","action":"tap"},…]`
  (формат difftest Arlecchino, `docs/difftest_format.md` в arlecchino-remake);
  `t_ms` отсчитывается от конца boot-wait; имена клавиш: `UP/DOWN/LEFT/RIGHT`,
  `NUM0`–`NUM9`, `STAR`, `POUND`, `FIRE`/`SELECT`/`SOFT3`, `SOFT_LEFT`,
  `SOFT_RIGHT`, `CLR`; число вместо имени = сырой MIDP-keyCode;
- `--rms DIR` — расширение upstream `--rms (file|memory)`: значение-путь =
  file-backed RMS **плоско** в DIR (без `suite-<имя>` — store-файлы сравнимы
  между оригинальным и пересобранным jar даже при разных именах MIDlet-классов);
- автостарт: если последний аргумент — `.jar`/`.jad`, класс из `MIDlet-1`
  достаётся из манифеста/джада и подставляется class-name-инвокация
  (headless-Launcher не репейнтится, без автостарта кадры были бы пустыми).

## ASM-инструментация

`MIDletClassLoader` прогоняет классы игры через ASM-инструментацию
(редирект `System.out`/`System.err` в `org.microemu.Injected`, подмена
`Thread`/`Timer`/`TimerTask`, `getResourceAsStream`). Бандлится **ASM 9.8**
(BSD-3-Clause, отдельный jar в `lib/`, см. NOTICE); код
`org.microemu.app.classloader.*` портирован с ASM 3.1 на API ASM 9.
Инструментация работает и на оригинальных телефонных jar (старый байткод,
major 45–49), и на современном байткоде (major 52+) — вшитый в upstream
ASM 3.1 на major 52+ падал (историческое ограничение B1', снято в B2').

Отключение — флагом CLI `--no-instrument` (теряется System.out-редирект
и перехват исключений; для рантайма некритично).

Примечание: `ClassWriter(0)` сохраняет stack map frames входного класса как
есть; нестандартная опция `--enhanceCatchBlock` (выключена по умолчанию)
вставляет байткод в catch-блоки и на major 51+ может потребовать пересчёта
frames — на ней апгрейд не проверялся.

## Smoke-тест

```sh
./smoke/smoke.sh   # инструментация ВКЛ; три фазы:
                   #  1) тестовый MIDlet major 52 (современный байткод),
                   #  2) оригинальный телефонный jar (SMOKE_PHONE_JAR, опционально),
                   #  3) headless-захват: 12 PNG + index.json, ввод NUM5 даёт
                   #     новое состояние экрана (>=2 уникальных sha256);
                   # фазы 1–2: два headless-прогона, дамп кадра, сравнение sha256
```

Звук (JSR-135) проверяется отдельным живым smoke'ом — слышно из колонок:

```sh
javac --release 8 -cp build/midp-rt.jar -d smoke/out smoke/MmapiSmoke.java
java -cp build/midp-rt.jar:smoke/out MmapiSmoke <file.mid>
# сценарий Аллодов: createPlayer/realize/prefetch/setLoopCount(-1)/start,
# затем VolumeControl setLevel/setMute и Manager.playTone
```

Зафиксированные факты (2026-06-10/11, JDK 17.0.19, Linux), всё — с ВКЛЮЧЁННОЙ
ASM 9-инструментацией:

- тестовый MIDlet **major 52** (Canvas: fillRect + drawString + key-полоса,
  176×176): два независимых headless-прогона дают **бит-в-бит одинаковый
  PNG-кадр** — sha256
  `00d27a40f5686289c20438f26cb88df9cff1275b9b811e5aa78843e82bf85e16`
  (2026-06-11, после добавления key-полосы в TestMidlet);
- оригинальный телефонный jar **Allods / Rage of Mages** (Nival 2005,
  176×220, class file **major 45**, GameCanvas + MMAPI): рендерится сплэш
  Nival Interactive, два прогона бит-в-бит — sha256
  `ad301c11ef8224c8e0609a3144d4f402b5f999a7b0cdebcd5d6dde1599c7838b`;
- headless-захват (фаза 3 + живой прогон 240×320): Аллоды
  `Allods_1_2_240x320.jar` с input-скриптом 2×`FIRE` доходят до игрового
  экрана (30 кадров, 6 уникальных состояний, 0 пустых), RMS-store
  `system.rms` пишется плоско в `--rms`-каталог.

Рендер детерминирован (важно для checkpoint-диффов Блока A / A7 Arlecchino).
Оговорка: проверено на одной машине/JDK; шрифтовые метрики могут отличаться между
платформами/версиями JDK — кросс-платформенный детерминизм не утверждается.

Headless-рендер GameCanvas-игр работает благодаря форк-фиксу: upstream
`J2SEDeviceDisplay.flushGraphics` жёстко кастовал display component к
`SwingDisplayComponent` и в headless молча ронял поток игры
`ClassCastException`'ом — введён общий интерфейс
`J2SEGraphicsSurfaceDisplayComponent` (Swing + NoUi).
