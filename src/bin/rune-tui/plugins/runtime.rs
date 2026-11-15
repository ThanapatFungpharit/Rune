//! The sandbox one plugin runs in: a wasmtime instance with a fuel budget
//! per call, a memory cap and a single import (`rune.log`).
//!
//! ABI (see `docs/PLUGINS.md`): the module exports `memory`,
//! `rune_api_version() -> i32` (`major << 16 | minor`), `rune_alloc(len) ->
//! ptr` (a buffer for the next event) and `rune_call(ptr, len) -> i64` (the
//! reply's `ptr << 32 | len`, valid until the next call). Everything that
//! goes wrong is an error value: a trap, running out of fuel or memory, a
//! pointer outside memory, even a panic inside wasmtime.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

use rune_plugin::ApiVersion;
use wasmtime::{
    Caller, Config, Engine, Extern, ExternType, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, Trap, TypedFunc, ValType,
};

/// What one plugin may use.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Fuel (roughly, wasm instructions) per event.
    pub fuel: u64,
    /// Linear memory, bytes.
    pub memory: usize,
    /// Largest reply, bytes.
    pub reply: usize,
    /// Largest module file, bytes.
    pub module: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            fuel: 200_000_000,
            memory: 64 << 20,
            reply: 1 << 20,
            module: 16 << 20,
        }
    }
}

/// Log lines a plugin may write per event, and their length.
const LOGS_PER_CALL: u32 = 200;
const LOG_LINE_MAX: usize = 4096;
/// Elements per table (function tables of real plugins hold a few hundred).
const TABLE_ELEMENTS: usize = 100_000;

struct HostState {
    limits: StoreLimits,
    id: String,
    logs: u32,
}

pub struct Sandbox {
    store: Store<HostState>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    call: TypedFunc<(i32, i32), i64>,
    limits: Limits,
    /// The API version the module itself reports.
    pub api: ApiVersion,
}

/// One engine for every plugin (fuel metering on); not `ltsm`'s.
fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.consume_fuel(true);
        Engine::new(&config).expect("plugin engine configuration is valid")
    })
}

/// The imports the host provides; anything else is refused before linking.
const IMPORTS: &[(&str, &str)] = &[("rune", "log")];

fn describe(e: &wasmtime::Error) -> String {
    match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => "took too long (ran out of fuel)".into(),
        Some(t) => format!("crashed ({t})"),
        None => format!("{e:#}"),
    }
}

impl Sandbox {
    /// Compile and instantiate a module (blocking: run off the UI thread).
    pub fn load(id: &str, wasm: &[u8], limits: Limits) -> Result<Sandbox, String> {
        if wasm.len() > limits.module {
            return Err(format!("module is larger than {} MB", limits.module >> 20));
        }
        catch_unwind(AssertUnwindSafe(|| Self::load_inner(id, wasm, limits)))
            .unwrap_or_else(|_| Err("loading it crashed the runtime".into()))
    }

    /// Whether a module could be loaded, without running any of it: it
    /// compiles, imports only what rune provides and exports the ABI
    /// (blocking; used before installing a plugin).
    pub fn check(wasm: &[u8], limits: Limits) -> Result<(), String> {
        if wasm.len() > limits.module {
            return Err(format!("module is larger than {} MB", limits.module >> 20));
        }
        catch_unwind(AssertUnwindSafe(|| {
            let module = compile(wasm)?;
            let func = |name: &str, params: &[fn(&ValType) -> bool], results: &[fn(&ValType) -> bool]| {
                let ok = match module.get_export(name) {
                    Some(ExternType::Func(f)) => {
                        f.params().len() == params.len()
                            && f.params().zip(params).all(|(t, is)| is(&t))
                            && f.results().len() == results.len()
                            && f.results().zip(results).all(|(t, is)| is(&t))
                    }
                    _ => false,
                };
                if ok {
                    Ok(())
                } else {
                    Err(format!("incompatible: it does not export {name} as rune expects"))
                }
            };
            if !matches!(module.get_export("memory"), Some(ExternType::Memory(_))) {
                return Err("incompatible: it exports no memory".into());
            }
            func("rune_api_version", &[], &[ValType::is_i32])?;
            func("rune_alloc", &[ValType::is_i32], &[ValType::is_i32])?;
            func(
                "rune_call",
                &[ValType::is_i32, ValType::is_i32],
                &[ValType::is_i64],
            )
        }))
        .unwrap_or_else(|_| Err("checking it crashed the runtime".into()))
    }

