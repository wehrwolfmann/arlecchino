//! jrt — runtime for Java ME code translated to Rust by `arlecchino port-rust`.
//!
//! Every Java reference (object, array, string) is a nullable shared handle.
//! Borrows of the referenced value never outlive one helper call (`get!`/`put!`,
//! `JArr::get`/`set`), so re-entrant calls on the same object cannot trip a
//! `RefCell` borrow.  Java exceptions travel as panics carrying [`Throw`] and are
//! caught only where the Java code had a `try` ([`catch`]).
//!
//! Integer semantics come from `j2me-jvm` (CC0, sushi-shi/j2me-preservation-kit).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

pub use j2me_jvm::JavaError;

pub mod api;
mod font_metrics;
pub mod gfx;
pub mod raster;

// ---- exceptions ----------------------------------------------------------

/// A Java exception travelling as a Rust panic payload.
#[derive(Debug, Clone)]
pub struct Throw {
    pub class: &'static str,
    pub err: Option<JavaError>,
}

impl Throw {
    /// `catch (T e)` test: is this exception an instance of Java class `t`?
    pub fn is_a(&self, t: &str) -> bool {
        if matches!(t, "java.lang.Throwable" | "java.lang.Exception") {
            return true;
        }
        if t == self.class {
            return true;
        }
        // the few hierarchy links J2ME code relies on
        let parents: &[(&str, &str)] = &[
            ("java.lang.ArrayIndexOutOfBoundsException", "java.lang.IndexOutOfBoundsException"),
            ("java.lang.StringIndexOutOfBoundsException", "java.lang.IndexOutOfBoundsException"),
            ("java.lang.IndexOutOfBoundsException", "java.lang.RuntimeException"),
            ("java.lang.NullPointerException", "java.lang.RuntimeException"),
            ("java.lang.ArithmeticException", "java.lang.RuntimeException"),
            ("java.lang.NegativeArraySizeException", "java.lang.RuntimeException"),
            ("java.lang.IllegalArgumentException", "java.lang.RuntimeException"),
            ("java.lang.NumberFormatException", "java.lang.IllegalArgumentException"),
            ("java.io.EOFException", "java.io.IOException"),
            ("javax.microedition.rms.RecordStoreNotFoundException", "javax.microedition.rms.RecordStoreException"),
        ];
        let mut c = self.class;
        loop {
            match parents.iter().find(|(k, _)| *k == c) {
                Some((_, p)) => {
                    if *p == t {
                        return true;
                    }
                    c = p;
                }
                None => return false,
            }
        }
    }
}

fn class_of(e: &JavaError) -> &'static str {
    match e {
        JavaError::NullPointer => "java.lang.NullPointerException",
        JavaError::ArrayIndexOutOfBounds { .. } => "java.lang.ArrayIndexOutOfBoundsException",
        JavaError::NegativeArraySize { .. } => "java.lang.NegativeArraySizeException",
        JavaError::Arithmetic => "java.lang.ArithmeticException",
        JavaError::IndexOutOfBounds => "java.lang.IndexOutOfBoundsException",
        _ => "java.lang.RuntimeException",
    }
}

pub fn throw(e: JavaError) -> ! {
    let class = class_of(&e);
    std::panic::panic_any(Throw { class, err: Some(e) })
}

/// `throw new T(...)`.
pub fn throw_new(class: &'static str) -> ! {
    std::panic::panic_any(Throw { class, err: None })
}

/// `"" + e` / `e.toString()` for a caught exception (class name only).
pub fn exc_string(t: &Throw) -> JStr {
    JStr::lit(t.class)
}

pub fn rethrow(t: Throw) -> ! {
    std::panic::panic_any(t)
}

/// Control flow leaving a `try` closure: normal end, `return`, or a labelled jump.
pub enum Flow<R> {
    Normal,
    Ret(R),
    Brk(i64),
    Cont(i64),
}

