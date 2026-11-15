// Dumps the javac-attributed AST of a Java source tree as JSON, with resolved types
// and symbols, for the Rust generator of `arlecchino port-rust` (src/portrust/gen.rs).
// Original work (Apache-2.0); it uses only the JDK's own compiler API.
//
// usage: java --add-exports jdk.compiler/com.sun.tools.javac.{code,tree,util,api}=ALL-UNNAMED \
//             JavacAstDump <classpath> <out.json> <file.java>...
import com.sun.source.util.JavacTask;
import com.sun.source.util.SourcePositions;
import com.sun.source.util.Trees;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.tools.javac.code.Symbol;
import com.sun.tools.javac.code.Type;
import com.sun.tools.javac.code.Types;
import com.sun.tools.javac.code.Flags;
import com.sun.tools.javac.tree.JCTree;
import com.sun.tools.javac.tree.JCTree.*;
import com.sun.tools.javac.tree.TreeInfo;
import com.sun.tools.javac.util.Context;
import com.sun.tools.javac.api.BasicJavacTask;

import javax.tools.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.*;

public class JavacAstDump {
    static StringBuilder out = new StringBuilder();
    static Types types;
    static SourcePositions pos;
    static CompilationUnitTree curCu;
    static String curSrc;
    static IdentityHashMap<JCTree, Integer> targets = new IdentityHashMap<>();
    static int nextTarget = 1;

    public static void main(String[] a) throws Exception {
        JavaCompiler jc = ToolProvider.getSystemJavaCompiler();
        StandardJavaFileManager fm = jc.getStandardFileManager(null, null, StandardCharsets.UTF_8);
        List<File> files = new ArrayList<>();
        for (int i = 2; i < a.length; i++) files.add(new File(a[i]));
        List<String> opts = Arrays.asList("-cp", a[0], "-proc:none", "-nowarn", "-encoding", "UTF-8");
        JavacTask task = (JavacTask) jc.getTask(null, fm, null, opts, null, fm.getJavaFileObjectsFromFiles(files));
        Iterable<? extends CompilationUnitTree> cus = task.parse();
        task.analyze();
        Context ctx = ((BasicJavacTask) task).getContext();
        types = Types.instance(ctx);
        pos = Trees.instance(task).getSourcePositions();
        out.append("{\"classes\":[");
        boolean first = true;
        for (CompilationUnitTree cu : cus) {
            curCu = cu;
            curSrc = cu.getSourceFile().getCharContent(true).toString();
            for (JCTree t : ((JCCompilationUnit) cu).defs) {
                if (t instanceof JCClassDecl) {
                    if (!first) out.append(',');
                    first = false;
                    classDecl((JCClassDecl) t);
                }
            }
        }
        out.append("]}");
        Files.write(Paths.get(a[1]), out.toString().getBytes(StandardCharsets.UTF_8));
    }

