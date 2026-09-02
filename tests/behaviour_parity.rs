//! Голден-сверка поведенческой верификации с Python-эталоном.
//!
//! Эталон `tests/golden/behaviour_parity.json` снят с питоновской редакции
//! (`arlecchino.difftest` / `diffrun` / `semdiff` / `midprun`) на тех же входах:
//! те же последовательности хэшей, те же RunSummary, тот же текст `javap`
//! (`tests/golden/javap_orig.txt` и `javap_recompiled.txt`), та же `mapping.txt`,
//! те же манифесты и синтетический jar. Абсолютные пути временного каталога в
//! эталоне заменены маркером `<TMP>` — здесь делается та же подстановка.
//!
//! Дерева `~/Projects/arlecchino-python` больше нет: оно снесено 02.09.2026,
//! когда эта редакция прошла сверку. Голден самодостаточен — он в репозитории,
//! и заново снимать его неоткуда и не нужно. Если эталон всё же понадобится
//! пересобрать, полная история питоновской редакции лежит связкой в
//! `~/Projects/_архив/arlecchino-python-полная-история.bundle`.
//!
//! Что НЕ сверяется голденом (и почему): три сознательные починки багов
//! оригинала — см. `normsig`/`canon_ref` в `semdiff` и `await_ready` в `diffrun`;
//! на них заведены отдельные тесты ниже.

use arlecchino::{difftest as dt, diffrun as dr, midprun as mr, semdiff as sd};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden")
}

fn golden() -> Value {
    let text = std::fs::read_to_string(golden_dir().join("behaviour_parity.json"))
        .expect("golden file");
    serde_json::from_str(&text).expect("golden json")
}

fn hs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Те же 9 пар последовательностей, что и в генераторе эталона.
fn seq_cases() -> Vec<(Vec<String>, Vec<String>)> {
    let mut long_a = hs(&["s"]);
    long_a.extend(std::iter::repeat_n("m".to_string(), 9));
    let mut long_b: Vec<String> = std::iter::repeat_n("s".to_string(), 6).collect();
    long_b.extend(std::iter::repeat_n("m".to_string(), 4));
    vec![
        (hs(&["a", "a", "b", "b", "b", "a"]), hs(&["a", "a", "b", "b", "b", "a"])),
        (
            hs(&["s", "s", "s", "m", "m", "a", "b", "a"]),
            hs(&["s", "s", "s", "s", "m", "a", "b", "a"]),
        ),
        (hs(&["x", "y"]), hs(&["p", "q"])),
        (hs(&["s", "m"]), hs(&["s", "m", "crash"])),
        (hs(&["s", "m", "g"]), hs(&["s", "g", "m"])),
        (vec![], vec![]),
        (hs(&["s"]), vec![]),
        (hs(&["a1", "a1", "a2", "a3"]), hs(&["b1", "b1", "b2", "b3"])),
        (long_a, long_b),
    ]
}

fn run(
    label: &str,
    hashes: &[&str],
    rms: &[(&str, &str)],
    exc: &[&str],
    error: Option<&str>,
    nonblank: bool,
) -> dt::RunSummary {
    dt::RunSummary {
        label: label.into(),
        out_dir: "/none".into(),
        hashes: hs(hashes),
        nonblank,
        rms: rms.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        exceptions: exc.iter().map(|s| s.to_string()).collect(),
        error: error.map(|s| s.to_string()),
    }
}

fn round12(x: f64) -> f64 {
    (x * 1e12).round() / 1e12
}

fn seq_value(c: &dt::SeqComparison) -> Value {
    json!({
        "jaccard": round12(c.jaccard),
        "only_a": c.only_a,
        "only_b": c.only_b,
        "order_ok": c.order_ok,
        "max_shift": c.max_shift,
        "frame_match_rate": round12(c.frame_match_rate),
        "sets_equal": c.sets_equal(),
    })
}

fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}

