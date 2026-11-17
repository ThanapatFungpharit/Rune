//! A minimal host for LINE's Emscripten-built `ltsm.wasm`.
//!
//! The module is a C++ (`LTSM::…`) build exposed through Embind. In the Chrome
//! extension it runs inside `ltsmSandbox.js`, whose generated glue supplies 40
//! minified imports (`a.a` … `a.N`). This file re-implements exactly those
//! imports for wasmtime:
//!
//! * a handful of libc/syscall shims (`/dev/urandom`, `memcpy_big`, …),
//! * the Embind *registration* calls (types, classes, methods), recorded into a
//!   registry so we can later call methods by name, and
//! * a tiny `emval` (`emscripten::val`) value table, covering what the module
//!   actually does with JS values: read `Uint8Array`s element by element,
//!   construct `Uint8Array`s from memory views, and convert numbers.
//!
//! The import letter → function mapping was taken from the `Zt` import object
//! in `ltsmSandbox.js` (LINE Chrome 3.7.2). It is specific to this wasm build;
//! a new `ltsm.wasm` needs that mapping re-checked.

use std::collections::HashMap;

use wasmtime::{
    Caller, Engine, Func, Instance, Linker, Memory, Module, Ref, Store, Table, TypedFunc, Val, ValType,
};

pub(crate) type WResult<T> = wasmtime::Result<T>;

fn err(msg: impl Into<String>) -> wasmtime::Error {
    wasmtime::Error::msg(msg.into())
}

// ---------------------------------------------------------------------------
// Registry types
// ---------------------------------------------------------------------------

/// A registered Embind type, keyed by its raw type id.
#[derive(Debug, Clone)]
pub(crate) enum TypeInfo {
    Void,
    Bool,
    Int {
        size: u32,
        signed: bool,
    },
    BigInt,
    Float {
        size: u32,
    },
    StdString,
    WString,
    Emval,
    MemoryView,
    /// A class value / pointer / const pointer; payload is the class raw type.
    Class(u32),
}

#[derive(Debug, Clone)]
pub(crate) struct Callable {
    /// Return type, then (for methods) `this`, then the parameters.
    pub arg_types: Vec<u32>,
    pub invoker: u32,
    /// Embind "context": the C++ function pointer the invoker dispatches to.
    pub context: u32,
    pub is_method: bool,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ClassInfo {
    pub name: String,
    pub base: u32,
    pub upcast: u32,
    pub destructor: u32,
    pub methods: HashMap<String, Callable>,
    pub statics: HashMap<String, Callable>,
    /// Constructors by parameter count.
    pub ctors: HashMap<usize, Callable>,
}

/// A JS value as seen by the module through `emscripten::val`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EmVal {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    /// A `Uint8Array` (or other typed array; only bytes are used here).
    Bytes(Vec<u8>),
    Array(Vec<EmVal>),
    /// `globalThis` (`None`) or a named global such as `Uint8Array`.
    Global(Option<String>),
    /// A C++ object handed to JS (class raw type, pointer).
    Object(u32, u32),
    /// The destructor list `emval::as` hands back; we never need its content.
    Destructors,
}

pub(crate) struct Host {
    origin: String,
    memory: Option<Memory>,
    table: Option<Table>,
    malloc: Option<TypedFunc<i32, i32>>,
    free: Option<TypedFunc<i32, ()>>,
    get_type_name: Option<TypedFunc<i32, i32>>,
    pub(crate) types: HashMap<u32, TypeInfo>,
    pub(crate) type_names: HashMap<u32, String>,
    pub(crate) classes: HashMap<u32, ClassInfo>,
    pub(crate) functions: HashMap<String, Callable>,
    pub(crate) constants: HashMap<String, f64>,
    emvals: Vec<Option<(u32, EmVal)>>,
    free_handles: Vec<u32>,
    /// Message of the last C++ exception thrown through `__cxa_throw`.
    last_exception: Option<String>,
}

const URANDOM_FD: i32 = 1000;

// EM_ASM snippet addresses in this build (the `fe` table of ltsmSandbox.js).
const ASM_THROW: i32 = 1456932; // (msg, detail) => { throw ... }
const ASM_RANDOM_INIT: i32 = 1456985; // () => init getRandomValues, 0 = ok
const ASM_RANDOM_FILL: i32 = 1457725; // (ptr, len) => getRandomValues, 0 = ok

impl Host {
    fn new(origin: String) -> Self {
        let mut emvals = Vec::with_capacity(64);
        // Handles 0..=4 are reserved by Embind: 1 undefined, 2 null, 3 true, 4 false.
        emvals.push(None);
        emvals.push(Some((u32::MAX, EmVal::Undefined)));
        emvals.push(Some((u32::MAX, EmVal::Null)));
        emvals.push(Some((u32::MAX, EmVal::Bool(true))));
        emvals.push(Some((u32::MAX, EmVal::Bool(false))));
        Host {
            origin,
            memory: None,
            table: None,
            malloc: None,
            free: None,
            get_type_name: None,
            types: HashMap::new(),
            type_names: HashMap::new(),
            classes: HashMap::new(),
            functions: HashMap::new(),
            constants: HashMap::new(),
            emvals,
            free_handles: Vec::new(),
            last_exception: None,
        }
    }

    pub(crate) fn new_handle(&mut self, v: EmVal) -> u32 {
        match v {
            EmVal::Undefined => 1,
            EmVal::Null => 2,
            EmVal::Bool(true) => 3,
            EmVal::Bool(false) => 4,
            v => {
                if let Some(h) = self.free_handles.pop() {
                    self.emvals[h as usize] = Some((1, v));
                    h
                } else {
                    self.emvals.push(Some((1, v)));
                    (self.emvals.len() - 1) as u32
                }
            }
        }
    }

    fn value(&self, h: u32) -> WResult<&EmVal> {
        self.emvals
            .get(h as usize)
            .and_then(|e| e.as_ref())
            .map(|(_, v)| v)
            .ok_or_else(|| err(format!("use of deleted emval handle {h}")))
    }

    fn incref(&mut self, h: u32) {
        if h > 4
            && let Some(Some((rc, _))) = self.emvals.get_mut(h as usize)
        {
            *rc += 1;
        }
    }