    // ---- json helpers ---------------------------------------------------
    static void str(String s) {
        out.append('"');
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"': out.append("\\\""); break;
                case '\\': out.append("\\\\"); break;
                case '\n': out.append("\\n"); break;
                case '\r': out.append("\\r"); break;
                case '\t': out.append("\\t"); break;
                default:
                    if (c < 0x20 || c > 0x7e) out.append(String.format("\\u%04x", (int) c));
                    else out.append(c);
            }
        }
        out.append('"');
    }
    static void key(String k) { out.append('"').append(k).append("\":"); }
    static void kv(String k, String v) { key(k); if (v == null) out.append("null"); else str(v); out.append(','); }
    static void kb(String k, boolean v) { key(k); out.append(v); out.append(','); }
    static void ki(String k, long v) { key(k); out.append(v); out.append(','); }
    static void end() { // drop trailing comma, close object
        if (out.charAt(out.length() - 1) == ',') out.setLength(out.length() - 1);
        out.append('}');
    }

    static String ty(Type t) {
        if (t == null) return null;
        Type e = types.erasure(t);
        return e.toString();
    }
    static String src(JCTree t) {
        long s = pos.getStartPosition(curCu, t), e = pos.getEndPosition(curCu, t);
        if (s < 0 || e < 0 || e > curSrc.length()) return "?";
        String r = curSrc.substring((int) s, (int) e);
        return r.length() > 400 ? r.substring(0, 400) + "…" : r;
    }
    static long line(JCTree t) {
        return curCu.getLineMap().getLineNumber(pos.getStartPosition(curCu, t));
    }

    // ---- declarations ---------------------------------------------------
    static void classDecl(JCClassDecl c) {
        out.append('{');
        Symbol.ClassSymbol cs = c.sym;
        kv("name", cs.flatName().toString());
        kv("super", cs.getSuperclass() == null ? null : ty(cs.getSuperclass()));
        key("ifaces"); out.append('[');
        boolean f = true;
        for (Type i : cs.getInterfaces()) { if (!f) out.append(','); f = false; str(ty(i)); }
        out.append("],");
        kb("interface", (cs.flags() & Flags.INTERFACE) != 0);
        kb("abstract", (cs.flags() & Flags.ABSTRACT) != 0);
        key("members"); out.append('[');
        f = true;
        for (JCTree m : c.defs) {
            if (!f) out.append(',');
            f = false;
            if (m instanceof JCVariableDecl) {
                JCVariableDecl v = (JCVariableDecl) m;
                out.append('{'); kv("k", "field"); kv("name", v.name.toString()); kv("t", ty(v.sym.type));
                kb("static", (v.sym.flags() & Flags.STATIC) != 0);
                kb("final", (v.sym.flags() & Flags.FINAL) != 0);
                ki("line", line(v));
                if (v.init != null) { key("init"); expr(v.init); out.append(','); }
                end();
            } else if (m instanceof JCMethodDecl) {
                JCMethodDecl md = (JCMethodDecl) m;
                out.append('{'); kv("k", "method"); kv("name", md.name.toString());
                kv("desc", md.sym.type.toString());
                kb("static", (md.sym.flags() & Flags.STATIC) != 0);
                kb("abstract", (md.sym.flags() & Flags.ABSTRACT) != 0);
                kb("synchronized", (md.sym.flags() & Flags.SYNCHRONIZED) != 0);
                kv("ret", md.restype == null ? "void" : ty(md.restype.type));
                ki("line", line(md));
                key("params"); out.append('[');
                boolean pf = true;
                for (JCVariableDecl p : md.params) {
                    if (!pf) out.append(',');
                    pf = false;
                    out.append('{'); kv("name", p.name.toString()); kv("t", ty(p.sym.type)); end();
                }
                out.append("],");
                if (md.body != null) { key("body"); stmt(md.body); out.append(','); }
                end();
            } else if (m instanceof JCBlock) {
                JCBlock b = (JCBlock) m;
                out.append('{'); kv("k", "init"); kb("static", (b.flags & Flags.STATIC) != 0);
                key("body"); stmt(b); out.append(','); end();
            } else if (m instanceof JCClassDecl) {
                out.append('{'); kv("k", "nested"); kv("src", "nested class " + ((JCClassDecl) m).name); end();
            } else {
                out.append('{'); kv("k", "unknown"); kv("src", src(m)); end();
            }
        }
        out.append("],");
        end();
    }

    // ---- statements -----------------------------------------------------
    static int tid(JCTree t) {
        Integer i = targets.get(t);
        if (i == null) { i = nextTarget++; targets.put(t, i); }
        return i;
    }

    static void stmts(List<? extends JCTree> l) {
        out.append('[');
        boolean f = true;
        for (JCTree s : l) { if (!f) out.append(','); f = false; stmt(s); }
        out.append(']');
    }

    static void stmt(JCTree s) {
        out.append('{');
        if (s instanceof JCBlock) {
            kv("k", "block"); key("stmts"); stmts(((JCBlock) s).stats); out.append(',');
        } else if (s instanceof JCVariableDecl) {
            JCVariableDecl v = (JCVariableDecl) s;
            kv("k", "local"); kv("name", v.name.toString()); kv("t", ty(v.sym.type));
            if (v.init != null) { key("init"); expr(v.init); out.append(','); }
        } else if (s instanceof JCExpressionStatement) {
            kv("k", "expr"); key("e"); expr(((JCExpressionStatement) s).expr); out.append(',');
        } else if (s instanceof JCIf) {
            JCIf i = (JCIf) s;
            kv("k", "if"); key("cond"); expr(i.cond); out.append(',');
            key("then"); stmt(i.thenpart); out.append(',');
            if (i.elsepart != null) { key("else"); stmt(i.elsepart); out.append(','); }
        } else if (s instanceof JCWhileLoop) {
            JCWhileLoop w = (JCWhileLoop) s;
            kv("k", "while"); ki("id", tid(w)); key("cond"); expr(w.cond); out.append(',');
            key("body"); stmt(w.body); out.append(',');
        } else if (s instanceof JCDoWhileLoop) {
            JCDoWhileLoop w = (JCDoWhileLoop) s;
            kv("k", "do"); ki("id", tid(w)); key("cond"); expr(w.cond); out.append(',');
            key("body"); stmt(w.body); out.append(',');
        } else if (s instanceof JCForLoop) {
            JCForLoop w = (JCForLoop) s;
            kv("k", "for"); ki("id", tid(w));
            key("init"); stmts(w.init); out.append(',');
            if (w.cond != null) { key("cond"); expr(w.cond); out.append(','); }
            key("update"); stmts(w.step); out.append(',');
            key("body"); stmt(w.body); out.append(',');
        } else if (s instanceof JCLabeledStatement) {
            JCLabeledStatement l = (JCLabeledStatement) s;
            int lid = tid(l.body); targets.put(l, lid);
            kv("k", "labeled"); ki("id", lid); key("body"); stmt(l.body); out.append(',');
        } else if (s instanceof JCSwitch) {
            JCSwitch w = (JCSwitch) s;
            kv("k", "switch"); ki("id", tid(w)); key("sel"); expr(w.selector); out.append(',');
            key("cases"); out.append('[');
            boolean f = true;
            for (JCCase c : w.cases) {
                if (!f) out.append(',');
                f = false;
                out.append('{');
                List<JCExpression> labels = new ArrayList<>();
                for (JCTree l : c.labels) if (l instanceof JCExpression) labels.add((JCExpression) l);
                kb("default", labels.isEmpty());
                key("labels"); out.append('[');
                boolean lf = true;
                for (JCExpression l : labels) { if (!lf) out.append(','); lf = false; expr(l); }
                out.append("],");
                key("stmts"); stmts(c.stats); out.append(',');
                end();
            }
            out.append("],");
        } else if (s instanceof JCBreak) {
            JCBreak b = (JCBreak) s;
            kv("k", "break"); ki("target", b.target == null ? 0 : tid(b.target));
        } else if (s instanceof JCContinue) {
            JCContinue b = (JCContinue) s;
            kv("k", "continue"); ki("target", b.target == null ? 0 : tid(b.target));
        } else if (s instanceof JCReturn) {
            JCReturn r = (JCReturn) s;
            kv("k", "return");
            if (r.expr != null) { key("e"); expr(r.expr); out.append(','); }
        } else if (s instanceof JCThrow) {
            kv("k", "throw"); key("e"); expr(((JCThrow) s).expr); out.append(',');
        } else if (s instanceof JCTry) {
            JCTry t = (JCTry) s;
            kv("k", "try"); key("body"); stmt(t.body); out.append(',');
            key("catches"); out.append('[');
            boolean f = true;
            for (JCCatch c : t.catchers) {
                if (!f) out.append(',');
                f = false;
                out.append('{'); kv("name", c.param.name.toString()); kv("t", ty(c.param.sym.type));
                key("body"); stmt(c.body); out.append(','); end();
            }
            out.append("],");
            if (t.finalizer != null) { key("finally"); stmt(t.finalizer); out.append(','); }
        } else if (s instanceof JCSynchronized) {
            JCSynchronized y = (JCSynchronized) s;
            kv("k", "sync"); key("lock"); expr(y.lock); out.append(','); key("body"); stmt(y.body); out.append(',');
        } else if (s instanceof JCSkip) {
            kv("k", "empty");
        } else {
            kv("k", "unknown");
        }
        ki("line", line(s));
        kv("src", s instanceof JCBlock ? "{…}" : src(s));
        end();
    }

    // ---- expressions ----------------------------------------------------
    static void exprs(List<? extends JCExpression> l) {
        out.append('[');
        boolean f = true;
        for (JCExpression e : l) { if (!f) out.append(','); f = false; expr(e); }
        out.append(']');
    }

    static void constant(Object v, Type t) {
        kv("k", "lit");
        String tn = ty(t);
        if (v instanceof String) { kv("kind", "string"); kv("v", (String) v); }
        else if (tn.equals("char")) { kv("kind", "char"); ki("v", ((Number) v).intValue()); }
        else if (tn.equals("boolean")) { kv("kind", "bool"); kb("v", ((Number) v).intValue() != 0); }
        else if (tn.equals("float") || tn.equals("double")) {
            kv("kind", tn); kv("v", Double.toString(((Number) v).doubleValue()));
            kv("bits", tn.equals("float") ? Integer.toString(Float.floatToRawIntBits(((Number) v).floatValue()))
                                            : Long.toString(Double.doubleToRawLongBits(((Number) v).doubleValue())));
        }
        else { kv("kind", tn); ki("v", ((Number) v).longValue()); }
    }

    static void symInfo(Symbol sym) {
        if (sym == null) { kv("sym", "none"); return; }
        if (sym instanceof Symbol.VarSymbol) {
            Symbol.VarSymbol v = (Symbol.VarSymbol) sym;
            if (v.owner instanceof Symbol.ClassSymbol) {
                kv("sym", "field"); kv("owner", ((Symbol.ClassSymbol) v.owner).flatName().toString());
                kb("static", (v.flags() & Flags.STATIC) != 0);
            } else {
                kv("sym", "local");
            }
        } else if (sym instanceof Symbol.ClassSymbol) {
            kv("sym", "type"); kv("owner", ((Symbol.ClassSymbol) sym).flatName().toString());
        } else if (sym instanceof Symbol.MethodSymbol) {
            kv("sym", "method");
        } else {
            kv("sym", sym.kind.toString());
        }
    }

    static void expr(JCExpression e) {
        out.append('{');
        Type t = e.type;
        Object cv = t == null ? null : t.constValue();
        if (cv != null && !(e instanceof JCAssign) && !(e instanceof JCAssignOp)) {
            constant(cv, t);
        } else if (e instanceof JCParens) {
            out.setLength(out.length() - 1);
            expr(((JCParens) e).expr);
            return;
        } else if (e instanceof JCLiteral) {
            JCLiteral l = (JCLiteral) e;
            if (l.value == null) kv("k", "null");
            else constant(l.value, t);
        } else if (e instanceof JCIdent) {
            JCIdent i = (JCIdent) e;
            kv("k", "ident"); kv("name", i.name.toString()); symInfo(i.sym);
        } else if (e instanceof JCFieldAccess) {
            JCFieldAccess f = (JCFieldAccess) e;
            kv("k", "select"); kv("name", f.name.toString()); symInfo(f.sym);
            key("target"); expr(f.selected); out.append(',');
        } else if (e instanceof JCMethodInvocation) {
            JCMethodInvocation m = (JCMethodInvocation) e;
            Symbol s = TreeInfo.symbol(m.meth);
            kv("k", "call");
            if (s instanceof Symbol.MethodSymbol) {
                Symbol.MethodSymbol ms = (Symbol.MethodSymbol) s;
                kv("name", ms.name.toString());
                kv("owner", ((Symbol.ClassSymbol) ms.owner).flatName().toString());
                kb("static", (ms.flags() & Flags.STATIC) != 0);
                kv("desc", types.erasure(ms.type).toString());
                key("ptypes"); out.append('[');
                boolean f = true;
                for (Type p : ms.type.getParameterTypes()) { if (!f) out.append(','); f = false; str(ty(p)); }
                out.append("],");
            }
            if (m.meth instanceof JCFieldAccess) {
                JCFieldAccess fa = (JCFieldAccess) m.meth;
                key("recv"); expr(fa.selected); out.append(',');
            } else {
                JCIdent id = (JCIdent) m.meth;
                kv("implicit", id.name.toString()); // this(...)/super(...)/plain call
            }
            key("args"); exprs(m.args); out.append(',');
        } else if (e instanceof JCNewClass) {
            JCNewClass n = (JCNewClass) e;
            kv("k", "new");
            kv("class", ty(n.type));
            if (n.constructor != null) {
                key("ptypes"); out.append('[');
                boolean f = true;
                for (Type p : n.constructor.type.getParameterTypes()) { if (!f) out.append(','); f = false; str(ty(p)); }
                out.append("],");
            }
            kb("anon", n.def != null);
            key("args"); exprs(n.args); out.append(',');
        } else if (e instanceof JCNewArray) {
            JCNewArray n = (JCNewArray) e;
            kv("k", "newarray");
            key("dims"); exprs(n.dims); out.append(',');
            if (n.elems != null) { key("elems"); exprs(n.elems); out.append(','); }
        } else if (e instanceof JCBinary) {
            JCBinary b = (JCBinary) e;
            kv("k", "bin"); kv("op", b.getTag().toString());
            Type ot = b.operator.type;
            kv("lt", ty(ot.getParameterTypes().get(0)));
            kv("rt", ty(ot.getParameterTypes().get(1)));
            key("l"); expr(b.lhs); out.append(',');
            key("r"); expr(b.rhs); out.append(',');
        } else if (e instanceof JCUnary) {
            JCUnary u = (JCUnary) e;
            kv("k", "unary"); kv("op", u.getTag().toString());
            if (u.operator != null) kv("ot", ty(u.operator.type.getParameterTypes().get(0)));
            key("e"); expr(u.arg); out.append(',');
        } else if (e instanceof JCAssign) {
            JCAssign a = (JCAssign) e;
            kv("k", "assign"); key("l"); expr(a.lhs); out.append(','); key("r"); expr(a.rhs); out.append(',');
        } else if (e instanceof JCAssignOp) {
            JCAssignOp a = (JCAssignOp) e;
            kv("k", "assignop"); kv("op", a.getTag().toString());
            Type ot = a.operator.type;
            kv("lt", ty(ot.getParameterTypes().get(0)));
            kv("rt", ty(ot.getParameterTypes().get(1)));
            kv("res", ty(ot.getReturnType()));
            key("l"); expr(a.lhs); out.append(','); key("r"); expr(a.rhs); out.append(',');
        } else if (e instanceof JCConditional) {
            JCConditional c = (JCConditional) e;
            kv("k", "cond"); key("c"); expr(c.cond); out.append(',');
            key("a"); expr(c.truepart); out.append(','); key("b"); expr(c.falsepart); out.append(',');
        } else if (e instanceof JCTypeCast) {
            JCTypeCast c = (JCTypeCast) e;
            kv("k", "cast"); key("e"); expr(c.expr); out.append(',');
        } else if (e instanceof JCInstanceOf) {
            JCInstanceOf c = (JCInstanceOf) e;
            kv("k", "instanceof"); kv("class", ty(c.pattern.type)); key("e"); expr(c.expr); out.append(',');
        } else if (e instanceof JCArrayAccess) {
            JCArrayAccess x = (JCArrayAccess) e;
            kv("k", "index"); key("a"); expr(x.indexed); out.append(','); key("i"); expr(x.index); out.append(',');
        } else {
            kv("k", "unknown");
        }
        kv("t", ty(t));
        kv("src", src(e));
        end();
    }
}