fn seq_dict(c: Option<&dt::SeqComparison>) -> Value {
    match c {
        None => Value::Null,
        Some(c) => json!({
            "jaccard": round4(c.jaccard),
            "sets_equal": c.sets_equal(),
            "only_a": c.only_a,
            "only_b": c.only_b,
            "order_ok": c.order_ok,
            "max_shift_frames": c.max_shift,
            "frame_match_rate": round4(c.frame_match_rate),
        }),
    }
}

fn run_dict(r: &dt::RunSummary) -> Value {
    json!({
        "label": r.label,
        "out_dir": r.out_dir,
        "frames": r.hashes.len(),
        "unique_states": r.hashes.iter().collect::<BTreeSet<_>>().len(),
        "nonblank": r.nonblank,
        "rms_files": r.rms.keys().cloned().collect::<Vec<_>>(),
        "exceptions": r.exceptions.iter().cloned().collect::<Vec<_>>(),
        "error": r.error,
    })
}

// ───────────────────── difftest: чистые последовательности ──────────────────
#[test]
fn sequence_model_matches_python() {
    let g = golden();
    let cases = seq_cases();
    let collapse: Vec<Value> = cases
        .iter()
        .map(|(a, _)| {
            Value::Array(
                dt::collapse_states(a)
                    .into_iter()
                    .map(|(h, n)| json!([h, n]))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(g["collapse_states"], Value::Array(collapse));

    let uniq: Vec<Value> = cases.iter().map(|(a, _)| json!(dt::unique_in_order(a))).collect();
    assert_eq!(g["unique_in_order"], Value::Array(uniq));

    let fs: Vec<Value> = cases.iter().map(|(a, _)| json!(dt::first_seen(a))).collect();
    assert_eq!(g["first_seen"], Value::Array(fs));

    let tf: Vec<Value> = cases.iter().map(|(a, _)| json!(dt::transition_frames(a))).collect();
    assert_eq!(g["transition_frames"], Value::Array(tf));

    let cs: Vec<Value> = cases
        .iter()
        .map(|(a, b)| seq_value(&dt::compare_sequences(a, b)))
        .collect();
    assert_eq!(g["compare_sequences"], Value::Array(cs));

    let sd_: Vec<Value> = cases
        .iter()
        .map(|(a, b)| seq_dict(Some(&dt::compare_sequences(a, b))))
        .collect();
    assert_eq!(g["seq_dict"], Value::Array(sd_));
}

// ───────────────────────────── difftest: вердикт ────────────────────────────
#[test]
fn verdicts_match_python() {
    let g = golden();
    let long9: Vec<&str> = {
        let mut v = vec!["s"];
        v.extend(std::iter::repeat_n("m", 9));
        v
    };
    let base6: Vec<&str> = {
        let mut v: Vec<&str> = std::iter::repeat_n("s", 6).collect();
        v.extend(std::iter::repeat_n("m", 4));
        v
    };
    let reb7: Vec<&str> = {
        let mut v: Vec<&str> = std::iter::repeat_n("s", 7).collect();
        v.extend(std::iter::repeat_n("m", 3));
        v
    };
    let reb9: Vec<&str> = {
        let mut v: Vec<&str> = std::iter::repeat_n("s", 9).collect();
        v.push("m");
        v
    };
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, dt::RunSummary, Option<dt::RunSummary>, dt::RunSummary)> = vec![
        (
            "noise-ok",
            run("orig_a", &["s", "s", "m", "g"], &[("rms/x.rms", "h1")], &[], None, true),
            Some(run("orig_b", &["s", "s", "s", "m"], &[("rms/x.rms", "h1")], &[], None, true)),
            run("rebuilt", &["s", "m", "m", "g"], &[("rms/x.rms", "h1")], &[], None, true),
        ),
        (
            "states-differ",
            run("orig_a", &["s", "m"], &[], &[], None, true),
            Some(run("orig_b", &["s", "m"], &[], &[], None, true)),
            run("rebuilt", &["X", "Y"], &[], &[], None, true),
        ),
        (
            "exc-noise",
            run("orig_a", &["s"], &[], &["IOException"], None, true),
            Some(run("orig_b", &["s"], &[], &["IOException"], None, true)),
            run(
                "rebuilt",
                &["s"],
                &[],
                &["IOException", "NullPointerException"],
                None,
                true,
            ),
        ),
        (
            "blank",
            run("orig_a", &["s"], &[], &[], None, true),
            Some(run("orig_b", &["s"], &[], &[], None, true)),
            run("rebuilt", &["0"], &[], &[], None, false),
        ),
        (
            "rebuilt-error",
            run("orig_a", &["s"], &[], &[], None, true),
            Some(run("orig_b", &["s"], &[], &[], None, true)),
            run("rebuilt", &[], &[], &[], Some("emulator died"), true),
        ),
        (
            "orig-error",
            run("orig_a", &[], &[], &[], Some("boom"), true),
            Some(run("orig_b", &["s"], &[], &[], None, true)),
            run("rebuilt", &["s"], &[], &[], None, true),
        ),
        (
            "shift-widened",
            run("orig_a", &long9, &[], &[], None, true),
            Some(run("orig_b", &base6, &[], &[], None, true)),
            run("rebuilt", &reb7, &[], &[], None, true),
        ),
        (
            "shift-too-big",
            run("orig_a", &long9, &[], &[], None, true),
            Some(run("orig_b", &base6, &[], &[], None, true)),
            run("rebuilt", &reb9, &[], &[], None, true),
        ),
        (
            "rms-content-noise",
            run("orig_a", &["s"], &[("rms/x.rms", "h1")], &[], None, true),
            Some(run("orig_b", &["s"], &[("rms/x.rms", "h2")], &[], None, true)),
            run("rebuilt", &["s"], &[("rms/x.rms", "h9")], &[], None, true),
        ),
        (
            "rms-paths-differ",
            run("orig_a", &["s"], &[("rms/x.rms", "h1")], &[], None, true),
            Some(run("orig_b", &["s"], &[("rms/x.rms", "h2")], &[], None, true)),
            run("rebuilt", &["s"], &[("rms/OTHER.rms", "h1")], &[], None, true),
        ),
        (
            "rms-content-strict",
            run("orig_a", &["s"], &[("rms/x.rms", "h1")], &[], None, true),
            Some(run("orig_b", &["s"], &[("rms/x.rms", "h1")], &[], None, true)),
            run("rebuilt", &["s"], &[("rms/x.rms", "h9")], &[], None, true),
        ),
        (
            "no-baseline-pass",
            run("orig_a", &["s", "m"], &[], &[], None, true),
            None,
            run("rebuilt", &["s", "m"], &[], &[], None, true),
        ),
        (
            "no-baseline-fail",
            run("orig_a", &["s", "m"], &[], &[], None, true),
            None,
            run("rebuilt", &["X"], &[], &[], None, true),
        ),
        (
            "order-unstable-baseline",
            run("orig_a", &["s", "m", "g"], &[], &[], None, true),
            Some(run("orig_b", &["s", "g", "m"], &[], &[], None, true)),
            run("rebuilt", &["s", "g", "m"], &[], &[], None, true),
        ),
    ];

    let mut got = Vec::new();
    for (name, a, b, r) in &cases {
        let v = dt::decide(a, b.as_ref(), r, 3, 0.15);
        let mut runs = vec![run_dict(a)];
        if let Some(bb) = b {
            runs.push(run_dict(bb));
        }
        runs.push(run_dict(r));
        got.push(json!({
            "name": name,
            "passed": v.passed,
            "reasons": v.reasons,
            "notes": v.notes,
            "baseline": seq_dict(v.baseline.as_ref()),
            "cross": seq_dict(Some(&v.cross)),
            "report_md": dt::render_report(
                Path::new("/o.jar"), Path::new("/r.jar"), a, b.as_ref(), r, &v, "freej2me"),
            "report_json": json!({
                "orig_jar": "/o.jar",
                "rebuilt_jar": "/r.jar",
                "engine": "freej2me",
                "passed": v.passed,
                "reasons": v.reasons,
                "notes": v.notes,
                "baseline": seq_dict(v.baseline.as_ref()),
                "cross": seq_dict(Some(&v.cross)),
                "runs": runs,
            }),
        }));
    }
    let want = g["verdicts"].as_array().unwrap();
    assert_eq!(want.len(), got.len());
    for (w, gt) in want.iter().zip(got.iter()) {
        assert_eq!(w, gt, "вердикт {} разошёлся", gt["name"]);
    }
}

// ───────────────────────── межмоторное сравнение ────────────────────────────
#[test]
fn cross_engine_matches_python() {
    let g = golden();
    let six = ["a1", "a2", "a3", "a4", "a5", "a6"];
    let cases: Vec<(&str, dt::RunSummary, dt::RunSummary)> = vec![
        (
            "agree",
            run("freej2me", &["a1", "a1", "a2", "a3"], &[("Game/save", "h1")], &[], None, true),
            run("midp-rt", &["b1", "b1", "b2", "b3"], &[("save.rs", "hX")], &[], None, true),
        ),
        (
            "state-count",
            run("freej2me", &six, &[], &[], None, true),
            run("midp-rt", &["b1"; 6], &[], &[], None, true),
        ),
        (
            "blank",
            run("freej2me", &six, &[], &[], None, true),
            run("midp-rt", &["0"; 6], &[], &[], None, false),
        ),
        (
            "run-error",
            run("freej2me", &six, &[], &[], None, true),
            run("midp-rt", &[], &[], &[], Some("jvm died"), true),
        ),
        (
            "one-sided-exc",
            run("freej2me", &["a1"], &[], &["NullPointerException"], None, true),
            run("midp-rt", &["b1"], &[], &[], None, true),
        ),
        (
            "mangled-names",
            run("freej2me", &["a1"], &[("0a9e551506c3lzdgvt", "h")], &[], None, true),
            run("midp-rt", &["b1"], &[("system.rs", "h")], &[], None, true),
        ),
        (
            "rms-activity",
            run("freej2me", &["a1"], &[("0a9e551506c3lzdgvt", "h")], &[], None, true),
            run("midp-rt", &["b1"], &[], &[], None, true),
        ),
    ];
    let mut got = Vec::new();
    for (name, a, b) in &cases {
        let c = dt::compare_cross_engine(a, b, 3);
        got.push(json!({
            "name": name,
            "agree": c.agree,
            "reasons": c.reasons,
            "notes": c.notes,
            "states": [c.states.0, c.states.1],
            "transitions": [c.transitions.0.clone(), c.transitions.1.clone()],
            "pixel_jaccard": round4(c.pixel_jaccard),
            "report_md": dt::render_cross_report(Path::new("/g.jar"), a, b, &c),
        }));
    }
    let want = g["cross_engine"].as_array().unwrap();
    assert_eq!(want.len(), got.len());
    for (w, gt) in want.iter().zip(got.iter()) {
        assert_eq!(w, gt, "межмоторный случай {} разошёлся", gt["name"]);
    }

    let s = run(
        "e",
        &["x"],
        &[
            ("suite-Game/store one.rs", "h"),
            ("deep/тест.rms", "h"),
            ("plain", "h"),
            ("game.conf", "h"),
        ],
        &[],
        None,
        true,
    );
    assert_eq!(
        g["rms_store_names"],
        json!(dt::rms_store_names(&s).into_iter().collect::<Vec<_>>())
    );
}

// ────────────────────────── diffrun: протокол и RMS ─────────────────────────
fn ev(key: Value, action: &str) -> dr::InputEvent {
    dr::InputEvent { t_ms: 0, key, action: action.into() }
}

#[test]
fn pipe_protocol_matches_python() {
    let g = golden();
    assert_eq!(g["child_args"], json!(dr::child_args(240, 320)));

    let keys: BTreeMap<String, i64> = g["libretro_keys"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_i64().unwrap()))
        .collect();
    for (name, code) in &keys {
        assert_eq!(dr::libretro_key(name), Some(*code), "клавиша {name}");
    }

    let cmds = json!([
        dr::encode_cmd(15, 0).to_vec(),
        dr::encode_cmd(11, 4096).to_vec(),
        dr::encode_cmd(3, 18).to_vec(),
        dr::encode_cmd(2, 0).to_vec(),
        dr::encode_cmd(10, 0x01020304).to_vec(),
    ]);
    assert_eq!(g["cmd_bytes"], cmds);

    let paths = json!([
        dr::encode_path_cmd(11, "/tmp/rms/"),
        dr::encode_path_cmd(10, "/tmp/тест.jar"),
    ]);
    assert_eq!(g["path_bytes"], paths);

    let inputs = json!([
        dr::encode_input(&ev(json!("NUM5"), "tap")).unwrap(),
        dr::encode_input(&ev(json!("up"), "press")).unwrap(),
        dr::encode_input(&ev(json!(7), "release")).unwrap(),
    ]);
    assert_eq!(g["input_bytes"], inputs);
}

#[test]
fn rms_snapshot_and_stderr_scan_match_python() {
    let g = golden();
    let tmp = arlecchino::util::make_temp_dir("arl-parity-rms").unwrap();
    let rms = tmp.join("rms");
    std::fs::create_dir_all(rms.join("sub")).unwrap();
    std::fs::write(rms.join("a.rms"), b"AAA").unwrap();
    std::fs::write(rms.join("sub").join("b.rs"), b"BBB").unwrap();
    std::fs::write(rms.join("game.conf"), b"conf").unwrap();
    let before = dr::snapshot_rms(&rms);
    std::fs::write(rms.join("a.rms"), b"CCC").unwrap();
    std::fs::write(rms.join("sub").join("c.rs"), b"DDD").unwrap();
    std::fs::remove_file(rms.join("game.conf")).unwrap();
    let after = dr::snapshot_rms(&rms);
    assert_eq!(g["snapshot_rms_before"], json!(before));
    assert_eq!(g["snapshot_rms_after"], json!(after));
    assert_eq!(g["diff_rms"], json!(dr::diff_rms(&before, &after)));

    let log = tmp.join("emulator_stderr.log");
    std::fs::write(
        &log,
        "java.io.IOException: nope\n  at x\nCaused by: java.lang.NullPointerException\n\
         SomeError happened, and org.foo.BarException too\n",
    )
    .unwrap();
    assert_eq!(
        g["scan_stderr"],
        json!(dt::scan_stderr(&log).into_iter().collect::<Vec<_>>())
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ──────────────────────────── midprun: argv и манифест ──────────────────────
const MANIFESTS: [&str; 4] = [
    "Manifest-Version: 1.0\nMIDlet-1: Game, /i.png, com.x.\n Main\nMIDlet-Name: G\n",
    "MIDlet-1: Game, /i.png\n",
    "MIDlet-1: Game, /i.png, com.y.Main\n\nJunkline\nMIDlet-2: X, , z.W\n",
    "no-colon-line\nKey:   spaced   \n",
];

fn detok(v: &Value, tmp: &str) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace(tmp, "<TMP>")),
        Value::Array(a) => Value::Array(a.iter().map(|x| detok(x, tmp)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.replace(tmp, "<TMP>"), detok(x, tmp)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[test]
fn midprun_argv_and_manifest_match_python() {
    let g = golden();
    let folded: Vec<Value> = MANIFESTS.iter().map(|m| json!(mr::fold_manifest_lines(m))).collect();
    assert_eq!(g["fold_manifest"], Value::Array(folded));
    let cls: Vec<Value> = MANIFESTS
        .iter()
        .map(|m| json!(mr::midlet1_class(&mr::fold_manifest_lines(m))))
        .collect();
    assert_eq!(g["midlet1_class"], Value::Array(cls));

    let tmp = arlecchino::util::make_temp_dir("arl-parity-midprun").unwrap();
    let tmp = tmp.canonicalize().unwrap();
    let tok = tmp.to_string_lossy().into_owned();

    let jar = tmp.join("synth.jar");
    {
        let f = std::fs::File::create(&jar).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        z.start_file("META-INF/MANIFEST.MF", o).unwrap();
        std::io::Write::write_all(&mut z, MANIFESTS[0].as_bytes()).unwrap();
        z.start_file("Main.class", o).unwrap();
        std::io::Write::write_all(&mut z, &[0xca, 0xfe, 0xba, 0xbe]).unwrap();
        z.finish().unwrap();
    }
    let jad = tmp.join("synth.jad");
    std::fs::write(&jad, "MIDlet-1: Game, /i.png, com.x.Main\nMIDlet-Jar-URL: synth.jar\n").unwrap();

    let as_triple = |t: (Option<String>, Option<PathBuf>, Option<PathBuf>)| -> Value {
        json!([
            t.0,
            t.1.map(|p| p.to_string_lossy().into_owned()),
            t.2.map(|p| p.to_string_lossy().into_owned())
        ])
    };
    assert_eq!(g["autostart_jar"], detok(&as_triple(mr::autostart_target(&jar)), &tok));
    assert_eq!(g["autostart_jad"], detok(&as_triple(mr::autostart_target(&jad)), &tok));
    assert_eq!(
        g["autostart_missing"],
        detok(&as_triple(mr::autostart_target(&tmp.join("nope.jar"))), &tok)
    );

    let engines = tmp.join("engines");
    std::fs::create_dir_all(&engines).unwrap();
    let fake_rt = engines.join("midp-rt.jar");
    std::fs::write(&fake_rt, b"x").unwrap();
    std::fs::write(engines.join("asm-9.7.jar"), b"x").unwrap();
    assert_eq!(
        g["runtime_classpath"],
        detok(
            &json!(mr::runtime_classpath(&fake_rt)
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>()),
            &tok
        )
    );

    let argv = mr::build_run_argv(
        &jar,
        &fake_rt,
        &mr::WindowOpts {
            width: Some(240),
            height: Some(320),
            rms_dir: Some(tmp.join("rmsdir")),
            instrument: true,
            trace: Some(tmp.join("trace.txt")),
        },
    );
    assert_eq!(g["build_run_argv"], detok(&json!(argv), &tok));

    let argv2 = mr::build_run_argv(
        &tmp.join("nope.jar"),
        &fake_rt,
        &mr::WindowOpts { instrument: false, ..Default::default() },
    );
    assert_eq!(g["build_run_argv_launcher"], detok(&json!(argv2), &tok));

    let argv3 = mr::build_headless_argv(
        &jar,
        &fake_rt,
        &mr::HeadlessArgvOpts {
            frames_dir: tmp.join("frames"),
            frames: 12,
            period_ms: 50,
            boot_wait_ms: 1500,
            inputs_file: Some(tmp.join("in.json")),
            rms_dir: Some(tmp.join("rmsdir")),
            width: Some(240),
            height: Some(320),
            instrument: true,
            home_dir: Some(tmp.join("home")),
        },
    );
    assert_eq!(g["build_headless_argv"], detok(&json!(argv3), &tok));
    let _ = std::fs::remove_dir_all(&tmp);
}

// ──────────────────────────────── semdiff ───────────────────────────────────
fn refs_map(m: &BTreeMap<String, BTreeSet<sd::Ref>>) -> Value {
    let mut o = serde_json::Map::new();
    for (k, v) in m {
        let mut items: Vec<String> = v.iter().map(sd::ref_repr).collect();
        items.sort();
        o.insert(k.clone(), json!(items));
    }
    Value::Object(o)
}

#[test]
fn semdiff_matches_python() {
    let g = golden();
    let rev = sd::load_reverse_map(Some(&golden_dir().join("mapping.txt")));
    assert_eq!(g["reverse_map"], json!(rev));

    let orig_txt = std::fs::read_to_string(golden_dir().join("javap_orig.txt")).unwrap();
    let rec_txt = std::fs::read_to_string(golden_dir().join("javap_recompiled.txt")).unwrap();
    let a = sd::parse_javap(&orig_txt, &rev);
    let b = sd::parse_javap(&rec_txt, &rev);
    assert_eq!(g["parse_javap_orig"], refs_map(&a));
    assert_eq!(g["parse_javap_recompiled"], refs_map(&b));
    let empty = BTreeMap::new();
    assert_eq!(
        g["parse_javap_orig_nomap"],
        refs_map(&sd::parse_javap(&orig_txt, &empty))
    );

    let res = sd::compare_maps("Fixture", &a, &b);
    assert_eq!(g["compare_class"], serde_json::to_value(&res).unwrap());

    let canon: [&str; 8] = [
        " Field aa.H:I",
        " Method j.foo:(I)V",
        " InterfaceMethod x/Y.go:(I)I",
        " class java/lang/StringBuffer",
        " class \"[Ljava/lang/String;\"",
        " Method \"<init>\":()V",
        " Field a:Ljava/lang/StringBuffer;",
        " InvokeDynamic #0:makeConcat",
    ];
    let as_list = |r: sd::Ref| -> Value {
        if r.0 == "Type" || r.0 == "NewArray" || r.0 == "?" {
            json!([r.0, r.1])
        } else {
            json!([r.0, r.1, r.2])
        }
    };
    let got: Vec<Value> = canon.iter().map(|c| as_list(sd::canon_ref(c, &rev))).collect();
    assert_eq!(g["canon_ref"], Value::Array(got));
    let got2: Vec<Value> = canon.iter().map(|c| as_list(sd::canon_ref(c, &empty))).collect();
    assert_eq!(g["canon_ref_nomap"], Value::Array(got2));

    let sigs = [
        "  public final int mix(int);",
        "private static void a(java.lang.String);",
        "  protected abstract synchronized  java.lang.String  b_af64044c(int, int);",
        "public Fixture();",
    ];
    let ns: Vec<Value> = sigs.iter().map(|s| json!(sd::normsig(s, &rev))).collect();
    assert_eq!(g["normsig"], Value::Array(ns));
    let ns2: Vec<Value> = sigs.iter().map(|s| json!(sd::normsig(s, &empty))).collect();
    assert_eq!(g["normsig_nomap"], Value::Array(ns2));
}

// ───────────── сознательные отступления от Python (починенные баги) ──────────
#[test]
fn normsig_does_not_chain_renames() {
    // Баг Python: цикл по словарю применял замены поверх друг друга, поэтому
    // `a_x -> a` и `a -> b` давали `a_x -> b`, а результат зависел от порядка
    // строк в mapping.txt. Здесь каждое имя заменяется ровно один раз.
    let mut rev = BTreeMap::new();
    rev.insert("a_x".to_string(), "a".to_string());
    rev.insert("a".to_string(), "b".to_string());
    assert_eq!(sd::normsig("void a_x(int)", &rev), "void a(int)");
    assert_eq!(sd::normsig("void a(int)", &rev), "void b(int)");
}

#[test]
fn canon_ref_without_descriptor_is_not_mangled() {
    // Баг Python: без ':' в операнде `rfind` давал −1, у имени отгрызался
    // последний символ, а дескриптором становилась вся строка.
    let empty = BTreeMap::new();
    assert_eq!(
        sd::canon_ref(" Method owner.name", &empty),
        ("Method".into(), "name".into(), String::new())
    );
}