    pub(crate) fn decref(&mut self, h: u32) {
        if h <= 4 {
            return;
        }
        if let Some(slot) = self.emvals.get_mut(h as usize)
            && let Some((rc, _)) = slot
        {
            *rc -= 1;
            if *rc == 0 {
                *slot = None;
                self.free_handles.push(h);
            }
        }
    }

    /// Take the value out of a handle the module returned to us (it transfers
    /// one reference to the caller).
    pub(crate) fn take_handle(&mut self, h: u32) -> WResult<EmVal> {
        let v = self.value(h)?.clone();
        self.decref(h);
        Ok(v)
    }

    pub(crate) fn live_handles(&self) -> usize {
        self.emvals.iter().skip(5).filter(|e| e.is_some()).count()
    }

    fn live_handle_ids(&self) -> Vec<u32> {
        (5..self.emvals.len() as u32)
            .filter(|&h| self.emvals[h as usize].is_some())
            .collect()
    }

    /// After a trap no C++ frame of the failed call survives, so every emval
    /// created during it is garbage (its `val` destructors never ran).
    fn drop_handles_except(&mut self, keep: &[u32]) {
        for h in 5..self.emvals.len() as u32 {
            if self.emvals[h as usize].is_some() && !keep.contains(&h) {
                self.emvals[h as usize] = None;
                self.free_handles.push(h);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Memory helpers (work on both `Caller` and `Store`)
// ---------------------------------------------------------------------------

fn mem_of(caller: &Caller<'_, Host>) -> WResult<Memory> {
    caller.data().memory.ok_or_else(|| err("memory not ready"))
}

fn read_bytes(caller: &Caller<'_, Host>, ptr: u32, len: u32) -> WResult<Vec<u8>> {
    let mem = mem_of(caller)?;
    let data = mem.data(caller);
    let (s, e) = (ptr as usize, ptr as usize + len as usize);
    data.get(s..e)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| err("out-of-bounds read"))
}

fn read_cstr(caller: &Caller<'_, Host>, ptr: u32) -> WResult<String> {
    let mem = mem_of(caller)?;
    let data = mem.data(caller);
    let start = ptr as usize;
    let tail = data.get(start..).ok_or_else(|| err("bad cstr pointer"))?;
    let len = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    // Embind names are Latin-1; they are plain ASCII in practice.
    Ok(tail[..len].iter().map(|&b| b as char).collect())
}

fn read_u32(caller: &Caller<'_, Host>, ptr: u32) -> WResult<u32> {
    let b = read_bytes(caller, ptr, 4)?;
    Ok(u32::from_le_bytes(b.try_into().unwrap()))
}

fn write_bytes(caller: &mut Caller<'_, Host>, ptr: u32, bytes: &[u8]) -> WResult<()> {
    let mem = mem_of(caller)?;
    let data = mem.data_mut(caller);
    let s = ptr as usize;
    data.get_mut(s..s + bytes.len())
        .ok_or_else(|| err("out-of-bounds write"))?
        .copy_from_slice(bytes);
    Ok(())
}

fn malloc(caller: &mut Caller<'_, Host>, n: u32) -> WResult<u32> {
    let f = caller
        .data()
        .malloc
        .clone()
        .ok_or_else(|| err("malloc not ready"))?;
    let p = f.call(&mut *caller, n as i32)? as u32;
    if p == 0 {
        return Err(err("wasm malloc failed"));
    }
    Ok(p)
}

fn free(caller: &mut Caller<'_, Host>, p: u32) -> WResult<()> {
    let f = caller.data().free.clone().ok_or_else(|| err("free not ready"))?;
    f.call(&mut *caller, p as i32)
}

fn read_u32_vec(caller: &Caller<'_, Host>, ptr: u32, n: u32) -> WResult<Vec<u32>> {
    (0..n).map(|i| read_u32(caller, ptr + 4 * i)).collect()
}

fn fill_random(buf: &mut [u8]) -> WResult<()> {
    getrandom::fill(buf).map_err(|e| err(format!("getrandom: {e}")))
}

/// `readValueFromPointer` for a registered type (used by `_emval_take_value`
/// and `_emval_new` argument packs).
fn read_value_from_pointer(caller: &mut Caller<'_, Host>, ty: u32, ptr: u32) -> WResult<EmVal> {
    let info = caller
        .data()
        .types
        .get(&ty)
        .cloned()
        .ok_or_else(|| err(format!("unknown type id {ty}")))?;
    Ok(match info {
        TypeInfo::Int { size, signed } => {
            let b = read_bytes(caller, ptr, size)?;
            let v = match (size, signed) {
                (1, true) => b[0] as i8 as f64,
                (1, false) => b[0] as f64,
                (2, true) => i16::from_le_bytes([b[0], b[1]]) as f64,
                (2, false) => u16::from_le_bytes([b[0], b[1]]) as f64,
                (4, true) => i32::from_le_bytes(b[..4].try_into().unwrap()) as f64,
                (4, false) => u32::from_le_bytes(b[..4].try_into().unwrap()) as f64,
                _ => return Err(err(format!("unsupported int size {size}"))),
            };
            EmVal::Number(v)
        }
        TypeInfo::Float { size } => {
            let b = read_bytes(caller, ptr, size)?;
            EmVal::Number(match size {
                4 => f32::from_le_bytes(b[..4].try_into().unwrap()) as f64,
                8 => f64::from_le_bytes(b[..8].try_into().unwrap()),
                _ => return Err(err(format!("unsupported float size {size}"))),
            })
        }
        TypeInfo::Bool => EmVal::Bool(read_bytes(caller, ptr, 1)?[0] != 0),
        TypeInfo::MemoryView => {
            let size = read_u32(caller, ptr)?;
            let data = read_u32(caller, ptr + 4)?;
            EmVal::Bytes(read_bytes(caller, data, size)?)
        }
        TypeInfo::Emval => {
            let h = read_u32(caller, ptr)?;
            caller.data_mut().take_handle(h)?
        }
        TypeInfo::StdString => {
            let p = read_u32(caller, ptr)?;
            let len = read_u32(caller, p)?;
            let bytes = read_bytes(caller, p + 4, len)?;
            free(caller, p)?;
            EmVal::Str(String::from_utf8_lossy(&bytes).into_owned())
        }
        TypeInfo::Class(raw) => EmVal::Object(raw, read_u32(caller, ptr)?),
        other => return Err(err(format!("readValueFromPointer: unsupported {other:?}"))),
    })
}

// ---------------------------------------------------------------------------
// Imports
// ---------------------------------------------------------------------------

fn register_type(caller: &mut Caller<'_, Host>, id: i32, name_ptr: i32, info: TypeInfo) -> WResult<()> {
    let name = read_cstr(caller, name_ptr as u32)?;
    let h = caller.data_mut();
    h.types.insert(id as u32, info);
    h.type_names.insert(id as u32, name);
    Ok(())
}

fn class_entry(h: &mut Host, raw: u32) -> &mut ClassInfo {
    h.classes.entry(raw).or_default()
}

fn add_imports(linker: &mut Linker<Host>) -> WResult<()> {
    // --- C++ runtime -------------------------------------------------------
    // p: __cxa_allocate_exception(size) -> malloc(size + 24) + 24
    linker.func_wrap("a", "p", |mut c: Caller<'_, Host>, size: i32| -> WResult<i32> {
        Ok((malloc(&mut c, size as u32 + 24)? + 24) as i32)
    })?;
    // B: __cxa_throw(ptr, type, destructor)
    linker.func_wrap(
        "a",
        "B",
        |mut c: Caller<'_, Host>, ptr: i32, ty: i32, _dtor: i32| -> WResult<()> {
            let name = match c.data().get_type_name.clone() {
                Some(f) => {
                    let p = f.call(&mut c, ty)? as u32;
                    let n = read_cstr(&c, p)?;
                    free(&mut c, p)?;
                    n
                }
                None => format!("type#{ty}"),
            };
            let code = read_u32(&c, ptr as u32).unwrap_or(0) as i32;
            let msg = format!("C++ exception {name} (value {code})");
            c.data_mut().last_exception = Some(msg.clone());
            Err(err(msg))
        },
    )?;
    // m: abort
    linker.func_wrap("a", "m", |_c: Caller<'_, Host>| -> WResult<()> {
        Err(err("wasm abort()"))
    })?;
    // H: emscripten_memcpy_big(dest, src, n)
    linker.func_wrap(
        "a",
        "H",
        |c: Caller<'_, Host>, dest: i32, src: i32, n: i32| -> WResult<()> {
            let mem = mem_of(&c)?;
            let mut c = c;
            let data = mem.data_mut(&mut c);
            let (d, s, n) = (dest as u32 as usize, src as u32 as usize, n as u32 as usize);
            if s + n > data.len() || d + n > data.len() {
                return Err(err("memcpy_big out of bounds"));
            }
            data.copy_within(s..s + n, d);
            Ok(())
        },
    )?;
    // C: emscripten_resize_heap -> this build's glue always refuses.
    linker.func_wrap("a", "C", |_c: Caller<'_, Host>, _n: i32| -> i32 { 0 })?;
    // w: returns malloc'd window.location.origin (checked against the LTSM token).
    linker.func_wrap("a", "w", |mut c: Caller<'_, Host>| -> WResult<i32> {
        let origin = c.data().origin.clone();
        let p = malloc(&mut c, origin.len() as u32 + 1)?;
        let mut bytes = origin.into_bytes();
        bytes.push(0);
        write_bytes(&mut c, p, &bytes)?;
        Ok(p as i32)
    })?;
    // o: emscripten_asm_const_int(code, sigPtr, argbuf)
    linker.func_wrap(
        "a",
        "o",
        |mut c: Caller<'_, Host>, code: i32, sig: i32, buf: i32| -> WResult<i32> {
            let sig = read_cstr(&c, sig as u32)?;
            // Read EM_ASM args: 'i' = i32 (4-aligned), 'j' = i64, else f64 (8-aligned).
            let mut args = Vec::new();
            let mut p = buf as u32;
            for ch in sig.chars() {
                if ch == 'i' {
                    args.push(read_u32(&c, p)? as f64);
                    p += 4;
                } else {
                    p = (p + 7) & !7;
                    let b = read_bytes(&c, p, 8)?;
                    args.push(if ch == 'j' {
                        i64::from_le_bytes(b.try_into().unwrap()) as f64
                    } else {
                        f64::from_le_bytes(b.try_into().unwrap())
                    });
                    p += 8;
                }
            }
            match code {
                ASM_RANDOM_INIT => Ok(0),
                ASM_RANDOM_FILL => {
                    let (ptr, len) = (args[0] as u32, args[1] as u32);
                    let mut b = vec![0u8; len as usize];
                    fill_random(&mut b)?;
                    write_bytes(&mut c, ptr, &b)?;
                    Ok(0)
                }
                ASM_THROW => {
                    let a = read_cstr(&c, args[0] as u32)?;
                    let b = read_cstr(&c, args[1] as u32)?;
                    let msg = format!("{a}: {b}");
                    c.data_mut().last_exception = Some(msg.clone());
                    Err(err(msg))
                }
                _ => Err(err(format!("unknown EM_ASM snippet {code}"))),
            }
        },
    )?;

    // --- syscalls (only /dev/urandom is ever opened) -------------------------
    // G: __syscall_openat(dirfd, path, flags, varargs)
    linker.func_wrap(
        "a",
        "G",
        |c: Caller<'_, Host>, _dirfd: i32, path: i32, _flags: i32, _va: i32| -> WResult<i32> {
            let p = read_cstr(&c, path as u32)?;
            Ok(if p == "/dev/urandom" || p == "/dev/random" {
                URANDOM_FD
            } else {
                -44 // ENOENT
            })
        },
    )?;
    // E: fd_read(fd, iov, iovcnt, pnum)
    linker.func_wrap(
        "a",
        "E",
        |mut c: Caller<'_, Host>, fd: i32, iov: i32, iovcnt: i32, pnum: i32| -> WResult<i32> {
            if fd != URANDOM_FD {
                return Ok(8); // EBADF
            }
            let mut total = 0u32;
            for i in 0..iovcnt as u32 {
                let base = read_u32(&c, iov as u32 + 8 * i)?;
                let len = read_u32(&c, iov as u32 + 8 * i + 4)?;
                let mut b = vec![0u8; len as usize];
                fill_random(&mut b)?;
                write_bytes(&mut c, base, &b)?;
                total += len;
            }
            write_bytes(&mut c, pnum as u32, &total.to_le_bytes())?;
            Ok(0)
        },
    )?;
    // r: fd_write(fd, iov, iovcnt, pnum) — stdout/stderr from the module.
    linker.func_wrap(
        "a",
        "r",
        |mut c: Caller<'_, Host>, fd: i32, iov: i32, iovcnt: i32, pnum: i32| -> WResult<i32> {
            let mut out = Vec::new();
            for i in 0..iovcnt as u32 {
                let base = read_u32(&c, iov as u32 + 8 * i)?;
                let len = read_u32(&c, iov as u32 + 8 * i + 4)?;
                out.extend(read_bytes(&c, base, len)?);
            }
            tracing::debug!(fd, "ltsm.wasm output: {}", String::from_utf8_lossy(&out));
            write_bytes(&mut c, pnum as u32, &(out.len() as u32).to_le_bytes())?;
            Ok(0)
        },
    )?;
    // q: fd_close
    linker.func_wrap("a", "q", |_c: Caller<'_, Host>, _fd: i32| -> i32 { 0 })?;
    // s: __syscall_fcntl64
    linker.func_wrap(
        "a",
        "s",
        |_c: Caller<'_, Host>, _a: i32, _b: i32, _d: i32| -> i32 { 0 },
    )?;
    // F: __syscall_ioctl -> ENOTTY
    linker.func_wrap(
        "a",
        "F",
        |_c: Caller<'_, Host>, _a: i32, _b: i32, _d: i32| -> i32 { -59 },
    )?;
    // D: fd_seek -> ESPIPE
    linker.func_wrap(
        "a",
        "D",
        |_c: Caller<'_, Host>, _fd: i32, _off: i64, _wh: i32, _p: i32| -> i32 { 70 },
    )?;

    // --- Embind: primitive type registration -------------------------------
    linker.func_wrap("a", "K", |mut c: Caller<'_, Host>, id: i32, name: i32| {
        register_type(&mut c, id, name, TypeInfo::Void)
    })?;
    linker.func_wrap(
        "a",
        "J",
        |mut c: Caller<'_, Host>, id: i32, name: i32, _size: i32, _t: i32, _f: i32| {
            register_type(&mut c, id, name, TypeInfo::Bool)
        },
    )?;
    linker.func_wrap(
        "a",
        "h",
        |mut c: Caller<'_, Host>, id: i32, name: i32, size: i32, min: i32, _max: i32| {
            // Embind treats an integer type as unsigned when its minimum is 0.
            let info = TypeInfo::Int {
                size: size as u32,
                signed: min != 0,
            };
            register_type(&mut c, id, name, info)
        },
    )?;
    linker.func_wrap(
        "a",
        "v",
        |mut c: Caller<'_, Host>, id: i32, name: i32, _size: i32, _min: i64, _max: i64| {
            register_type(&mut c, id, name, TypeInfo::BigInt)
        },
    )?;
    linker.func_wrap(
        "a",
        "u",
        |mut c: Caller<'_, Host>, id: i32, name: i32, size: i32| {
            register_type(&mut c, id, name, TypeInfo::Float { size: size as u32 })
        },
    )?;
    linker.func_wrap("a", "t", |mut c: Caller<'_, Host>, id: i32, name: i32| {
        register_type(&mut c, id, name, TypeInfo::StdString)
    })?;
    linker.func_wrap(
        "a",
        "n",
        |mut c: Caller<'_, Host>, id: i32, _cs: i32, name: i32| {
            register_type(&mut c, id, name, TypeInfo::WString)
        },
    )?;
    linker.func_wrap("a", "I", |mut c: Caller<'_, Host>, id: i32, name: i32| {
        register_type(&mut c, id, name, TypeInfo::Emval)
    })?;
    linker.func_wrap(
        "a",
        "d",
        |mut c: Caller<'_, Host>, id: i32, _kind: i32, name: i32| {
            register_type(&mut c, id, name, TypeInfo::MemoryView)
        },
    )?;
    // c: _embind_register_constant(name, type, value)
    linker.func_wrap(
        "a",
        "c",
        |mut c: Caller<'_, Host>, name: i32, _ty: i32, v: f64| -> WResult<()> {
            let n = read_cstr(&c, name as u32)?;
            c.data_mut().constants.insert(n, v);
            Ok(())
        },
    )?;

    // --- Embind: classes and functions -------------------------------------
    // i: _embind_register_class(raw, ptr, constPtr, base, getActualTypeSig,
    //    getActualType, upcastSig, upcast, downcastSig, downcast, name,
    //    destructorSig, destructor)
    linker.func_wrap(
        "a",
        "i",
        |mut c: Caller<'_, Host>,
         raw: i32,
         ptr: i32,
         cptr: i32,
         base: i32,
         _gats: i32,
         _gat: i32,
         _ups: i32,
         upcast: i32,
         _dns: i32,
         _down: i32,
         name: i32,
         _ds: i32,
         dtor: i32|
         -> WResult<()> {
            let name = read_cstr(&c, name as u32)?;
            let h = c.data_mut();
            for (id, suffix) in [(raw, ""), (ptr, "*"), (cptr, " const*")] {
                h.types.insert(id as u32, TypeInfo::Class(raw as u32));
                h.type_names.insert(id as u32, format!("{name}{suffix}"));
            }
            let cls = class_entry(h, raw as u32);
            cls.name = name;
            cls.base = base as u32;
            cls.upcast = upcast as u32;
            cls.destructor = dtor as u32;
            Ok(())
        },
    )?;
    // k: _embind_register_class_constructor(cls, argc, argTypes, sig, invoker, ctor)
    linker.func_wrap(
        "a",
        "k",
        |mut c: Caller<'_, Host>,
         cls: i32,
         argc: i32,
         types: i32,
         _sig: i32,
         inv: i32,
         ctor: i32|
         -> WResult<()> {
            let mut arg_types = read_u32_vec(&c, types as u32, argc as u32)?;
            arg_types.insert(1, 0); // no `this`
            let callable = Callable {
                arg_types,
                invoker: inv as u32,
                context: ctor as u32,
                is_method: false,
            };
            class_entry(c.data_mut(), cls as u32)
                .ctors
                .insert(argc as usize - 1, callable);
            Ok(())
        },
    )?;
    // b: _embind_register_class_function(cls, name, argc, argTypes, sig, invoker, context, pureVirtual)
    linker.func_wrap(
        "a",
        "b",
        |mut c: Caller<'_, Host>,
         cls: i32,
         name: i32,
         argc: i32,
         types: i32,
         _sig: i32,
         inv: i32,
         ctx: i32,
         _pv: i32|
         -> WResult<()> {
            let name = read_cstr(&c, name as u32)?;
            let arg_types = read_u32_vec(&c, types as u32, argc as u32)?;
            let callable = Callable {
                arg_types,
                invoker: inv as u32,
                context: ctx as u32,
                is_method: true,
            };
            class_entry(c.data_mut(), cls as u32)
                .methods
                .insert(name, callable);
            Ok(())
        },
    )?;
    // g: _embind_register_class_class_function(cls, name, argc, argTypes, sig, invoker, fn)
    linker.func_wrap(
        "a",
        "g",
        |mut c: Caller<'_, Host>,
         cls: i32,
         name: i32,
         argc: i32,
         types: i32,
         _sig: i32,
         inv: i32,
         f: i32|
         -> WResult<()> {
            let name = read_cstr(&c, name as u32)?;
            let mut arg_types = read_u32_vec(&c, types as u32, argc as u32)?;
            arg_types.insert(1, 0); // no `this`
            let callable = Callable {
                arg_types,
                invoker: inv as u32,
                context: f as u32,
                is_method: false,
            };
            class_entry(c.data_mut(), cls as u32)
                .statics
                .insert(name, callable);
            Ok(())
        },
    )?;
    // N: _embind_register_function(name, argc, argTypes, sig, invoker, fn)
    linker.func_wrap(
        "a",
        "N",
        |mut c: Caller<'_, Host>,
         name: i32,
         argc: i32,
         types: i32,
         _sig: i32,
         inv: i32,
         f: i32|
         -> WResult<()> {
            let name = read_cstr(&c, name as u32)?;
            let mut arg_types = read_u32_vec(&c, types as u32, argc as u32)?;
            arg_types.insert(1, 0);
            let callable = Callable {
                arg_types,
                invoker: inv as u32,
                context: f as u32,
                is_method: false,
            };
            c.data_mut().functions.insert(name, callable);
            Ok(())
        },
    )?;

    // --- emval ---------------------------------------------------------------
    // a: _emval_decref
    linker.func_wrap("a", "a", |mut c: Caller<'_, Host>, h: i32| {
        c.data_mut().decref(h as u32)
    })?;
    // j: _emval_incref
    linker.func_wrap("a", "j", |mut c: Caller<'_, Host>, h: i32| {
        c.data_mut().incref(h as u32)
    })?;
    // y: _emval_run_destructors(handle) — our destructor lists are always empty.
    linker.func_wrap("a", "y", |mut c: Caller<'_, Host>, h: i32| {
        c.data_mut().decref(h as u32)
    })?;
    // x: _emval_new_array
    linker.func_wrap("a", "x", |mut c: Caller<'_, Host>| -> i32 {
        c.data_mut().new_handle(EmVal::Array(Vec::new())) as i32
    })?;
    // M: _emval_new_cstring
    linker.func_wrap("a", "M", |mut c: Caller<'_, Host>, p: i32| -> WResult<i32> {
        let s = read_cstr(&c, p as u32)?;
        Ok(c.data_mut().new_handle(EmVal::Str(s)) as i32)
    })?;
    // f: _emval_get_global(name) — 0 means globalThis.
    linker.func_wrap("a", "f", |mut c: Caller<'_, Host>, p: i32| -> WResult<i32> {
        let v = if p == 0 {
            EmVal::Global(None)
        } else {
            EmVal::Global(Some(read_cstr(&c, p as u32)?))
        };
        Ok(c.data_mut().new_handle(v) as i32)
    })?;
    // A: _emval_get_property(obj, key)
    linker.func_wrap(
        "a",
        "A",
        |mut c: Caller<'_, Host>, obj: i32, key: i32| -> WResult<i32> {
            let h = c.data();
            let (o, k) = (h.value(obj as u32)?, h.value(key as u32)?);
            let v = match (o, k) {
                (EmVal::Bytes(b), EmVal::Str(s)) if s == "length" => EmVal::Number(b.len() as f64),
                (EmVal::Bytes(b), EmVal::Number(i)) => b
                    .get(*i as usize)
                    .map_or(EmVal::Undefined, |x| EmVal::Number(*x as f64)),
                (EmVal::Array(a), EmVal::Str(s)) if s == "length" => EmVal::Number(a.len() as f64),
                (EmVal::Array(a), EmVal::Number(i)) => {
                    a.get(*i as usize).cloned().unwrap_or(EmVal::Undefined)
                }
                (EmVal::Str(s), EmVal::Str(k)) if k == "length" => {
                    EmVal::Number(s.encode_utf16().count() as f64)
                }
                (EmVal::Global(None), EmVal::Str(name)) => EmVal::Global(Some(name.clone())),
                (o, k) => return Err(err(format!("emval get_property unsupported: {o:?}[{k:?}]"))),
            };
            Ok(c.data_mut().new_handle(v) as i32)
        },
    )?;
    // L: _emval_set_property(obj, key, value)
    linker.func_wrap(
        "a",
        "L",
        |mut c: Caller<'_, Host>, obj: i32, key: i32, val: i32| -> WResult<()> {
            let h = c.data_mut();
            let k = h.value(key as u32)?.clone();
            let v = h.value(val as u32)?.clone();
            let slot = h
                .emvals
                .get_mut(obj as usize)
                .and_then(|e| e.as_mut())
                .ok_or_else(|| err("set_property on deleted handle"))?;
            match (&mut slot.1, k) {
                (EmVal::Array(a), EmVal::Number(i)) => {
                    let i = i as usize;
                    if a.len() <= i {
                        a.resize(i + 1, EmVal::Undefined);
                    }
                    a[i] = v;
                    Ok(())
                }
                (o, k) => Err(err(format!("emval set_property unsupported: {o:?}[{k:?}]"))),
            }
        },
    )?;
    // l: _emval_take_value(type, ptr)
    linker.func_wrap(
        "a",
        "l",
        |mut c: Caller<'_, Host>, ty: i32, p: i32| -> WResult<i32> {
            let v = read_value_from_pointer(&mut c, ty as u32, p as u32)?;
            Ok(c.data_mut().new_handle(v) as i32)
        },
    )?;
    // z: _emval_as(handle, type, destructorsRef) -> f64
    linker.func_wrap(
        "a",
        "z",
        |mut c: Caller<'_, Host>, h: i32, ty: i32, dref: i32| -> WResult<f64> {
            let v = c.data().value(h as u32)?.clone();
            let info = c.data().types.get(&(ty as u32)).cloned();
            let d = c.data_mut().new_handle(EmVal::Destructors);
            write_bytes(&mut c, dref as u32, &d.to_le_bytes())?;
            match (info, v) {
                (Some(TypeInfo::Int { size, signed }), EmVal::Number(n)) => {
                    // JS ToInt32/ToUint32-style wrapping, like the generated toWireType.
                    let bits = size * 8;
                    let mut x = (n as i64) & ((1i64 << bits) - 1);
                    if signed && bits < 64 && x >= 1i64 << (bits - 1) {
                        x -= 1i64 << bits;
                    }
                    Ok(x as f64)
                }
                (Some(TypeInfo::Float { .. }), EmVal::Number(n)) => Ok(n),
                (Some(TypeInfo::Bool), v) => Ok(if v == EmVal::Bool(true) { 1.0 } else { 0.0 }),
                (info, v) => Err(err(format!("emval as unsupported: {v:?} as {info:?}"))),
            }
        },
    )?;
    // e: _emval_new(ctor, argCount, argTypes, argsPtr)
    linker.func_wrap(
        "a",
        "e",
        |mut c: Caller<'_, Host>, ctor: i32, argc: i32, types: i32, args: i32| -> WResult<i32> {
            let ctor = c.data().value(ctor as u32)?.clone();
            let types = read_u32_vec(&c, types as u32, argc as u32)?;
            let mut vals = Vec::new();
            let mut p = args as u32;
            for ty in types {
                vals.push(read_value_from_pointer(&mut c, ty, p)?);
                p += 8; // argPackAdvance
            }
            let v = match (&ctor, vals.as_slice()) {
                (EmVal::Global(Some(n)), [EmVal::Bytes(b)]) if n == "Uint8Array" => EmVal::Bytes(b.clone()),
                (EmVal::Global(Some(n)), [EmVal::Number(len)]) if n == "Uint8Array" => {
                    EmVal::Bytes(vec![0; *len as usize])
                }
                (EmVal::Global(Some(n)), [EmVal::Array(a)]) if n == "Uint8Array" => EmVal::Bytes(
                    a.iter()
                        .map(|v| if let EmVal::Number(x) = v { *x as u8 } else { 0 })
                        .collect(),
                ),
                _ => return Err(err(format!("emval new unsupported: new {ctor:?}({vals:?})"))),
            };
            Ok(c.data_mut().new_handle(v) as i32)
        },
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// The runtime
// ---------------------------------------------------------------------------