/// Runs `body`; a Java exception thrown inside comes back as `Err`.
/// Non-Java panics (including `todo_ai!`) keep unwinding.
pub fn catch<R>(body: impl FnOnce() -> R) -> Result<R, Throw> {
    install_hook();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(v) => Ok(v),
        Err(p) => match p.downcast::<Throw>() {
            Ok(t) => {
                if std::env::var_os("JRT_DEBUG").is_some() {
                    eprintln!("jrt: caught {} {:?}", t.class, t.err);
                }
                Err(*t)
            }
            Err(p) => std::panic::resume_unwind(p),
        },
    }
}

/// Silences the default panic message for Java exceptions that are caught.
pub fn install_hook() {
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if info.payload().downcast_ref::<Throw>().is_some() || info.payload().downcast_ref::<api::Stop>().is_some() {
            return;
        }
        prev(info)
    }));
}

/// Marks a construct the mechanical translator did not translate.
/// Expands to a diverging expression, so it type-checks in any position.
#[macro_export]
macro_rules! todo_ai {
    ($t:ty; $why:expr, $java:expr) => {
        $crate::untranslated_as::<$t>($why, $java)
    };
    ($why:expr, $java:expr) => {
        $crate::untranslated($why, $java)
    };
}

#[cold]
pub fn untranslated_as<T>(why: &str, java: &str) -> T {
    untranslated(why, java)
}

#[cold]
pub fn untranslated(why: &str, java: &str) -> ! {
    panic!("untranslated ({why}): {java}")
}

// ---- object handles -------------------------------------------------------

pub struct JRef<T>(Option<Rc<RefCell<T>>>);

impl<T> Clone for JRef<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> Default for JRef<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T> JRef<T> {
    pub fn new(v: T) -> Self {
        Self(Some(Rc::new(RefCell::new(v))))
    }
    pub fn null() -> Self {
        Self(None)
    }
    pub fn is_null(&self) -> bool {
        self.0.is_none()
    }
    pub fn ptr_eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
    fn cell(&self) -> &RefCell<T> {
        match &self.0 {
            Some(c) => c,
            None => throw(JavaError::NullPointer),
        }
    }
    pub fn rd<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.cell().borrow())
    }
    pub fn wr<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        f(&mut self.cell().borrow_mut())
    }
}

/// `o.f` (read; the value is cloned out, the borrow ends at once).
#[macro_export]
macro_rules! get {
    ($o:expr, $f:ident) => {
        $o.rd(|__o| __o.$f.clone())
    };
}

/// `o.f = v` (the value is computed BEFORE the object is borrowed).
#[macro_export]
macro_rules! put {
    ($o:expr, $f:ident, $v:expr) => {{
        let __v = $v;
        $o.wr(|__o| __o.$f = __v)
    }};
}

