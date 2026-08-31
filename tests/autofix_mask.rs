//! Regression net — autofix robustness to strings / chars / comments / generics.
//!
//! Ported 1:1 from the reference implementation's `tests/test_autofix_mask.py`.
//!
//! The old `brace_block` / regex fixes scanned the RAW source, so a `{`, `}`, `;` (or
//! the keyword text) hiding inside a string literal, a char literal, a line comment or
//! a block comment was mis-counted, mis-matching block boundaries and mis-firing
//! fixes.  These tests pin the masked behaviour: structural scanning runs on a
//! length-preserving masked view, so literals/comments can't move a block boundary or
//! trigger a spurious edit, while genuine code is rewritten exactly as before.
//!
//! Pure stdlib — no javac / engines / network, so this always runs in bare CI.

use arlecchino::autofix as a;

// ───────────────────────── length-preserving mask ───────────────────────────

#[test]
fn mask_is_length_preserving() {
    let src = "class C {\n\
               \x20   String a = \"}; static { }\";   // text\n\
               \x20   char c = '}';\n\
               \x20   /* block } ; { comment */\n\
               \x20   int x = 1;\n\
               }\n";
    let masked = a::mask_literals(src);
    assert_eq!(masked.len(), src.len());
    // line boundaries are identical (newlines are never blanked)
    assert_eq!(
        masked.matches('\n').count(),
        src.matches('\n').count(),
        "newline count must be preserved"
    );
    assert_eq!(masked.lines().count(), src.lines().count());
}

#[test]
fn mask_blanks_literal_and_comment_contents() {
    let src = "x = \"a};b\"; // }static {\n/* } ; */ int y;";
    let masked = a::mask_literals(src);
    // nothing structural survives inside the string, the line comment or the block
    // comment (everything that isn't genuine code is spaces).
    let q1 = masked.find('"').unwrap();
    let q2 = masked[q1 + 1..].find('"').unwrap() + q1 + 1;
    assert_eq!(masked[q1 + 1..q2].trim(), "", "string content must be blanked");
    // the real trailing `;` and the genuine `int y;` are still present
    assert!(masked.trim_end().ends_with("int y;"), "masked = {masked:?}");
    // no stray brace/semicolon leaks from the comments
    let after_real_semi = masked.split_once(';').unwrap().1;
    let first_line = after_real_semi.split('\n').next().unwrap();
    assert!(!first_line.contains("static"));
}

#[test]
fn mask_handles_escapes() {
    // an escaped quote must NOT terminate the string early
    let src = r#"String s = "a\"};b\\"; int x;"#;
    let masked = a::mask_literals(src);
    assert_eq!(masked.len(), src.len());
    // the genuine `; int x;` after the string survives; no `}`/`{` leaks
    assert!(masked.trim_end().ends_with("int x;"), "masked = {masked:?}");
    assert!(!masked.contains('}')); // the only brace was inside the (now-blanked) string
}

// ───────────────────── brace-matcher block boundaries ────────────────────────