/// Argument passed into an Embind call. Converted to the wire form the
/// declared parameter type expects.
#[derive(Debug, Clone)]
pub(crate) enum Arg {
    Int(i64),
    Bool(bool),
    Bytes(Vec<u8>),
    Str(String),
    Obj(Object),
}

/// Result of an Embind call, converted from the declared return type.
#[derive(Debug, Clone)]
pub(crate) enum Ret {
    Void,
    Num(f64),
    BigInt(i64),
    Val(EmVal),
    Obj(Object),
}

/// A C++ object living in wasm memory (class raw type id + pointer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Object {
    pub class: u32,
    pub ptr: u32,
}

pub(crate) struct Runtime {
    pub(crate) store: Store<Host>,
    table: Table,
    malloc: TypedFunc<i32, i32>,
    free: TypedFunc<i32, ()>,
}

impl Runtime {
    pub(crate) fn new(wasm: &[u8], origin: &str) -> WResult<Self> {
        let mut config = wasmtime::Config::new();
        config.cranelift_opt_level(wasmtime::OptLevel::Speed);
        let engine = Engine::new(&config)?;
        let module = Module::new(&engine, wasm)?;
        let mut linker = Linker::new(&engine);
        add_imports(&mut linker)?;
        let mut store = Store::new(&engine, Host::new(origin.to_owned()));
        let instance: Instance = linker.instantiate(&mut store, &module)?;

        let memory = instance
            .get_memory(&mut store, "O")
            .ok_or_else(|| err("ltsm.wasm: no memory export"))?;
        let table = instance
            .get_table(&mut store, "Q")
            .ok_or_else(|| err("ltsm.wasm: no table export"))?;
        let malloc = instance.get_typed_func::<i32, i32>(&mut store, "V")?;
        let free = instance.get_typed_func::<i32, ()>(&mut store, "R")?;
        let get_type_name = instance.get_typed_func::<i32, i32>(&mut store, "S")?;
        {
            let h = store.data_mut();
            h.memory = Some(memory);
            h.table = Some(table);
            h.malloc = Some(malloc.clone());
            h.free = Some(free.clone());
            h.get_type_name = Some(get_type_name);
        }
        // __wasm_call_ctors: runs the EMSCRIPTEN_BINDINGS registrations.
        let ctors = instance.get_typed_func::<(), ()>(&mut store, "P")?;
        ctors.call(&mut store, ())?;
        Ok(Runtime {
            store,
            table,
            malloc,
            free,
        })
    }