/// Reference equality (`==` on references).
pub trait Same {
    fn same(&self, other: &Self) -> bool;
}
impl<T> Same for JRef<T> {
    fn same(&self, o: &Self) -> bool {
        self.ptr_eq(o)
    }
}
impl<T> Same for JArr<T> {
    fn same(&self, o: &Self) -> bool {
        match (&self.0, &o.0) {
            (None, None) => true,
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}
impl Same for JStr {
    fn same(&self, o: &Self) -> bool {
        match (&self.0, &o.0) {
            (None, None) => true,
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}
pub fn same<T: Same>(a: &T, b: &T) -> bool {
    a.same(b)
}

// ---- statics ---------------------------------------------------------------

/// `Class.field` read for a `thread_local!` static (`S_x.val()`).
pub trait SGet<T> {
    fn val(&'static self) -> T;
}
impl<T: Clone + 'static> SGet<T> for std::thread::LocalKey<RefCell<T>> {
    fn val(&'static self) -> T {
        self.with(|c| c.borrow().clone())
    }
}

// ---- arrays ---------------------------------------------------------------

pub struct JArr<T>(Option<Rc<RefCell<Vec<T>>>>);

impl<T> Clone for JArr<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> Default for JArr<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T: Clone + Default> JArr<T> {
    pub fn new(len: i32) -> Self {
        if len < 0 {
            throw(JavaError::NegativeArraySize { length: len });
        }
        Self(Some(Rc::new(RefCell::new(vec![T::default(); len as usize]))))
    }
    pub fn from_vec(v: Vec<T>) -> Self {
        Self(Some(Rc::new(RefCell::new(v))))
    }
    pub fn is_null(&self) -> bool {
        self.0.is_none()
    }
    fn cell(&self) -> &RefCell<Vec<T>> {
        match &self.0 {
            Some(c) => c,
            None => throw(JavaError::NullPointer),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn len(&self) -> i32 {
        self.cell().borrow().len() as i32
    }
    pub fn get(&self, i: i32) -> T {
        let v = self.cell().borrow();
        if i < 0 || i as usize >= v.len() {
            let length = v.len() as i32;
            drop(v);
            throw(JavaError::ArrayIndexOutOfBounds { index: i, length });
        }
        v[i as usize].clone()
    }
    pub fn set(&self, i: i32, x: T) {
        let mut v = self.cell().borrow_mut();
        if i < 0 || i as usize >= v.len() {
            let length = v.len() as i32;
            drop(v);
            throw(JavaError::ArrayIndexOutOfBounds { index: i, length });
        }
        v[i as usize] = x;
    }
    /// Snapshot of the elements (for the platform layer).
    pub fn to_vec(&self) -> Vec<T> {
        self.cell().borrow().clone()
    }
    pub fn with<R>(&self, f: impl FnOnce(&[T]) -> R) -> R {
        f(&self.cell().borrow())
    }
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut [T]) -> R) -> R {
        f(&mut self.cell().borrow_mut())
    }
}

impl<T: Clone + Default> JArr<JArr<T>> {
    pub fn new_2d(a: i32, b: i32) -> Self {
        let outer = JArr::<JArr<T>>::new(a);
        if b < 0 {
            throw(JavaError::NegativeArraySize { length: b });
        }
        for i in 0..a {
            outer.set(i, JArr::<T>::new(b));
        }
        outer
    }
}

/// `System.arraycopy`, including the overlapping same-array case.
pub fn arraycopy<T: Clone + Default>(src: &JArr<T>, sp: i32, dst: &JArr<T>, dp: i32, n: i32) {
    if src.is_null() || dst.is_null() {
        throw(JavaError::NullPointer);
    }
    if n < 0 || sp < 0 || dp < 0 || sp as i64 + n as i64 > src.len() as i64 || dp as i64 + n as i64 > dst.len() as i64 {
        throw(JavaError::IndexOutOfBounds);
    }
    let tmp: Vec<T> = src.with(|s| s[sp as usize..(sp + n) as usize].to_vec());
    dst.with_mut(|d| d[dp as usize..(dp + n) as usize].clone_from_slice(&tmp));
}

// ---- strings (UTF-16 like java.lang.String) -------------------------------

#[derive(Clone, Default)]
pub struct JStr(pub Option<Rc<[u16]>>);

impl JStr {
    pub fn lit(s: &str) -> Self {
        Self(Some(s.encode_utf16().collect::<Vec<_>>().into()))
    }
    pub fn from_units(u: Vec<u16>) -> Self {
        Self(Some(u.into()))
    }
    pub fn is_null(&self) -> bool {
        self.0.is_none()
    }
    pub fn units(&self) -> &[u16] {
        match &self.0 {
            Some(u) => u,
            None => throw(JavaError::NullPointer),
        }
    }
    pub fn length(&self) -> i32 {
        self.units().len() as i32
    }
    pub fn char_at(&self, i: i32) -> u16 {
        let u = self.units();
        if i < 0 || i as usize >= u.len() {
            throw_new("java.lang.StringIndexOutOfBoundsException");
        }
        u[i as usize]
    }
    pub fn substring(&self, a: i32, b: i32) -> JStr {
        let u = self.units();
        if a < 0 || b > u.len() as i32 || a > b {
            throw_new("java.lang.StringIndexOutOfBoundsException");
        }
        JStr::from_units(u[a as usize..b as usize].to_vec())
    }
    pub fn substring_from(&self, a: i32) -> JStr {
        let n = self.length();
        self.substring(a, n)
    }
    pub fn equals(&self, o: &JStr) -> bool {
        !o.is_null() && self.units() == o.units()
    }
    pub fn index_of_char(&self, c: i32, from: i32) -> i32 {
        let u = self.units();
        let from = from.max(0) as usize;
        u.iter().skip(from).position(|&x| x as i32 == c).map(|p| (p + from) as i32).unwrap_or(-1)
    }
    pub fn to_rust(&self) -> String {
        String::from_utf16_lossy(self.units())
    }
    pub fn starts_with(&self, p: &JStr) -> bool {
        self.units().starts_with(p.units())
    }
    /// `String.trim()`: strips code units <= ' ' at both ends.
    pub fn trim(&self) -> JStr {
        let u = self.units();
        let a = u.iter().position(|&c| c > 0x20).unwrap_or(u.len());
        let b = u.iter().rposition(|&c| c > 0x20).map(|x| x + 1).unwrap_or(a);
        JStr::from_units(u[a..b.max(a)].to_vec())
    }
    /// `String.toUpperCase()` — CLDC maps only ISO-8859-1 letters.
    pub fn to_upper(&self) -> JStr {
        JStr::from_units(self.units().iter().map(|&c| api::char_to_upper(c)).collect())
    }
}

/// One piece of a Java string concatenation (`"a" + i + c`).
pub enum Piece {
    S(JStr),
    I(i64),
    C(u16),
    B(bool),
}

pub fn concat(parts: Vec<Piece>) -> JStr {
    let mut out: Vec<u16> = Vec::new();
    for p in parts {
        match p {
            Piece::S(s) => {
                if s.is_null() {
                    out.extend("null".encode_utf16());
                } else {
                    out.extend_from_slice(s.units());
                }
            }
            Piece::I(i) => out.extend(i.to_string().encode_utf16()),
            Piece::C(c) => out.push(c),
            Piece::B(b) => out.extend(b.to_string().encode_utf16()),
        }
    }
    JStr::from_units(out)
}

// ---- Java integer semantics (from j2me-jvm) -------------------------------

pub fn idiv(a: i32, b: i32) -> i32 {
    j2me_jvm::java_div(a, b).unwrap_or_else(|e| throw(e))
}
pub fn irem(a: i32, b: i32) -> i32 {
    j2me_jvm::java_rem(a, b).unwrap_or_else(|e| throw(e))
}
pub fn ldiv(a: i64, b: i64) -> i64 {
    j2me_jvm::java_ldiv(a, b).unwrap_or_else(|e| throw(e))
}
pub fn lrem(a: i64, b: i64) -> i64 {
    j2me_jvm::java_lrem(a, b).unwrap_or_else(|e| throw(e))
}
pub use j2me_jvm::{iabs, imax, imin, ishl, ishr, iushr, lshl, lshr, lushr};

// ---- platform peers ---------------------------------------------------------

/// Callbacks the platform makes into a user object (MIDlet, Canvas, Runnable…).
pub trait Callbacks {
    fn run(&self) {}
    fn paint(&self, _g: api::Graphics) {}
    fn key_pressed(&self, _k: i32) {}
    fn key_released(&self, _k: i32) {}
    fn key_repeated(&self, _k: i32) {}
    fn show_notify(&self) {}
    fn hide_notify(&self) {}
    fn size_changed(&self, _w: i32, _h: i32) {}
    fn start_app(&self) {}
    fn pause_app(&self) {}
    fn destroy_app(&self, _u: bool) {}
}

/// A user object handed to the platform as a Runnable/Displayable/MIDlet.
#[derive(Clone, Default)]
pub struct Peer(pub Option<Rc<dyn Callbacks>>);

impl Peer {
    pub fn of(c: impl Callbacks + 'static) -> Self {
        Peer(Some(Rc::new(c)))
    }
    pub fn is_null(&self) -> bool {
        self.0.is_none()
    }
    pub fn get(&self) -> Rc<dyn Callbacks> {
        match &self.0 {
            Some(c) => c.clone(),
            None => throw(JavaError::NullPointer),
        }
    }
}

/// A Java type the API table has no mapping for.
#[derive(Clone, Default)]
pub struct Unknown(pub Option<Rc<dyn Any>>);
impl Unknown {
    pub fn is_null(&self) -> bool {
        self.0.is_none()
    }
}