/// The pre-mask RAW brace matcher (used only to PROVE the old misfire).
fn old_brace_block(text: &str, start_kw: &str) -> Option<(usize, usize, usize)> {
    let si = text.find(start_kw)?;
    let i = text[si..].find('{').map(|p| si + p)?;
    let bytes = text.as_bytes();
    let mut depth: i32 = 0;
    let mut j = i;
    while j < bytes.len() {
        match bytes[j] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((si, i, j));
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

#[test]
fn brace_block_ignores_close_brace_in_string() {
    let src = "    static {\n        String s = \"}\";\n        int x = 1;\n    }\n";
    let old = old_brace_block(src, "static {").unwrap();
    let new = a::brace_block(src, "static {").unwrap();
    // OLD misfires: it stops at the `}` inside the string literal, so the body is
    // truncated and never reaches the real statements.
    assert!(!src[old.1..old.2 + 1].contains("int x = 1"));
    // NEW finds the real closing brace.
    assert!(src[new.1..new.2 + 1].contains("int x = 1"));
}

#[test]
fn brace_block_ignores_braces_in_comment() {
    let src = "    static {\n\
               \x20       // closing here } would fool the old matcher\n\
               \x20       /* and { } here too */\n\
               \x20       int y = 2;\n\
               \x20   }\n";
    let old = old_brace_block(src, "static {").unwrap();
    let new = a::brace_block(src, "static {").unwrap();
    assert!(!src[old.1..old.2 + 1].contains("int y = 2"));
    assert!(src[new.1..new.2 + 1].contains("int y = 2"));
}

#[test]
fn brace_block_ignores_start_kw_in_string() {
    // a `static {` token inside a string must not be treated as the block start.
    let src = "class C {\n\
               \x20   String log = \"entered static { scope\";\n\
               \x20   static { int real = 1; }\n\
               }\n";
    let new = a::brace_block(src, "static {").expect("real static block must be found");
    // the matched body must be the REAL static block, not the string token.
    assert!(src[new.1..new.2 + 1].contains("int real = 1"));
}

// ───────────────────── end-to-end fixes: no misfire, real fire ───────────────

#[test]
fn fold_does_not_fire_on_static_block_in_string() {
    // NOT a constant interface — the only `static {` is inside a string literal.
    let src = "public interface X {\n\
               \x20   String DOC = \"use a static { } block\";\n\
               \x20   int A = 1;\n\
               }\n";
    // neither fix should mistake the in-string `static {` for a real one.
    assert_eq!(a::fold_interface_static_init(src), src);
    assert_eq!(a::const_interface_to_class(src), src);
}

#[test]
fn fold_still_fires_on_genuine_constant_interface() {
    let src = "public interface K {\n\
               \x20   public static final int A;\n\
               \x20   public static final int B;\n\
               \x20   static {\n\
               \x20       A = 1;   // comment with } and ; to bait the matcher\n\
               \x20       B = 2;\n\
               \x20   }\n\
               }\n";
    let out = a::fold_interface_static_init(src);
    assert_ne!(out, src);
    assert!(out.contains("public static final int A = 1;"), "out = {out:?}");
    assert!(out.contains("public static final int B = 2;"), "out = {out:?}");
    // the static block must be fully removed (its real closing brace was found
    // despite the `}` in the comment).
    assert!(!out.contains("static {"));
    // body integrity: still a single well-formed interface (balanced braces).
    assert_eq!(out.matches('{').count(), out.matches('}').count());
}

#[test]
fn illegal_static_local_skips_static_in_string() {
    // a `static ` token buried in a deeply-indented STRING must not be stripped.
    let src = "class C {\n\
               \x20   void m() {\n\
               \x20       String s = \"        static thing\";\n\
               \x20       int n = 0;\n\
               \x20   }\n\
               }\n";
    assert_eq!(a::illegal_static_local(src), src); // nothing genuine to fix -> unchanged
}

#[test]
fn illegal_static_local_still_strips_real_local_static() {
    let src = "class C {\n\
               \x20   void m() {\n\
               \x20       if (true) {\n\
               \x20           static int bad = 1;\n\
               \x20       }\n\
               \x20   }\n\
               }\n";
    let out = a::illegal_static_local(src);
    assert!(!out.contains("static int bad"), "out = {out:?}");
    assert!(out.contains("int bad = 1;"), "out = {out:?}");
}

#[test]
fn scratch_dead_array_ignores_generic_lookalike_in_string() {
    // a generic + brace-looking fragment inside a string must not be touched.
    let src = "void f() {\n\
               \x20   String t = \"Map<String, List<Integer>> x = new char[]{...\";\n\
               \x20   int y = 0;\n\
               }\n";
    assert_eq!(a::scratch_temp_dead_array_type(src), src);
}

#[test]
fn scratch_dead_array_still_fires_on_real_dead_store() {
    let src = "void f() {\n\
               \x20   int[] var10000 = new char[]{' ', 'x'};\n\
               \x20   var10000 = new int[]{0, 2, 3, 4};\n\
               }\n";
    let out = a::scratch_temp_dead_array_type(src);
    assert_ne!(out, src);
    assert!(out.contains("Object var10000 = new char[]"), "out = {out:?}");
}

#[test]
fn generic_nested_does_not_break_brace_matching() {
    // `Map<String, List<X>>` contains no braces, but its `>>` / `<` must not perturb
    // the brace matcher; the real block is found normally.
    let src = "class C {\n\
               \x20   static {\n\
               \x20       Map<String, List<Integer>> m = build();\n\
               \x20       int z = 1;\n\
               \x20   }\n\
               }\n";
    let new = a::brace_block(src, "static {").unwrap();
    assert!(src[new.1..new.2 + 1].contains("int z = 1"));
    assert_eq!(&src[new.2..new.2 + 1], "}"); // ends on a real closing brace
}

#[test]
fn apply_all_unchanged_on_clean_code_with_literals() {
    // genuine clean code that happens to contain literals/comments must round-trip
    // unchanged (no fix should mis-fire).
    let src = "public class C {\n\
               \x20   String s = \"}; static { } ;\";  // bait } { ;\n\
               \x20   int f() { return 1; }\n\
               }\n";
    let (out, applied) = a::apply_all(src);
    assert_eq!(out, src);
    assert!(applied.is_empty(), "applied = {applied:?}");
}