    fn load_inner(id: &str, wasm: &[u8], limits: Limits) -> Result<Sandbox, String> {
        let engine = engine();
        let module = compile(wasm)?;
        let mut linker: Linker<HostState> = Linker::new(engine);
        linker
            .func_wrap("rune", "log", host_log)
            .map_err(|e| format!("linking: {e:#}"))?;
        let state = HostState {
            limits: StoreLimitsBuilder::new()
                .memory_size(limits.memory)
                .memories(1)
                .tables(4)
                // Table elements live outside linear memory, so the memory
                // cap does not bound them.
                .table_elements(TABLE_ELEMENTS)
                .instances(1)
                .build(),
            id: id.to_owned(),
            logs: 0,
        };
        let mut store = Store::new(engine, state);
        store.limiter(|s| &mut s.limits);
        // Start-up code (data segments, a start function) is metered too.
        store.set_fuel(limits.fuel).map_err(|e| describe(&e))?;
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| format!("could not start: {}", describe(&e)))?;
        let missing = |what: &str, e: wasmtime::Error| format!("incompatible: export {what}: {e:#}");
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or("incompatible: it exports no memory")?;
        let version = instance
            .get_typed_func::<(), i32>(&mut store, "rune_api_version")
            .map_err(|e| missing("rune_api_version", e))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "rune_alloc")
            .map_err(|e| missing("rune_alloc", e))?;
        let call = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, "rune_call")
            .map_err(|e| missing("rune_call", e))?;
        store.set_fuel(limits.fuel).map_err(|e| describe(&e))?;
        let api = version
            .call(&mut store, ())
            .map(ApiVersion::from_packed)
            .map_err(|e| format!("rune_api_version {}", describe(&e)))?;
        Ok(Sandbox {
            store,
            memory,
            alloc,
            call,
            limits,
            api,
        })
    }

    /// Hand the plugin one event (JSON) and return its reply (JSON bytes).
    /// After an error the sandbox must not be used again.
    pub fn call(&mut self, input: &[u8]) -> Result<Vec<u8>, String> {
        catch_unwind(AssertUnwindSafe(|| self.call_inner(input)))
            .unwrap_or_else(|_| Err("crashed the runtime".into()))
    }

    fn call_inner(&mut self, input: &[u8]) -> Result<Vec<u8>, String> {
        let len = i32::try_from(input.len()).map_err(|_| "event too large".to_owned())?;
        self.store.data_mut().logs = 0;
        self.store.set_fuel(self.limits.fuel).map_err(|e| describe(&e))?;
        let ptr = self.alloc.call(&mut self.store, len).map_err(|e| describe(&e))?;
        let at = checked_range(
            ptr as u32 as u64,
            input.len() as u64,
            self.memory.data_size(&self.store),
        )
        .ok_or("gave an input buffer outside its memory")?;
        self.memory.data_mut(&mut self.store)[at].copy_from_slice(input);
        let packed = self
            .call
            .call(&mut self.store, (ptr, len))
            .map_err(|e| describe(&e))?;
        let (out_ptr, out_len) = ((packed as u64 >> 32), (packed as u64 & 0xffff_ffff));
        if out_len > self.limits.reply as u64 {
            return Err(format!("replied with more than {} KB", self.limits.reply >> 10));
        }
        let at = checked_range(out_ptr, out_len, self.memory.data_size(&self.store))
            .ok_or("replied from outside its memory")?;
        Ok(self.memory.data(&self.store)[at].to_vec())
    }
}

/// Compile a module and refuse imports rune does not provide.
fn compile(wasm: &[u8]) -> Result<Module, String> {
    let module = Module::new(engine(), wasm).map_err(|e| format!("not a valid WebAssembly module: {e:#}"))?;
    for import in module.imports() {
        let known = IMPORTS.contains(&(import.module(), import.name()));
        if !known || !matches!(import.ty(), ExternType::Func(_)) {
            return Err(format!(
                "needs {}.{}, which this version of rune does not provide",
                import.module(),
                import.name()
            ));
        }
    }
    Ok(module)
}

fn checked_range(ptr: u64, len: u64, size: usize) -> Option<std::ops::Range<usize>> {
    let end = ptr.checked_add(len)?;
    (end <= size as u64).then_some(ptr as usize..end as usize)
}

/// `rune.log(level, ptr, len)`: a line in the diagnostics log. Bad
/// pointers and floods are ignored rather than trapping.
fn host_log(mut caller: Caller<'_, HostState>, level: i32, ptr: i32, len: i32) {
    let st = caller.data_mut();
    st.logs += 1;
    if st.logs > LOGS_PER_CALL {
        return;
    }
    let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
        return;
    };
    let len = (len as u32 as usize).min(LOG_LINE_MAX) as u64;
    let Some(at) = checked_range(ptr as u32 as u64, len, mem.data_size(&caller)) else {
        return;
    };
    let text = String::from_utf8_lossy(&mem.data(&caller)[at]).into_owned();
    let id = &caller.data().id;
    match level {
        0 => tracing::error!("{id}: {text}"),
        1 => tracing::warn!("{id}: {text}"),
        2 => tracing::info!("{id}: {text}"),
        _ => tracing::debug!("{id}: {text}"),
    }
}