    fn mem(&self) -> Memory {
        self.store.data().memory.expect("memory set at init")
    }

    fn alloc_bytes(&mut self, bytes: &[u8], prefix_len: bool, nul: bool) -> WResult<u32> {
        let total = bytes.len() + if prefix_len { 4 } else { 0 } + usize::from(nul);
        let p = self.malloc.call(&mut self.store, total.max(1) as i32)? as u32;
        if p == 0 {
            return Err(err("wasm malloc failed"));
        }
        let mem = self.mem();
        let data = mem.data_mut(&mut self.store);
        let mut off = p as usize;
        if prefix_len {
            data[off..off + 4].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            off += 4;
        }
        data[off..off + bytes.len()].copy_from_slice(bytes);
        if nul {
            data[off + bytes.len()] = 0;
        }
        Ok(p)
    }

    fn read(&self, ptr: u32, len: u32) -> WResult<Vec<u8>> {
        let data = self.mem().data(&self.store);
        data.get(ptr as usize..(ptr + len) as usize)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| err("out-of-bounds read"))
    }

    fn read_u32(&self, ptr: u32) -> WResult<u32> {
        Ok(u32::from_le_bytes(self.read(ptr, 4)?.try_into().unwrap()))
    }

    fn table_func(&mut self, idx: u32) -> WResult<Func> {
        match self.table.get(&mut self.store, idx as u64) {
            Some(Ref::Func(Some(f))) => Ok(f),
            _ => Err(err(format!("no function at table index {idx}"))),
        }
    }

    pub(crate) fn class_by_name(&self, name: &str) -> WResult<u32> {
        self.store
            .data()
            .classes
            .iter()
            .find(|(_, c)| c.name == name)
            .map(|(id, _)| *id)
            .ok_or_else(|| err(format!("class {name} not registered")))
    }

    /// Upcast `obj` to `target` by walking the base-class chain.
    fn upcast(&mut self, obj: Object, target: u32) -> WResult<u32> {
        let mut cls = obj.class;
        let mut ptr = obj.ptr;
        while cls != target {
            let info = self
                .store
                .data()
                .classes
                .get(&cls)
                .cloned()
                .ok_or_else(|| err(format!("unknown class {cls}")))?;
            if info.base == 0 {
                let want = self.type_name(target);
                return Err(err(format!("{} is not a {want}", info.name)));
            }
            if info.upcast != 0 {
                let f = self.table_func(info.upcast)?;
                let mut out = [Val::I32(0)];
                f.call(&mut self.store, &[Val::I32(ptr as i32)], &mut out)?;
                ptr = out[0].unwrap_i32() as u32;
            }
            cls = info.base;
        }
        Ok(ptr)
    }

    pub(crate) fn type_name(&self, id: u32) -> String {
        self.store
            .data()
            .type_names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("type#{id}"))
    }

    /// Convert one argument to its wire value; returns the value plus a heap
    /// pointer to free after the call (std::string arguments).
    fn encode_arg(&mut self, ty: u32, arg: &Arg) -> WResult<(Val, Option<u32>)> {
        let info = self
            .store
            .data()
            .types
            .get(&ty)
            .cloned()
            .ok_or_else(|| err(format!("unknown parameter type {ty}")))?;
        let v = match (&info, arg) {
            (TypeInfo::Int { .. }, Arg::Int(n)) => Val::I32(*n as i32),
            (TypeInfo::BigInt, Arg::Int(n)) => Val::I64(*n),
            (TypeInfo::Bool, Arg::Bool(b)) => Val::I32(i32::from(*b)),
            (TypeInfo::StdString, Arg::Str(s)) => {
                let p = self.alloc_bytes(s.as_bytes(), true, true)?;
                return Ok((Val::I32(p as i32), Some(p)));
            }
            (TypeInfo::StdString, Arg::Bytes(b)) => {
                let p = self.alloc_bytes(b, true, true)?;
                return Ok((Val::I32(p as i32), Some(p)));
            }
            (TypeInfo::Emval, a) => {
                let v = match a {
                    Arg::Int(n) => EmVal::Number(*n as f64),
                    Arg::Bool(b) => EmVal::Bool(*b),
                    Arg::Bytes(b) => EmVal::Bytes(b.clone()),
                    Arg::Str(s) => EmVal::Str(s.clone()),
                    Arg::Obj(o) => EmVal::Object(o.class, o.ptr),
                };
                // The module takes ownership of the new handle.
                Val::I32(self.store.data_mut().new_handle(v) as i32)
            }
            (TypeInfo::Class(target), Arg::Obj(o)) => Val::I32(self.upcast(*o, *target)? as i32),
            (info, a) => {
                return Err(err(format!(
                    "cannot pass {a:?} as {} ({info:?})",
                    self.type_name(ty)
                )));
            }
        };
        Ok((v, None))
    }

    fn decode_ret(&mut self, ty: u32, v: &Val) -> WResult<Ret> {
        let info = self
            .store
            .data()
            .types
            .get(&ty)
            .cloned()
            .ok_or_else(|| err(format!("unknown return type {ty}")))?;
        Ok(match info {
            TypeInfo::Void => Ret::Void,
            TypeInfo::Bool => Ret::Num(f64::from(v.unwrap_i32() != 0)),
            TypeInfo::Int { signed, .. } => {
                let x = v.unwrap_i32();
                Ret::Num(if signed { x as f64 } else { x as u32 as f64 })
            }
            TypeInfo::BigInt => Ret::BigInt(v.unwrap_i64()),
            TypeInfo::Float { .. } => Ret::Num(match v {
                Val::F32(b) => f32::from_bits(*b) as f64,
                Val::F64(b) => f64::from_bits(*b),
                _ => return Err(err("float return mismatch")),
            }),
            TypeInfo::StdString => {
                let p = v.unwrap_i32() as u32;
                let len = self.read_u32(p)?;
                let bytes = self.read(p + 4, len)?;
                self.free.call(&mut self.store, p as i32)?;
                Ret::Val(EmVal::Str(String::from_utf8_lossy(&bytes).into_owned()))
            }
            TypeInfo::Emval => {
                let h = v.unwrap_i32() as u32;
                Ret::Val(self.store.data_mut().take_handle(h)?)
            }
            TypeInfo::Class(raw) => Ret::Obj(Object {
                class: raw,
                ptr: v.unwrap_i32() as u32,
            }),
            other => return Err(err(format!("unsupported return type {other:?}"))),
        })
    }

    fn invoke(&mut self, c: &Callable, this: Option<Object>, args: &[Arg]) -> WResult<Ret> {
        let nparams = c.arg_types.len() - 2;
        if args.len() != nparams {
            return Err(err(format!("expected {nparams} args, got {}", args.len())));
        }
        // invoker(context, [this], args...)
        let mut wire = vec![Val::I32(c.context as i32)];
        if c.is_method {
            let this = this.ok_or_else(|| err("method call without `this`"))?;
            let target = match self.store.data().types.get(&c.arg_types[1]) {
                Some(TypeInfo::Class(t)) => *t,
                _ => return Err(err("method `this` is not a class")),
            };
            wire.push(Val::I32(self.upcast(this, target)? as i32));
        }
        let mut to_free = Vec::new();
        let live_before = self.store.data().live_handle_ids();
        let result = (|| {
            for (i, a) in args.iter().enumerate() {
                let (v, f) = self.encode_arg(c.arg_types[i + 2], a)?;
                wire.push(v);
                to_free.extend(f);
            }
            // The wasm function type is authoritative for the wire format (the
            // Embind signature string does not distinguish 64-bit ints here).
            let f = self.table_func(c.invoker)?;
            let fty = f.ty(&self.store);
            tracing::trace!(?wire, "ltsm invoke");
            for (v, pty) in wire.iter_mut().zip(fty.params()) {
                *v = match (pty, &*v) {
                    (ValType::I64, Val::I32(x)) => Val::I64(*x as u32 as i64),
                    (ValType::I32, Val::I64(x)) => Val::I32(*x as i32),
                    _ => *v,
                };
            }
            let mut out: Vec<Val> = fty
                .results()
                .map(|t| match t {
                    ValType::I64 => Val::I64(0),
                    ValType::F32 => Val::F32(0),
                    ValType::F64 => Val::F64(0),
                    _ => Val::I32(0),
                })
                .collect();
            self.store.data_mut().last_exception = None;
            f.call(&mut self.store, &wire, &mut out)?;
            match out.first() {
                Some(v) => self.decode_ret(c.arg_types[0], v),
                None => Ok(Ret::Void),
            }
        })();
        for p in to_free {
            self.free.call(&mut self.store, p as i32)?;
        }
        if result.is_err() {
            // Note: C++ heap allocations of the trapped frames leak, exactly as
            // they do when the browser unwinds a JS exception through wasm.
            self.store.data_mut().drop_handles_except(&live_before);
        }
        result.map_err(|e| match self.store.data().last_exception.clone() {
            Some(msg) => err(msg),
            None => e,
        })
    }

    fn find_method(&self, mut cls: u32, name: &str) -> WResult<Callable> {
        loop {
            let info = self
                .store
                .data()
                .classes
                .get(&cls)
                .ok_or_else(|| err(format!("unknown class {cls}")))?;
            if let Some(m) = info.methods.get(name) {
                return Ok(m.clone());
            }
            if info.base == 0 {
                return Err(err(format!("no method {name}")));
            }
            cls = info.base;
        }
    }

    pub(crate) fn call_method(&mut self, obj: Object, name: &str, args: &[Arg]) -> WResult<Ret> {
        let m = self.find_method(obj.class, name)?;
        self.invoke(&m, Some(obj), args)
    }

    pub(crate) fn call_static(&mut self, class: u32, name: &str, args: &[Arg]) -> WResult<Ret> {
        let m = self
            .store
            .data()
            .classes
            .get(&class)
            .and_then(|c| c.statics.get(name))
            .cloned()
            .ok_or_else(|| err(format!("no static {name}")))?;
        self.invoke(&m, None, args)
    }

    pub(crate) fn call_function(&mut self, name: &str, args: &[Arg]) -> WResult<Ret> {
        let f = self
            .store
            .data()
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| err(format!("no function {name}")))?;
        self.invoke(&f, None, args)
    }

    pub(crate) fn construct(&mut self, class: u32, args: &[Arg]) -> WResult<Object> {
        let c = self
            .store
            .data()
            .classes
            .get(&class)
            .and_then(|c| c.ctors.get(&args.len()))
            .cloned()
            .ok_or_else(|| err(format!("no {}-arg constructor", args.len())))?;
        match self.invoke(&c, None, args)? {
            Ret::Obj(o) => Ok(o),
            Ret::Num(p) => Ok(Object { class, ptr: p as u32 }),
            r => Err(err(format!("constructor returned {r:?}"))),
        }
    }

    /// Delete a C++ object via its class's raw destructor.
    pub(crate) fn delete(&mut self, obj: Object) -> WResult<()> {
        let dtor = self
            .store
            .data()
            .classes
            .get(&obj.class)
            .map(|c| c.destructor)
            .unwrap_or(0);
        if dtor == 0 || obj.ptr == 0 {
            return Ok(());
        }
        let f = self.table_func(dtor)?;
        f.call(&mut self.store, &[Val::I32(obj.ptr as i32)], &mut [])
    }

    /// Human-readable dump of every registered class and method signature.
    pub(crate) fn describe(&self) -> String {
        use std::fmt::Write;
        let h = self.store.data();
        let tn = |id: &u32| h.type_names.get(id).cloned().unwrap_or_else(|| format!("#{id}"));
        let sig = |c: &Callable| {
            let params: Vec<String> = c.arg_types[2..].iter().map(tn).collect();
            format!("({}) -> {}", params.join(", "), tn(&c.arg_types[0]))
        };
        let mut out = String::new();
        for (name, f) in &h.functions {
            let _ = writeln!(out, "fn {name}{}", sig(f));
        }
        let mut classes: Vec<_> = h.classes.values().collect();
        classes.sort_by(|a, b| a.name.cmp(&b.name));
        for c in classes {
            let base = h.classes.get(&c.base).map(|b| b.name.clone()).unwrap_or_default();
            let _ = writeln!(out, "class {} : {base}", c.name);
            for (n, m) in &c.ctors {
                let _ = writeln!(out, "  new/{n}{}", sig(m));
            }
            for (n, m) in &c.statics {
                let _ = writeln!(out, "  static {n}{}", sig(m));
            }
            for (n, m) in &c.methods {
                let _ = writeln!(out, "  {n}{}", sig(m));
            }
        }
        out
    }
}
