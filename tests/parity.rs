//! Parity tests: expected values captured from the original Python implementation
//! (arlecchino.autofix / normalize / deobf) on the same inputs.

use arlecchino::{autofix, deobf, normalize};

#[test]
fn normalize_jadx_clean_text_matches_python() {
    let dir = std::env::temp_dir().join(format!("arl-norm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let jadx = "package defpackage;\nimport defpackage.b;\n/* JADX INFO: Added by JADX */\nclass a extends b {\n    defpackage.b field;\n}\n";
    let f = dir.join("a.java");
    std::fs::write(&f, jadx).unwrap();
    normalize::normalize_jadx(&dir, true);
    let out = std::fs::read_to_string(&f).unwrap();
    assert_eq!(out, "class a extends b {\n    b field;\n}\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fold_interface_static_init_matches_python() {
    let iface = "public interface o {\n    public static final int a;\n    public static final String b;\n    static {\n        a = 5;\n        b = \"hi\";\n        int junk = 3;\n    }\n}\n";
    let expect = "public interface o {\n    public static final int a = 5;\n    public static final String b = \"hi\";\n    \n}\n";
    assert_eq!(autofix::fold_interface_static_init(iface), expect);
}

#[test]
fn const_interface_to_class_matches_python() {
    let iface2 = "public interface o {\n    public static final int a;\n    public static final int b;\n    static {\n        a = 5;\n    }\n}\n";
    let expect = "public final class o {\n    public static final int a;\n    public static final int b;\n    static {\n        a = 5;\n    }\n}\n";
    assert_eq!(autofix::const_interface_to_class(iface2), expect);
}

#[test]
fn illegal_static_local_matches_python() {
    let sl = "class X {\n    void m() {\n            static int y = 3;\n    }\n}\n";
    let expect = "class X {\n    void m() {\n            int y = 3;\n    }\n}\n";
    assert_eq!(autofix::illegal_static_local(sl), expect);
}

#[test]
fn duplicate_imports_matches_python() {
    let di = "import a.b.C;\nimport a.b.C;\nimport d.E;\nimport a.b.C;\nclass X{}\n";
    let expect = "import a.b.C;\nimport d.E;\nclass X{}\n";
    assert_eq!(autofix::duplicate_imports(di), expect);
}

#[test]
fn scratch_temp_dead_array_type_matches_python() {
    let st = "void m(){\n    int[] var10000 = new char[]{' ', 'x'};\n    var10000 = new int[]{0, 2, 3};\n}\n";
    let expect = "void m(){\n    Object var10000 = new char[]{' ', 'x'};\n    var10000 = new int[]{0, 2, 3};\n}\n";
    assert_eq!(autofix::scratch_temp_dead_array_type(st), expect);
}

#[test]
fn resolve_unrepresentable_casts_matches_python() {
    let line = "        Object x = (<unrepresentable>)var2.nextElement()).a_af64174c;\n";
    let donor = "        Object x = (Foo)elements.nextElement()).a_af64174c;\n".to_string();
    let expect = "        Object x = (Foo)var2.nextElement()).a_af64174c;\n";
    assert_eq!(autofix::resolve_unrepresentable_casts(line, &[donor]), expect);
}

#[test]
fn apply_all_matches_python() {
    let iface = "public interface o {\n    public static final int a;\n    public static final String b;\n    static {\n        a = 5;\n        b = \"hi\";\n        int junk = 3;\n    }\n}\n";
    let (text, applied) = autofix::apply_all(iface);
    assert_eq!(
        text,
        "public interface o {\n    public static final int a = 5;\n    public static final String b = \"hi\";\n    \n}\n"
    );
    assert_eq!(applied, vec!["fold_interface_static_init".to_string()]);
}

#[test]
fn deobf_classify_matches_python() {
    assert_eq!(deobf::classify("class a extends MIDlet {}"), Some("MIDlet"));
    assert_eq!(deobf::classify("class a implements Runnable {}"), Some("Worker"));
    assert_eq!(deobf::classify("class a {}"), None);
}
