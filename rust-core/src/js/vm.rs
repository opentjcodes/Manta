use std::collections::HashMap;
use std::sync::Arc;

use super::compiler::JsBytecodeCompiler;
use super::value::{abstract_eq, CompiledJsProgram, JsValue, OpCode};
use crate::clipboard::ClipboardSandbox;
use crate::cpp_bridge::{
    base64_decode, base64_encode, fast_f64_dot, fast_f64_max, fast_f64_min, fast_f64_scale_add,
    fast_f64_sort, fast_f64_sum, find_substr, json_escape_str, now_us, regex_find,
};
use crate::crypto_vault::{generate_volatile_entropy_32, sha256};
use crate::dom_arena::{ZeroCopyDomArena, NO_NODE};
use crate::downloads_engine::MantaDownloadManager;
use crate::module_bus::ModuleBus;
use crate::net_stream::StreamingNetworkPipe;
use crate::site_isolation::SiteIsolationModule;
use crate::storage_engine::MantaStorageEngine;

const MAX_VM_SCOPES: usize = 32_768;
const MAX_VM_HEAP_CONTAINERS: usize = 16_384;

#[derive(Debug, Clone)]
struct ScopeFrame {
    parent: Option<u32>,
    vars: HashMap<String, JsValue>,
}

#[derive(Debug, Clone)]
enum Callable {
    UserClosure { fn_idx: u16, env_scope: u32 },
    Builtin(&'static str),
    BoundResolver { promise_oid: u32, is_reject: bool },
}

#[derive(Debug, Clone)]
struct CallFrame {
    return_pc: usize,
    prev_scope: u32,
    try_depth: usize,
    async_promise_oid: Option<u32>,
}

#[derive(Debug, Clone)]
struct TryHandler {
    catch_pc: usize,
    stack_len: usize,
    call_depth: usize,
    scope_id: u32,
}

#[derive(Debug, Clone)]
struct AsyncSuspension {
    pc: usize,
    stack: Vec<JsValue>,
    scope_id: u32,
    call_frames: Vec<CallFrame>,
    try_stack: Vec<TryHandler>,
}

#[derive(Debug, Clone)]
enum Microtask {
    Callback {
        cb: JsValue,
        arg: JsValue,
        resolve_promise_oid: Option<u32>,
    },
    ResumeAsync {
        susp: AsyncSuspension,
        val: JsValue,
        is_err: bool,
    },
}

#[derive(Debug, Clone)]
struct RegexEntry {
    pattern: String,
    flags: String,
    last_index: usize,
}

#[derive(Debug, Clone)]
struct PromiseRecord {
    fulfilled: Option<bool>,
    value: JsValue,
    then_waiters: Vec<(Option<JsValue>, Option<JsValue>, u32)>,
    async_waiters: Vec<AsyncSuspension>,
}

pub struct MantaJsSandbox {
    scopes: Vec<Option<ScopeFrame>>,
    free_scope_slots: Vec<u32>,
    objects: Vec<Option<HashMap<String, JsValue>>>,
    free_obj_slots: Vec<u32>,
    arrays: Vec<Option<Vec<JsValue>>>,
    free_arr_slots: Vec<u32>,
    callables: Vec<Callable>,
    pinned_callables: Vec<u32>,
    regexes: Vec<RegexEntry>,
    maps: Vec<Vec<(String, JsValue)>>,
    sets: Vec<Vec<JsValue>>,
    typed_arrays: Vec<Vec<f64>>,
    promises: HashMap<u32, PromiseRecord>,
    timers: Vec<(u64, u64, Microtask)>,
    timer_seq: u64,
    gc_alloc_counter: usize,
    gc_threshold: usize,
    fuel_remaining: u32,
    max_fuel: u32,
    max_time_budget_us: u64,
    script_start_us: u64,
    vm_boot_us: u64,
    rng_state: u64,
    pub console_logs: Vec<String>,
}

impl MantaJsSandbox {
    pub fn new(max_fuel: u32) -> Self {
        let now = now_us();
        let mut vm = Self {
            scopes: vec![Some(ScopeFrame {
                parent: None,
                vars: HashMap::with_capacity(64),
            })],
            free_scope_slots: Vec::new(),
            objects: Vec::with_capacity(32),
            free_obj_slots: Vec::new(),
            arrays: Vec::with_capacity(16),
            free_arr_slots: Vec::new(),
            callables: Vec::with_capacity(64),
            pinned_callables: Vec::new(),
            regexes: Vec::new(),
            maps: Vec::new(),
            sets: Vec::new(),
            typed_arrays: Vec::new(),
            promises: HashMap::new(),
            timers: Vec::new(),
            timer_seq: 1,
            gc_alloc_counter: 0,
            gc_threshold: 1024,
            fuel_remaining: max_fuel,
            max_fuel,
            max_time_budget_us: 25_000,
            script_start_us: now,
            vm_boot_us: now,
            rng_state: now ^ 0x9E37_79B9_7F4A_7C15,
            console_logs: Vec::new(),
        };
        vm.install_default_globals();
        vm
    }

    pub fn set_time_budget_us(&mut self, budget_us: u64) {
        self.max_time_budget_us = budget_us;
    }

    #[inline]
    fn scope_ref(&self, sid: u32) -> Option<&ScopeFrame> {
        self.scopes.get(sid as usize).and_then(|s| s.as_ref())
    }

    #[inline]
    fn scope_mut(&mut self, sid: u32) -> Option<&mut ScopeFrame> {
        self.scopes.get_mut(sid as usize).and_then(|s| s.as_mut())
    }

    fn alloc_scope(
        &mut self,
        parent: Option<u32>,
        vars: HashMap<String, JsValue>,
    ) -> Result<u32, &'static str> {
        self.gc_alloc_counter += 1;
        if let Some(slot) = self.free_scope_slots.pop() {
            self.scopes[slot as usize] = Some(ScopeFrame { parent, vars });
            return Ok(slot);
        }
        if self.scopes.len() >= MAX_VM_SCOPES {
            return Err("JS scope memory limit exceeded");
        }
        let id = self.scopes.len() as u32;
        self.scopes.push(Some(ScopeFrame { parent, vars }));
        Ok(id)
    }

    #[inline]
    fn obj_map(&self, oid: u32) -> Option<&HashMap<String, JsValue>> {
        self.objects.get(oid as usize).and_then(|o| o.as_ref())
    }

    #[inline]
    fn obj_map_mut(&mut self, oid: u32) -> Option<&mut HashMap<String, JsValue>> {
        self.objects.get_mut(oid as usize).and_then(|o| o.as_mut())
    }

    #[inline]
    fn arr_vec(&self, aid: u32) -> Option<&Vec<JsValue>> {
        self.arrays.get(aid as usize).and_then(|a| a.as_ref())
    }

    #[inline]
    fn arr_vec_mut(&mut self, aid: u32) -> Option<&mut Vec<JsValue>> {
        self.arrays.get_mut(aid as usize).and_then(|a| a.as_mut())
    }

    fn install_default_globals(&mut self) {
        for name in [
            "parseInt",
            "parseFloat",
            "isNaN",
            "isFinite",
            "Number",
            "String",
            "Boolean",
            "queueMicrotask",
            "setTimeout",
            "clearTimeout",
            "encodeURIComponent",
            "decodeURIComponent",
            "btoa",
            "atob",
            "structuredClone",
            "fetch",
            "XMLHttpRequest",
            "AbortController",
            "Headers",
            "FormData",
            "Event",
            "CustomEvent",
            "MutationObserver",
            "ResizeObserver",
            "IntersectionObserver",
            "Map",
            "Set",
            "RegExp",
            "Promise",
            "URL",
            "URLSearchParams",
            "TextEncoder",
            "TextDecoder",
            "Float64Array",
            "Int32Array",
            "Uint8Array",
            "Error",
        ] {
            self.register_builtin_fn(name, name);
        }

        for ns in [
            "document",
            "window",
            "navigator",
            "location",
            "console",
            "performance",
            "crypto",
            "localStorage",
            "sessionStorage",
            "indexedDB",
            "MantaFS",
            "MantaForm",
            "MantaDownloads",
            "Math",
            "JSON",
            "Object",
            "Array",
            "Date",
        ] {
            let mut map = HashMap::new();
            map.insert("__manta_ns".to_string(), JsValue::Str(ns.to_string()));
            if ns == "navigator" {
                map.insert("webdriver".to_string(), JsValue::Bool(false));
                map.insert("cookieEnabled".to_string(), JsValue::Bool(true));
                map.insert(
                    "userAgent".to_string(),
                    JsValue::Str("Manta/4.0 (Native Rust+C++ Engine; W3C Standard)".to_string()),
                );
                let clip_id = self.alloc_obj_map({
                    let mut cm = HashMap::new();
                    cm.insert("__manta_ns".to_string(), JsValue::Str("clipboard".to_string()));
                    cm
                });
                map.insert("clipboard".to_string(), JsValue::Object(clip_id));
            } else if ns == "window" {
                map.insert("isSecureContext".to_string(), JsValue::Bool(true));
                map.insert("innerWidth".to_string(), JsValue::Number(390.0));
                map.insert("innerHeight".to_string(), JsValue::Number(844.0));
                map.insert("scrollY".to_string(), JsValue::Number(0.0));
            } else if ns == "crypto" {
                let subtle_id = self.alloc_obj_map({
                    let mut sm = HashMap::new();
                    sm.insert("__manta_ns".to_string(), JsValue::Str("crypto_subtle".to_string()));
                    sm
                });
                map.insert("subtle".to_string(), JsValue::Object(subtle_id));
            }
            let oid = self.alloc_obj_map(map);
            if let Some(global) = self.scope_mut(0) {
                global.vars.insert(ns.to_string(), JsValue::Object(oid));
            }
        }
    }

    fn register_builtin_fn(&mut self, global_name: &'static str, id: &'static str) {
        let cid = self.callables.len() as u32;
        self.callables.push(Callable::Builtin(id));
        if let Some(global) = self.scope_mut(0) {
            global
                .vars
                .insert(global_name.to_string(), JsValue::Fn(cid));
        }
    }

    fn alloc_obj_map(&mut self, map: HashMap<String, JsValue>) -> u32 {
        self.gc_alloc_counter += 1;
        if let Some(slot) = self.free_obj_slots.pop() {
            self.objects[slot as usize] = Some(map);
            return slot;
        }
        let id = self.objects.len() as u32;
        self.objects.push(Some(map));
        id
    }

    fn alloc_obj(&mut self) -> Result<u32, &'static str> {
        self.gc_alloc_counter += 1;
        if let Some(slot) = self.free_obj_slots.pop() {
            self.objects[slot as usize] = Some(HashMap::new());
            return Ok(slot);
        }
        if self.objects.len() >= MAX_VM_HEAP_CONTAINERS {
            return Err("JS heap object limit reached");
        }
        let id = self.objects.len() as u32;
        self.objects.push(Some(HashMap::new()));
        Ok(id)
    }

    fn alloc_arr(&mut self, items: Vec<JsValue>) -> Result<u32, &'static str> {
        self.gc_alloc_counter += 1;
        if let Some(slot) = self.free_arr_slots.pop() {
            self.arrays[slot as usize] = Some(items);
            return Ok(slot);
        }
        if self.arrays.len() >= MAX_VM_HEAP_CONTAINERS {
            return Err("JS heap array limit reached");
        }
        let id = self.arrays.len() as u32;
        self.arrays.push(Some(items));
        Ok(id)
    }

    fn maybe_collect_garbage(
        &mut self,
        stack: &[JsValue],
        current_scope: u32,
        call_frames: &[CallFrame],
        try_stack: &[TryHandler],
    ) {
        if self.gc_alloc_counter < self.gc_threshold {
            return;
        }
        self.gc_alloc_counter = 0;

        let mut marked_objs = vec![false; self.objects.len()];
        let mut marked_arrs = vec![false; self.arrays.len()];
        let mut marked_scopes = vec![false; self.scopes.len()];
        let mut scope_queue: Vec<u32> = Vec::with_capacity(32);
        let mut work: Vec<JsValue> = Vec::with_capacity(128);

        scope_queue.push(0);
        scope_queue.push(current_scope);
        for cf in call_frames {
            scope_queue.push(cf.prev_scope);
            if let Some(poid) = cf.async_promise_oid {
                work.push(JsValue::Object(poid));
            }
        }
        for th in try_stack {
            scope_queue.push(th.scope_id);
        }
        for &cid in &self.pinned_callables {
            work.push(JsValue::Fn(cid));
        }
        for v in stack {
            work.push(v.clone());
        }
        for (&poid, rec) in &self.promises {
            work.push(JsValue::Object(poid));
            work.push(rec.value.clone());
            for (ok, err, child) in &rec.then_waiters {
                if let Some(v) = ok {
                    work.push(v.clone());
                }
                if let Some(v) = err {
                    work.push(v.clone());
                }
                work.push(JsValue::Object(*child));
            }
            for susp in &rec.async_waiters {
                scope_queue.push(susp.scope_id);
                for cf in &susp.call_frames {
                    scope_queue.push(cf.prev_scope);
                }
                for sv in &susp.stack {
                    work.push(sv.clone());
                }
            }
        }
        for (_, _, task) in &self.timers {
            match task {
                Microtask::Callback {
                    cb,
                    arg,
                    resolve_promise_oid,
                } => {
                    work.push(cb.clone());
                    work.push(arg.clone());
                    if let Some(p) = resolve_promise_oid {
                        work.push(JsValue::Object(*p));
                    }
                }
                Microtask::ResumeAsync { susp, val, .. } => {
                    work.push(val.clone());
                    scope_queue.push(susp.scope_id);
                    for cf in &susp.call_frames {
                        scope_queue.push(cf.prev_scope);
                    }
                    for sv in &susp.stack {
                        work.push(sv.clone());
                    }
                }
            }
        }

        while !scope_queue.is_empty() || !work.is_empty() {
            while let Some(sid) = scope_queue.pop() {
                let idx = sid as usize;
                if idx < marked_scopes.len() && !marked_scopes[idx] {
                    marked_scopes[idx] = true;
                    if let Some(Some(frame)) = self.scopes.get(idx) {
                        if let Some(p) = frame.parent {
                            scope_queue.push(p);
                        }
                        for v in frame.vars.values() {
                            work.push(v.clone());
                        }
                    }
                }
            }

            while let Some(val) = work.pop() {
                match val {
                    JsValue::Object(oid) => {
                        let idx = oid as usize;
                        if idx < marked_objs.len() && !marked_objs[idx] {
                            marked_objs[idx] = true;
                            if let Some(Some(map)) = self.objects.get(idx) {
                                for child in map.values() {
                                    work.push(child.clone());
                                }
                            }
                        }
                    }
                    JsValue::Array(aid) => {
                        let idx = aid as usize;
                        if idx < marked_arrs.len() && !marked_arrs[idx] {
                            marked_arrs[idx] = true;
                            if let Some(Some(arr)) = self.arrays.get(idx) {
                                for child in arr {
                                    work.push(child.clone());
                                }
                            }
                        }
                    }
                    JsValue::Fn(cid) => {
                        if let Some(Callable::UserClosure { env_scope, .. }) =
                            self.callables.get(cid as usize)
                        {
                            scope_queue.push(*env_scope);
                        }
                    }
                    _ => {}
                }
            }
        }

        for (idx, is_live) in marked_objs.into_iter().enumerate() {
            if !is_live && self.objects[idx].is_some() {
                self.objects[idx] = None;
                self.free_obj_slots.push(idx as u32);
            }
        }
        for (idx, is_live) in marked_arrs.into_iter().enumerate() {
            if !is_live && self.arrays[idx].is_some() {
                self.arrays[idx] = None;
                self.free_arr_slots.push(idx as u32);
            }
        }
        for (idx, is_live) in marked_scopes.into_iter().enumerate().skip(1) {
            if !is_live && self.scopes[idx].is_some() {
                self.scopes[idx] = None;
                self.free_scope_slots.push(idx as u32);
            }
        }
    }

    fn enqueue_timer(&mut self, delay_ms: u64, task: Microtask) {
        if self.timers.len() >= 2048 {
            return;
        }
        let seq = self.timer_seq;
        self.timer_seq += 1;
        self.timers.push((delay_ms, seq, task));
    }

    fn pop_next_timer(&mut self) -> Option<Microtask> {
        if self.timers.is_empty() {
            return None;
        }
        let mut best_idx = 0usize;
        for i in 1..self.timers.len() {
            if (self.timers[i].0, self.timers[i].1) < (self.timers[best_idx].0, self.timers[best_idx].1)
            {
                best_idx = i;
            }
        }
        Some(self.timers.remove(best_idx).2)
    }

    fn alloc_promise(&mut self, fulfilled: Option<bool>, value: JsValue) -> Result<u32, &'static str> {
        let mut m = HashMap::new();
        m.insert("__manta_ns".to_string(), JsValue::Str("promise".to_string()));
        let oid = self.alloc_obj()?;
        self.objects[oid as usize] = Some(m);
        self.promises.insert(
            oid,
            PromiseRecord {
                fulfilled,
                value,
                then_waiters: Vec::new(),
                async_waiters: Vec::new(),
            },
        );
        Ok(oid)
    }

    fn settle_promise(&mut self, promise_oid: u32, is_fulfilled: bool, val: JsValue) {
        let Some(rec) = self.promises.get_mut(&promise_oid) else {
            return;
        };
        if rec.fulfilled.is_some() {
            return;
        }
        rec.fulfilled = Some(is_fulfilled);
        rec.value = val.clone();
        let then_waiters = std::mem::take(&mut rec.then_waiters);
        let async_waiters = std::mem::take(&mut rec.async_waiters);

        for (on_ok, on_err, child_oid) in then_waiters {
            let cb_opt = if is_fulfilled { on_ok } else { on_err };
            if let Some(cb) = cb_opt {
                self.enqueue_timer(
                    0,
                    Microtask::Callback {
                        cb,
                        arg: val.clone(),
                        resolve_promise_oid: Some(child_oid),
                    },
                );
            } else {
                self.settle_promise(child_oid, is_fulfilled, val.clone());
            }
        }
        for susp in async_waiters {
            self.enqueue_timer(
                0,
                Microtask::ResumeAsync {
                    susp,
                    val: val.clone(),
                    is_err: !is_fulfilled,
                },
            );
        }
    }

    fn lookup_var(&self, mut scope_id: u32, name: &str) -> JsValue {
        loop {
            let Some(frame) = self.scope_ref(scope_id) else {
                return JsValue::Undefined;
            };
            if let Some(val) = frame.vars.get(name) {
                return val.clone();
            }
            match frame.parent {
                Some(p) => scope_id = p,
                None => return JsValue::Undefined,
            }
        }
    }

    fn store_var(&mut self, mut scope_id: u32, name: &str, val: JsValue) {
        loop {
            if let Some(frame) = self.scope_mut(scope_id) {
                if frame.vars.contains_key(name) {
                    frame.vars.insert(name.to_string(), val);
                    return;
                }
                if let Some(p) = frame.parent {
                    scope_id = p;
                    continue;
                }
            }
            break;
        }
        if let Some(global) = self.scope_mut(0) {
            global.vars.insert(name.to_string(), val);
        }
    }

    pub fn execute_script(
        &mut self,
        script: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
    ) -> Result<JsValue, &'static str> {
        let mut dummy_storage = MantaStorageEngine::new(64 * 1024, 256 * 1024);
        let mut dummy_downloads = MantaDownloadManager::new("", 1024 * 1024);
        self.execute_script_full(
            script,
            "https://manta.local",
            dom,
            clipboard,
            &mut dummy_storage,
            &mut dummy_downloads,
        )
    }

    pub fn execute_script_with_storage(
        &mut self,
        script: &str,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
    ) -> Result<JsValue, &'static str> {
        let mut dummy_downloads = MantaDownloadManager::new("", 10 * 1024 * 1024);
        self.execute_script_full(
            script,
            origin_url,
            dom,
            clipboard,
            storage,
            &mut dummy_downloads,
        )
    }

    pub fn execute_script_full(
        &mut self,
        script: &str,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<JsValue, &'static str> {
        let program = Arc::new(JsBytecodeCompiler::compile(script)?);
        self.fuel_remaining = self.max_fuel;
        self.script_start_us = now_us();

        let res = self.run_vm_loop(
            &program,
            0,
            Vec::new(),
            0,
            Vec::new(),
            Vec::new(),
            origin_url,
            dom,
            clipboard,
            storage,
            downloads,
        )?;

        let mut tasks_run = 0usize;
        while let Some(task) = self.pop_next_timer() {
            if tasks_run >= 2048 || self.is_time_budget_exceeded() {
                break;
            }
            tasks_run += 1;
            match task {
                Microtask::Callback {
                    cb,
                    arg,
                    resolve_promise_oid,
                } => {
                    let cb_res = self.invoke_callable_val(
                        &program,
                        &cb,
                        &[arg],
                        None,
                        origin_url,
                        dom,
                        clipboard,
                        storage,
                        downloads,
                    );
                    if let Some(child_oid) = resolve_promise_oid {
                        match cb_res {
                            Ok(v) => self.settle_promise(child_oid, true, v),
                            Err(e) => {
                                self.settle_promise(child_oid, false, JsValue::Str(e.to_string()))
                            }
                        }
                    }
                }
                Microtask::ResumeAsync {
                    mut susp,
                    val,
                    is_err,
                } => {
                    if is_err {
                        if let Some(handler) = susp.try_stack.pop() {
                            susp.stack.truncate(handler.stack_len);
                            susp.call_frames.truncate(handler.call_depth);
                            susp.scope_id = handler.scope_id;
                            susp.stack.push(val);
                            let _ = self.run_vm_loop(
                                &program,
                                handler.catch_pc,
                                susp.stack,
                                susp.scope_id,
                                susp.call_frames,
                                susp.try_stack,
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            );
                        } else if let Some(top_cf) = susp.call_frames.first() {
                            if let Some(poid) = top_cf.async_promise_oid {
                                self.settle_promise(poid, false, val);
                            }
                        }
                    } else {
                        susp.stack.push(val);
                        let _ = self.run_vm_loop(
                            &program,
                            susp.pc,
                            susp.stack,
                            susp.scope_id,
                            susp.call_frames,
                            susp.try_stack,
                            origin_url,
                            dom,
                            clipboard,
                            storage,
                            downloads,
                        );
                    }
                }
            }
        }

        Ok(res)
    }

    #[inline]
    fn is_time_budget_exceeded(&self) -> bool {
        self.max_time_budget_us > 0
            && now_us().saturating_sub(self.script_start_us) >= self.max_time_budget_us
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_callable_val(
        &mut self,
        program: &CompiledJsProgram,
        callee: &JsValue,
        args: &[JsValue],
        this_val: Option<JsValue>,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<JsValue, &'static str> {
        let cid = match *callee {
            JsValue::Fn(cid) => cid,
            JsValue::Object(oid) => {
                if let Some(JsValue::Fn(ctor_cid)) =
                    self.obj_map(oid).and_then(|m| m.get("__ctor")).cloned()
                {
                    ctor_cid
                } else {
                    return Ok(JsValue::Undefined);
                }
            }
            _ => return Ok(JsValue::Undefined),
        };

        let Some(c) = self.callables.get(cid as usize).cloned() else {
            return Ok(JsValue::Undefined);
        };
        match c {
            Callable::Builtin(name) => self.invoke_builtin_fn(
                program, name, args, origin_url, dom, clipboard, storage, downloads,
            ),
            Callable::BoundResolver {
                promise_oid,
                is_reject,
            } => {
                let val = args.first().cloned().unwrap_or(JsValue::Undefined);
                self.settle_promise(promise_oid, !is_reject, val);
                Ok(JsValue::Undefined)
            }
            Callable::UserClosure { fn_idx, env_scope } => {
                let def = program
                    .functions
                    .get(fn_idx as usize)
                    .ok_or("Missing function def")?
                    .clone();
                let mut vars = HashMap::with_capacity(def.params.len() + 2);
                if let Some(tv) = this_val {
                    vars.insert("this".to_string(), tv);
                }
                for (i, param) in def.params.iter().enumerate() {
                    let pname = program
                        .identifiers
                        .get(param.name_id as usize)
                        .cloned()
                        .unwrap_or_default();
                    if param.is_rest {
                        let rest_items = if i < args.len() {
                            args[i..].to_vec()
                        } else {
                            Vec::new()
                        };
                        let aid = self.alloc_arr(rest_items)?;
                        vars.insert(pname, JsValue::Array(aid));
                        break;
                    }
                    let mut arg_val = args.get(i).cloned().unwrap_or(JsValue::Undefined);
                    if matches!(arg_val, JsValue::Undefined) {
                        if let Some(d_pc) = param.default_pc {
                            arg_val = self.run_vm_loop(
                                program,
                                d_pc,
                                Vec::new(),
                                env_scope,
                                Vec::new(),
                                Vec::new(),
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                        }
                    }
                    vars.insert(pname, arg_val);
                }
                let new_scope = self.alloc_scope(Some(env_scope), vars)?;

                if def.is_async {
                    let p_oid = self.alloc_promise(None, JsValue::Undefined)?;
                    let cf = CallFrame {
                        return_pc: usize::MAX,
                        prev_scope: 0,
                        try_depth: 0,
                        async_promise_oid: Some(p_oid),
                    };
                    match self.run_vm_loop(
                        program,
                        def.entry_pc,
                        Vec::new(),
                        new_scope,
                        vec![cf],
                        Vec::new(),
                        origin_url,
                        dom,
                        clipboard,
                        storage,
                        downloads,
                    ) {
                        Ok(ret_val) => {
                            if let Some(rec) = self.promises.get(&p_oid) {
                                if rec.fulfilled.is_none() {
                                    self.settle_promise(p_oid, true, ret_val);
                                }
                            }
                            Ok(JsValue::Object(p_oid))
                        }
                        Err(err) => {
                            self.settle_promise(p_oid, false, JsValue::Str(err.to_string()));
                            Ok(JsValue::Object(p_oid))
                        }
                    }
                } else {
                    self.run_vm_loop(
                        program,
                        def.entry_pc,
                        Vec::new(),
                        new_scope,
                        Vec::new(),
                        Vec::new(),
                        origin_url,
                        dom,
                        clipboard,
                        storage,
                        downloads,
                    )
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_vm_loop(
        &mut self,
        program: &CompiledJsProgram,
        start_pc: usize,
        mut stack: Vec<JsValue>,
        mut current_scope: u32,
        mut call_frames: Vec<CallFrame>,
        mut try_stack: Vec<TryHandler>,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<JsValue, &'static str> {
        let mut last_popped = JsValue::Undefined;
        let mut pc = start_pc;

        while pc < program.instructions.len() {
            if self.fuel_remaining == 0 {
                return Err("JS execution paused: instruction fuel limit reached");
            }
            self.fuel_remaining -= 1;

            if (self.fuel_remaining & 0xFF) == 0 && self.is_time_budget_exceeded() {
                return Err("JS execution paused: fixed compute budget reached");
            }

            if self.gc_alloc_counter >= self.gc_threshold {
                self.maybe_collect_garbage(&stack, current_scope, &call_frames, &try_stack);
            }

            match &program.instructions[pc] {
                OpCode::LoadConst(idx) => {
                    stack.push(
                        program
                            .constants
                            .get(*idx as usize)
                            .cloned()
                            .unwrap_or(JsValue::Undefined),
                    );
                    pc += 1;
                }
                OpCode::LoadIdent(idx) => {
                    let name = program
                        .identifiers
                        .get(*idx as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    stack.push(self.lookup_var(current_scope, name));
                    pc += 1;
                }
                OpCode::DeclVar(idx) => {
                    let val = stack.last().cloned().unwrap_or(JsValue::Undefined);
                    if let Some(name) = program.identifiers.get(*idx as usize) {
                        if let Some(frame) = self.scope_mut(current_scope) {
                            frame.vars.insert(name.clone(), val);
                        }
                    }
                    pc += 1;
                }
                OpCode::StoreVar(idx) => {
                    let val = stack.last().cloned().unwrap_or(JsValue::Undefined);
                    if let Some(name) = program.identifiers.get(*idx as usize) {
                        self.store_var(current_scope, name, val);
                    }
                    pc += 1;
                }
                OpCode::MakeArray(count) => {
                    let n = *count as usize;
                    let start = stack.len().saturating_sub(n);
                    let items: Vec<JsValue> = stack.drain(start..).collect();
                    let aid = self.alloc_arr(items)?;
                    stack.push(JsValue::Array(aid));
                    pc += 1;
                }
                OpCode::ArrayPush => {
                    let item = stack.pop().unwrap_or(JsValue::Undefined);
                    if let Some(JsValue::Array(aid)) = stack.last().cloned() {
                        if let Some(arr) = self.arr_vec_mut(aid) {
                            arr.push(item);
                        }
                    }
                    pc += 1;
                }
                OpCode::ArraySpread => {
                    let spread_val = stack.pop().unwrap_or(JsValue::Undefined);
                    if let Some(JsValue::Array(dst_id)) = stack.last().cloned() {
                        let extra: Vec<JsValue> = match spread_val {
                            JsValue::Array(src_id) => {
                                self.arr_vec(src_id).cloned().unwrap_or_default()
                            }
                            JsValue::Set(sid) => {
                                self.sets.get(sid as usize).cloned().unwrap_or_default()
                            }
                            JsValue::Str(s) => {
                                s.chars().map(|c| JsValue::Str(c.to_string())).collect()
                            }
                            _ => Vec::new(),
                        };
                        if let Some(dst) = self.arr_vec_mut(dst_id) {
                            dst.extend(extra);
                        }
                    }
                    pc += 1;
                }
                OpCode::MakeObject(_) => {
                    let oid = self.alloc_obj()?;
                    stack.push(JsValue::Object(oid));
                    pc += 1;
                }
                OpCode::ObjectSpread => {
                    let src_val = stack.pop().unwrap_or(JsValue::Undefined);
                    if let Some(JsValue::Object(dst_oid)) = stack.last().cloned() {
                        if let JsValue::Object(src_oid) = src_val {
                            if let Some(src_map) = self.obj_map(src_oid).cloned() {
                                if let Some(dst_map) = self.obj_map_mut(dst_oid) {
                                    for (k, v) in src_map {
                                        if !k.starts_with("__") {
                                            dst_map.insert(k, v);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    pc += 1;
                }
                OpCode::MakeClosure(fn_idx) => {
                    let cid = self.callables.len() as u32;
                    self.callables.push(Callable::UserClosure {
                        fn_idx: *fn_idx,
                        env_scope: current_scope,
                    });
                    stack.push(JsValue::Fn(cid));
                    pc += 1;
                }
                OpCode::MakeRegex { pat_id, flags_id } => {
                    let pat = program
                        .identifiers
                        .get(*pat_id as usize)
                        .cloned()
                        .unwrap_or_default();
                    let flags = program
                        .identifiers
                        .get(*flags_id as usize)
                        .cloned()
                        .unwrap_or_default();
                    let rid = self.regexes.len() as u32;
                    self.regexes.push(RegexEntry {
                        pattern: pat,
                        flags,
                        last_index: 0,
                    });
                    stack.push(JsValue::Regex(rid));
                    pc += 1;
                }
                OpCode::BuildClass {
                    has_parent,
                    method_count,
                } => {
                    let mc = *method_count as usize;
                    let mut methods = Vec::with_capacity(mc);
                    for _ in 0..mc {
                        let fn_val = stack.pop().unwrap_or(JsValue::Undefined);
                        let key = stack.pop().unwrap_or(JsValue::Undefined).as_str();
                        methods.push((key, fn_val));
                    }
                    methods.reverse();

                    let parent_opt = if *has_parent {
                        Some(stack.pop().unwrap_or(JsValue::Undefined))
                    } else {
                        None
                    };

                    let proto_oid = self.alloc_obj()?;
                    if let Some(JsValue::Object(parent_cls_oid)) = parent_opt {
                        if let Some(parent_proto) = self
                            .obj_map(parent_cls_oid)
                            .and_then(|m| m.get("prototype"))
                            .cloned()
                        {
                            if let Some(pm) = self.obj_map_mut(proto_oid) {
                                pm.insert("__proto".to_string(), parent_proto);
                            }
                        }
                    }

                    let mut class_map = HashMap::new();
                    class_map.insert("__manta_class".to_string(), JsValue::Bool(true));
                    if let Some(p_val) = parent_opt {
                        class_map.insert("__parent_class".to_string(), p_val);
                    }
                    for (k, fn_val) in methods {
                        if k == "constructor" {
                            class_map.insert("__ctor".to_string(), fn_val);
                        } else if let Some(static_name) = k.strip_prefix("__static::") {
                            class_map.insert(static_name.to_string(), fn_val);
                        } else if let Some(pm) = self.obj_map_mut(proto_oid) {
                            pm.insert(k, fn_val);
                        }
                    }
                    class_map.insert("prototype".to_string(), JsValue::Object(proto_oid));
                    let class_oid = self.alloc_obj_map(class_map);
                    stack.push(JsValue::Object(class_oid));
                    pc += 1;
                }
                OpCode::DestructureObj { pairs } => {
                    let src_obj = stack.pop().unwrap_or(JsValue::Undefined);
                    let mut defaults = Vec::with_capacity(pairs.len());
                    for (_, _, has_def) in pairs.iter().rev() {
                        if *has_def {
                            defaults.push(Some(stack.pop().unwrap_or(JsValue::Undefined)));
                        } else {
                            defaults.push(None);
                        }
                    }
                    defaults.reverse();
                    for (idx, (prop_id, target_var_id, _)) in pairs.iter().enumerate() {
                        let prop_name = program
                            .identifiers
                            .get(*prop_id as usize)
                            .map(|s| s.as_str())
                            .unwrap_or("");
                        let mut v =
                            self.get_property(&src_obj, prop_name, origin_url, dom, storage);
                        if matches!(v, JsValue::Undefined) {
                            if let Some(Some(ref def_val)) = defaults.get(idx) {
                                v = def_val.clone();
                            }
                        }
                        if let Some(tname) = program.identifiers.get(*target_var_id as usize) {
                            if let Some(frame) = self.scope_mut(current_scope) {
                                frame.vars.insert(tname.clone(), v);
                            }
                        }
                    }
                    stack.push(src_obj);
                    pc += 1;
                }
                OpCode::DestructureArr { targets } => {
                    let src = stack.pop().unwrap_or(JsValue::Undefined);
                    let items = match src {
                        JsValue::Array(aid) => self.arr_vec(aid).cloned().unwrap_or_default(),
                        _ => Vec::new(),
                    };
                    for (i, (var_id, is_rest)) in targets.iter().enumerate() {
                        let v = if *is_rest {
                            let rem = if i < items.len() {
                                items[i..].to_vec()
                            } else {
                                Vec::new()
                            };
                            JsValue::Array(self.alloc_arr(rem)?)
                        } else {
                            items.get(i).cloned().unwrap_or(JsValue::Undefined)
                        };
                        if let Some(tname) = program.identifiers.get(*var_id as usize) {
                            if let Some(frame) = self.scope_mut(current_scope) {
                                frame.vars.insert(tname.clone(), v);
                            }
                        }
                        if *is_rest {
                            break;
                        }
                    }
                    stack.push(src);
                    pc += 1;
                }
                OpCode::GetProp(prop_id) => {
                    let obj_val = stack.pop().unwrap_or(JsValue::Undefined);
                    let prop = program
                        .identifiers
                        .get(*prop_id as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    stack.push(self.get_property(&obj_val, prop, origin_url, dom, storage));
                    pc += 1;
                }
                OpCode::GetPropOpt(prop_id) => {
                    let obj_val = stack.pop().unwrap_or(JsValue::Undefined);
                    if matches!(obj_val, JsValue::Undefined | JsValue::Null) {
                        stack.push(JsValue::Undefined);
                    } else {
                        let prop = program
                            .identifiers
                            .get(*prop_id as usize)
                            .map(|s| s.as_str())
                            .unwrap_or("");
                        stack.push(self.get_property(&obj_val, prop, origin_url, dom, storage));
                    }
                    pc += 1;
                }
                OpCode::SetProp(prop_id) => {
                    let val = stack.pop().unwrap_or(JsValue::Undefined);
                    let obj_val = stack.pop().unwrap_or(JsValue::Undefined);
                    let prop = program
                        .identifiers
                        .get(*prop_id as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    self.set_property(&obj_val, prop, val.clone(), origin_url, dom, storage);
                    stack.push(val);
                    pc += 1;
                }
                OpCode::DeleteProp(prop_id) => {
                    let obj_val = stack.pop().unwrap_or(JsValue::Undefined);
                    let prop = program
                        .identifiers
                        .get(*prop_id as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    if let JsValue::Object(oid) = obj_val {
                        if let Some(m) = self.obj_map_mut(oid) {
                            m.remove(prop);
                        }
                    }
                    stack.push(JsValue::Bool(true));
                    pc += 1;
                }
                OpCode::GetIndex => {
                    let idx_val = stack.pop().unwrap_or(JsValue::Undefined);
                    let target = stack.pop().unwrap_or(JsValue::Undefined);
                    let res = match target {
                        JsValue::Array(aid) => {
                            let i = idx_val.as_number();
                            if i >= 0.0 && i.is_finite() {
                                self.arr_vec(aid)
                                    .and_then(|a| a.get(i as usize))
                                    .cloned()
                                    .unwrap_or(JsValue::Undefined)
                            } else {
                                JsValue::Undefined
                            }
                        }
                        JsValue::TypedArray(tid) => {
                            let i = idx_val.as_number();
                            if i >= 0.0 && i.is_finite() {
                                self.typed_arrays
                                    .get(tid as usize)
                                    .and_then(|a| a.get(i as usize))
                                    .map(|&n| JsValue::Number(n))
                                    .unwrap_or(JsValue::Undefined)
                            } else {
                                JsValue::Undefined
                            }
                        }
                        JsValue::Object(oid) => self
                            .lookup_proto_prop(oid, &idx_val.as_str())
                            .unwrap_or(JsValue::Undefined),
                        JsValue::Str(s) => {
                            let i = idx_val.as_number() as usize;
                            s.chars()
                                .nth(i)
                                .map(|c| JsValue::Str(c.to_string()))
                                .unwrap_or(JsValue::Undefined)
                        }
                        _ => JsValue::Undefined,
                    };
                    stack.push(res);
                    pc += 1;
                }
                OpCode::SetIndex => {
                    let val = stack.pop().unwrap_or(JsValue::Undefined);
                    let idx_val = stack.pop().unwrap_or(JsValue::Undefined);
                    let target = stack.pop().unwrap_or(JsValue::Undefined);
                    match target {
                        JsValue::Array(aid) => {
                            let idx = idx_val.as_number() as usize;
                            if let Some(arr) = self.arr_vec_mut(aid) {
                                if idx < 8192 {
                                    while arr.len() <= idx {
                                        arr.push(JsValue::Undefined);
                                    }
                                    arr[idx] = val.clone();
                                }
                            }
                        }
                        JsValue::TypedArray(tid) => {
                            let idx = idx_val.as_number() as usize;
                            if let Some(arr) = self.typed_arrays.get_mut(tid as usize) {
                                if idx < arr.len() {
                                    arr[idx] = val.as_number();
                                }
                            }
                        }
                        JsValue::Object(oid) => {
                            if let Some(obj) = self.obj_map_mut(oid) {
                                obj.insert(idx_val.as_str(), val.clone());
                            }
                        }
                        _ => {}
                    }
                    stack.push(val);
                    pc += 1;
                }
                OpCode::KeysArray => {
                    let target = stack.pop().unwrap_or(JsValue::Undefined);
                    let keys: Vec<JsValue> = match target {
                        JsValue::Object(oid) => self
                            .obj_map(oid)
                            .map(|m| {
                                m.keys()
                                    .filter(|k| !k.starts_with("__"))
                                    .map(|k| JsValue::Str(k.clone()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        JsValue::Array(aid) => {
                            let len = self.arr_vec(aid).map(|a| a.len()).unwrap_or(0);
                            (0..len).map(|i| JsValue::Str(i.to_string())).collect()
                        }
                        _ => Vec::new(),
                    };
                    stack.push(JsValue::Array(self.alloc_arr(keys)?));
                    pc += 1;
                }
                OpCode::Add => {
                    let r = stack.pop().unwrap_or(JsValue::Undefined);
                    let l = stack.pop().unwrap_or(JsValue::Undefined);
                    let res = match (&l, &r) {
                        (JsValue::Str(_), _) | (_, JsValue::Str(_)) => {
                            JsValue::Str(format!("{}{}", l.as_str(), r.as_str()))
                        }
                        _ => JsValue::Number(l.as_number() + r.as_number()),
                    };
                    stack.push(res);
                    pc += 1;
                }
                OpCode::Sub => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Number(a - b));
                    pc += 1;
                }
                OpCode::Mul => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Number(a * b));
                    pc += 1;
                }
                OpCode::Div => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Number(a / b));
                    pc += 1;
                }
                OpCode::Mod => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Number(a % b));
                    pc += 1;
                }
                OpCode::Pow => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Number(a.powf(b)));
                    pc += 1;
                }
                OpCode::BitAnd => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_i32();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_i32();
                    stack.push(JsValue::Number((a & b) as f64));
                    pc += 1;
                }
                OpCode::BitOr => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_i32();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_i32();
                    stack.push(JsValue::Number((a | b) as f64));
                    pc += 1;
                }
                OpCode::BitXor => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_i32();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_i32();
                    stack.push(JsValue::Number((a ^ b) as f64));
                    pc += 1;
                }
                OpCode::BitNot => {
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_i32();
                    stack.push(JsValue::Number((!a) as f64));
                    pc += 1;
                }
                OpCode::Shl => {
                    let b = (stack.pop().unwrap_or(JsValue::Undefined).as_i32() as u32) & 31;
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_i32();
                    stack.push(JsValue::Number((a.wrapping_shl(b)) as f64));
                    pc += 1;
                }
                OpCode::Shr => {
                    let b = (stack.pop().unwrap_or(JsValue::Undefined).as_i32() as u32) & 31;
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_i32();
                    stack.push(JsValue::Number((a.wrapping_shr(b)) as f64));
                    pc += 1;
                }
                OpCode::UShr => {
                    let b = (stack.pop().unwrap_or(JsValue::Undefined).as_i32() as u32) & 31;
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_i32() as u32;
                    stack.push(JsValue::Number((a.wrapping_shr(b)) as f64));
                    pc += 1;
                }
                OpCode::Neg => {
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Number(-a));
                    pc += 1;
                }
                OpCode::Not => {
                    let a = stack.pop().unwrap_or(JsValue::Undefined).is_truthy();
                    stack.push(JsValue::Bool(!a));
                    pc += 1;
                }
                OpCode::TypeOf => {
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Str(a.type_of().to_string()));
                    pc += 1;
                }
                OpCode::InOp => {
                    let obj = stack.pop().unwrap_or(JsValue::Undefined);
                    let key = stack.pop().unwrap_or(JsValue::Undefined).as_str();
                    let found = match obj {
                        JsValue::Object(oid) => self.lookup_proto_prop(oid, &key).is_some(),
                        JsValue::Array(aid) => {
                            if let Ok(idx) = key.parse::<usize>() {
                                idx < self.arr_vec(aid).map(|a| a.len()).unwrap_or(0)
                            } else {
                                false
                            }
                        }
                        _ => false,
                    };
                    stack.push(JsValue::Bool(found));
                    pc += 1;
                }
                OpCode::InstanceOf => {
                    let cls = stack.pop().unwrap_or(JsValue::Undefined);
                    let inst = stack.pop().unwrap_or(JsValue::Undefined);
                    let res = match (inst, cls) {
                        (JsValue::Array(_), JsValue::Object(c_oid)) => self
                            .obj_map(c_oid)
                            .and_then(|m| m.get("__manta_ns"))
                            .map(|v| v.as_str() == "Array")
                            .unwrap_or(false),
                        (JsValue::Object(i_oid), JsValue::Object(c_oid)) => {
                            let target_proto = self
                                .obj_map(c_oid)
                                .and_then(|m| m.get("prototype"))
                                .cloned();
                            if let Some(JsValue::Object(tp_oid)) = target_proto {
                                self.is_in_proto_chain(i_oid, tp_oid)
                            } else {
                                false
                            }
                        }
                        _ => false,
                    };
                    stack.push(JsValue::Bool(res));
                    pc += 1;
                }
                OpCode::StrictEq => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Bool(a == b));
                    pc += 1;
                }
                OpCode::StrictNeq => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Bool(a != b));
                    pc += 1;
                }
                OpCode::Eq => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Bool(abstract_eq(&a, &b)));
                    pc += 1;
                }
                OpCode::Neq => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Bool(!abstract_eq(&a, &b)));
                    pc += 1;
                }
                OpCode::Lt => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Bool(a < b));
                    pc += 1;
                }
                OpCode::Lte => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Bool(a <= b));
                    pc += 1;
                }
                OpCode::Gt => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Bool(a > b));
                    pc += 1;
                }
                OpCode::Gte => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Bool(a >= b));
                    pc += 1;
                }
                OpCode::And => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(if a.is_truthy() { b } else { a });
                    pc += 1;
                }
                OpCode::Or => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(if a.is_truthy() { a } else { b });
                    pc += 1;
                }
                OpCode::Nullish => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(if matches!(a, JsValue::Undefined | JsValue::Null) {
                        b
                    } else {
                        a
                    });
                    pc += 1;
                }
                OpCode::Jump(target) => {
                    pc = *target;
                }
                OpCode::JumpIfFalse(target) => {
                    let cond = stack.pop().unwrap_or(JsValue::Undefined);
                    pc = if !cond.is_truthy() { *target } else { pc + 1 };
                }
                OpCode::JumpIfTrue(target) => {
                    let cond = stack.pop().unwrap_or(JsValue::Undefined);
                    pc = if cond.is_truthy() { *target } else { pc + 1 };
                }
                OpCode::PushTry(catch_pc) => {
                    try_stack.push(TryHandler {
                        catch_pc: *catch_pc,
                        stack_len: stack.len(),
                        call_depth: call_frames.len(),
                        scope_id: current_scope,
                    });
                    pc += 1;
                }
                OpCode::PopTry => {
                    try_stack.pop();
                    pc += 1;
                }
                OpCode::Throw => {
                    let err_val = stack.pop().unwrap_or(JsValue::Undefined);
                    if let Some(handler) = try_stack.pop() {
                        stack.truncate(handler.stack_len);
                        call_frames.truncate(handler.call_depth);
                        current_scope = handler.scope_id;
                        stack.push(err_val);
                        pc = handler.catch_pc;
                    } else {
                        return Err("Uncaught JS exception");
                    }
                }
                OpCode::Await => {
                    let awaited = stack.pop().unwrap_or(JsValue::Undefined);
                    if let JsValue::Object(poid) = awaited {
                        if let Some(rec) = self.promises.get_mut(&poid) {
                            match rec.fulfilled {
                                Some(true) => {
                                    stack.push(rec.value.clone());
                                    pc += 1;
                                    continue;
                                }
                                Some(false) => {
                                    let err = rec.value.clone();
                                    if let Some(handler) = try_stack.pop() {
                                        stack.truncate(handler.stack_len);
                                        call_frames.truncate(handler.call_depth);
                                        current_scope = handler.scope_id;
                                        stack.push(err);
                                        pc = handler.catch_pc;
                                        continue;
                                    }
                                    return Err("Unhandled async rejection");
                                }
                                None => {
                                    rec.async_waiters.push(AsyncSuspension {
                                        pc: pc + 1,
                                        stack,
                                        scope_id: current_scope,
                                        call_frames,
                                        try_stack,
                                    });
                                    return Ok(JsValue::Undefined);
                                }
                            }
                        }
                    }
                    stack.push(awaited);
                    pc += 1;
                }
                OpCode::Construct { argc } => {
                    let n = *argc as usize;
                    let start = stack.len().saturating_sub(n);
                    let args: Vec<JsValue> = stack.drain(start..).collect();
                    let ctor = stack.pop().unwrap_or(JsValue::Undefined);
                    match ctor {
                        JsValue::Fn(cid) => {
                            let res = self.invoke_callable_val(
                                program,
                                &JsValue::Fn(cid),
                                &args,
                                None,
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                            stack.push(res);
                        }
                        JsValue::Object(c_oid) => {
                            let inst_oid = self.alloc_obj()?;
                            if let Some(proto_val) = self
                                .obj_map(c_oid)
                                .and_then(|m| m.get("prototype"))
                                .cloned()
                            {
                                if let Some(im) = self.obj_map_mut(inst_oid) {
                                    im.insert("__proto".to_string(), proto_val);
                                }
                            }
                            if let Some(ctor_fn) =
                                self.obj_map(c_oid).and_then(|m| m.get("__ctor")).cloned()
                            {
                                let parent_cls = self
                                    .obj_map(c_oid)
                                    .and_then(|m| m.get("__parent_class"))
                                    .cloned();
                                let res_obj = self.invoke_class_ctor(
                                    program,
                                    &ctor_fn,
                                    &args,
                                    inst_oid,
                                    parent_cls,
                                    origin_url,
                                    dom,
                                    clipboard,
                                    storage,
                                    downloads,
                                )?;
                                stack.push(res_obj);
                            } else {
                                stack.push(JsValue::Object(inst_oid));
                            }
                        }
                        _ => stack.push(JsValue::Undefined),
                    }
                    pc += 1;
                }
                OpCode::CallValue { argc } => {
                    let n = *argc as usize;
                    let start = stack.len().saturating_sub(n);
                    let args: Vec<JsValue> = stack.drain(start..).collect();
                    let callee = stack.pop().unwrap_or(JsValue::Undefined);
                    if let JsValue::Fn(cid) = callee {
                        match self.callables.get(cid as usize).cloned() {
                            Some(Callable::UserClosure { fn_idx, env_scope }) => {
                                let def = program
                                    .functions
                                    .get(fn_idx as usize)
                                    .ok_or("Missing function def")?
                                    .clone();
                                if def.is_async {
                                    let res = self.invoke_callable_val(
                                        program,
                                        &JsValue::Fn(cid),
                                        &args,
                                        None,
                                        origin_url,
                                        dom,
                                        clipboard,
                                        storage,
                                        downloads,
                                    )?;
                                    stack.push(res);
                                    pc += 1;
                                    continue;
                                }
                                if call_frames.len() >= 256 {
                                    return Err("JS call stack overflow");
                                }
                                let mut vars = HashMap::with_capacity(def.params.len());
                                for (i, param) in def.params.iter().enumerate() {
                                    let pname = program
                                        .identifiers
                                        .get(param.name_id as usize)
                                        .cloned()
                                        .unwrap_or_default();
                                    if param.is_rest {
                                        let rest_items = if i < args.len() {
                                            args[i..].to_vec()
                                        } else {
                                            Vec::new()
                                        };
                                        vars.insert(
                                            pname,
                                            JsValue::Array(self.alloc_arr(rest_items)?),
                                        );
                                        break;
                                    }
                                    let mut arg_val =
                                        args.get(i).cloned().unwrap_or(JsValue::Undefined);
                                    if matches!(arg_val, JsValue::Undefined) {
                                        if let Some(d_pc) = param.default_pc {
                                            arg_val = self.run_vm_loop(
                                                program,
                                                d_pc,
                                                Vec::new(),
                                                env_scope,
                                                Vec::new(),
                                                Vec::new(),
                                                origin_url,
                                                dom,
                                                clipboard,
                                                storage,
                                                downloads,
                                            )?;
                                        }
                                    }
                                    vars.insert(pname, arg_val);
                                }
                                let new_scope = self.alloc_scope(Some(env_scope), vars)?;
                                call_frames.push(CallFrame {
                                    return_pc: pc + 1,
                                    prev_scope: current_scope,
                                    try_depth: try_stack.len(),
                                    async_promise_oid: None,
                                });
                                current_scope = new_scope;
                                pc = def.entry_pc;
                                continue;
                            }
                            Some(Callable::Builtin(bname)) => {
                                let ret = self.invoke_builtin_fn(
                                    program, bname, &args, origin_url, dom, clipboard, storage,
                                    downloads,
                                )?;
                                stack.push(ret);
                            }
                            Some(Callable::BoundResolver {
                                promise_oid,
                                is_reject,
                            }) => {
                                let val = args.first().cloned().unwrap_or(JsValue::Undefined);
                                self.settle_promise(promise_oid, !is_reject, val);
                                stack.push(JsValue::Undefined);
                            }
                            None => stack.push(JsValue::Undefined),
                        }
                    } else {
                        stack.push(JsValue::Undefined);
                    }
                    pc += 1;
                }
                OpCode::CallValueArray => {
                    let args_arr = stack.pop().unwrap_or(JsValue::Undefined);
                    let callee = stack.pop().unwrap_or(JsValue::Undefined);
                    let args = match args_arr {
                        JsValue::Array(aid) => self.arr_vec(aid).cloned().unwrap_or_default(),
                        _ => Vec::new(),
                    };
                    let ret = self.invoke_callable_val(
                        program,
                        &callee,
                        &args,
                        None,
                        origin_url,
                        dom,
                        clipboard,
                        storage,
                        downloads,
                    )?;
                    stack.push(ret);
                    pc += 1;
                }
                OpCode::CallMethod { prop_id, argc } => {
                    let n = *argc as usize;
                    let start = stack.len().saturating_sub(n);
                    let args: Vec<JsValue> = stack.drain(start..).collect();
                    let receiver = stack.pop().unwrap_or(JsValue::Undefined);
                    let method_name = program
                        .identifiers
                        .get(*prop_id as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let ret = self.invoke_method(
                        program,
                        &receiver,
                        method_name,
                        &args,
                        origin_url,
                        dom,
                        clipboard,
                        storage,
                        downloads,
                    )?;
                    stack.push(ret);
                    pc += 1;
                }
                OpCode::CallMethodArray { prop_id } => {
                    let args_arr = stack.pop().unwrap_or(JsValue::Undefined);
                    let receiver = stack.pop().unwrap_or(JsValue::Undefined);
                    let args = match args_arr {
                        JsValue::Array(aid) => self.arr_vec(aid).cloned().unwrap_or_default(),
                        _ => Vec::new(),
                    };
                    let method_name = program
                        .identifiers
                        .get(*prop_id as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let ret = self.invoke_method(
                        program,
                        &receiver,
                        method_name,
                        &args,
                        origin_url,
                        dom,
                        clipboard,
                        storage,
                        downloads,
                    )?;
                    stack.push(ret);
                    pc += 1;
                }
                OpCode::Return => {
                    let ret_val = stack.pop().unwrap_or(JsValue::Undefined);
                    if let Some(frame) = call_frames.pop() {
                        if let Some(p_oid) = frame.async_promise_oid {
                            self.settle_promise(p_oid, true, ret_val.clone());
                            return Ok(JsValue::Object(p_oid));
                        }
                        try_stack.truncate(frame.try_depth);
                        current_scope = frame.prev_scope;
                        stack.push(ret_val);
                        pc = frame.return_pc;
                    } else {
                        stack.push(ret_val);
                        break;
                    }
                }
                OpCode::Pop => {
                    if let Some(v) = stack.pop() {
                        last_popped = v;
                    }
                    pc += 1;
                }
                OpCode::Dup => {
                    let top = stack.last().cloned().unwrap_or(JsValue::Undefined);
                    stack.push(top);
                    pc += 1;
                }
                OpCode::Halt => break,
            }
        }

        Ok(stack.pop().unwrap_or(last_popped))
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_class_ctor(
        &mut self,
        program: &CompiledJsProgram,
        ctor_fn: &JsValue,
        args: &[JsValue],
        inst_oid: u32,
        parent_class: Option<JsValue>,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<JsValue, &'static str> {
        let JsValue::Fn(cid) = *ctor_fn else {
            return Ok(JsValue::Object(inst_oid));
        };
        let Some(Callable::UserClosure { fn_idx, env_scope }) =
            self.callables.get(cid as usize).cloned()
        else {
            return Ok(JsValue::Object(inst_oid));
        };
        let def = program
            .functions
            .get(fn_idx as usize)
            .ok_or("Missing ctor")?
            .clone();
        let mut vars = HashMap::with_capacity(def.params.len() + 2);
        vars.insert("this".to_string(), JsValue::Object(inst_oid));

        if let Some(JsValue::Object(p_cls_oid)) = parent_class {
            if let Some(p_ctor) = self.obj_map(p_cls_oid).and_then(|m| m.get("__ctor")).cloned() {
                if let JsValue::Fn(pcid) = p_ctor {
                    if let Some(Callable::UserClosure {
                        fn_idx: p_fn_idx,
                        env_scope: p_env,
                    }) = self.callables.get(pcid as usize).cloned()
                    {
                        let mut sm = HashMap::new();
                        sm.insert("this".to_string(), JsValue::Object(inst_oid));
                        let super_scope = self.alloc_scope(Some(p_env), sm)?;
                        let super_fn_cid = self.callables.len() as u32;
                        self.callables.push(Callable::UserClosure {
                            fn_idx: p_fn_idx,
                            env_scope: super_scope,
                        });
                        vars.insert("super".to_string(), JsValue::Fn(super_fn_cid));
                    }
                }
            }
        }

        for (i, param) in def.params.iter().enumerate() {
            let pname = program
                .identifiers
                .get(param.name_id as usize)
                .cloned()
                .unwrap_or_default();
            vars.insert(pname, args.get(i).cloned().unwrap_or(JsValue::Undefined));
        }
        let new_scope = self.alloc_scope(Some(env_scope), vars)?;
        let ret = self.run_vm_loop(
            program,
            def.entry_pc,
            Vec::new(),
            new_scope,
            Vec::new(),
            Vec::new(),
            origin_url,
            dom,
            clipboard,
            storage,
            downloads,
        )?;
        if matches!(ret, JsValue::Object(_)) {
            Ok(ret)
        } else {
            Ok(JsValue::Object(inst_oid))
        }
    }

    fn lookup_proto_prop(&self, mut oid: u32, prop: &str) -> Option<JsValue> {
        for _ in 0..16 {
            let map = self.obj_map(oid)?;
            if let Some(val) = map.get(prop) {
                return Some(val.clone());
            }
            match map.get("__proto") {
                Some(JsValue::Object(next_oid)) => oid = *next_oid,
                _ => return None,
            }
        }
        None
    }

    fn is_in_proto_chain(&self, mut oid: u32, target_proto_oid: u32) -> bool {
        for _ in 0..16 {
            let Some(map) = self.obj_map(oid) else {
                return false;
            };
            match map.get("__proto") {
                Some(JsValue::Object(next_oid)) => {
                    if *next_oid == target_proto_oid {
                        return true;
                    }
                    oid = *next_oid;
                }
                _ => return false,
            }
        }
        false
    }

    fn get_property(
        &mut self,
        obj: &JsValue,
        prop: &str,
        origin_url: &str,
        dom: &ZeroCopyDomArena,
        storage: &MantaStorageEngine,
    ) -> JsValue {
        match obj {
            JsValue::Str(s) => match prop {
                "length" => JsValue::Number(s.chars().count() as f64),
                _ => JsValue::Undefined,
            },
            JsValue::Array(aid) => match prop {
                "length" => {
                    JsValue::Number(self.arr_vec(*aid).map(|a| a.len()).unwrap_or(0) as f64)
                }
                _ => JsValue::Undefined,
            },
            JsValue::TypedArray(tid) => match prop {
                "length" => JsValue::Number(
                    self.typed_arrays
                        .get(*tid as usize)
                        .map(|a| a.len())
                        .unwrap_or(0) as f64,
                ),
                _ => JsValue::Undefined,
            },
            JsValue::Map(mid) => match prop {
                "size" => JsValue::Number(
                    self.maps.get(*mid as usize).map(|m| m.len()).unwrap_or(0) as f64,
                ),
                _ => JsValue::Undefined,
            },
            JsValue::Set(sid) => match prop {
                "size" => JsValue::Number(
                    self.sets.get(*sid as usize).map(|s| s.len()).unwrap_or(0) as f64,
                ),
                _ => JsValue::Undefined,
            },
            JsValue::Regex(rid) => match prop {
                "source" => self
                    .regexes
                    .get(*rid as usize)
                    .map(|r| JsValue::Str(r.pattern.clone()))
                    .unwrap_or(JsValue::Undefined),
                "flags" => self
                    .regexes
                    .get(*rid as usize)
                    .map(|r| JsValue::Str(r.flags.clone()))
                    .unwrap_or(JsValue::Undefined),
                "lastIndex" => self
                    .regexes
                    .get(*rid as usize)
                    .map(|r| JsValue::Number(r.last_index as f64))
                    .unwrap_or(JsValue::Number(0.0)),
                _ => JsValue::Undefined,
            },
            JsValue::DomNode(nid) => match prop {
                "id" => dom
                    .node_id_attr(*nid)
                    .map(|s| JsValue::Str(s.to_string()))
                    .unwrap_or_else(|| JsValue::Str(String::new())),
                "tagName" => dom
                    .node_tag(*nid)
                    .map(|t| JsValue::Str(t.to_ascii_uppercase()))
                    .unwrap_or_else(|| JsValue::Str(String::new())),
                "textContent" | "innerText" | "innerHTML" => JsValue::Str(dom.text_content(*nid)),
                "parentElement" | "parentNode" => {
                    let p = dom.nodes().get(*nid as usize).map(|n| n.parent).unwrap_or(NO_NODE);
                    if p == NO_NODE {
                        JsValue::Null
                    } else {
                        JsValue::DomNode(p)
                    }
                }
                "children" => {
                    let kids: Vec<JsValue> = dom
                        .element_children(*nid)
                        .into_iter()
                        .map(JsValue::DomNode)
                        .collect();
                    match self.alloc_arr(kids) {
                        Ok(aid) => JsValue::Array(aid),
                        Err(_) => JsValue::Undefined,
                    }
                }
                "value" => dom
                    .forms
                    .get_control(*nid)
                    .map(|c| JsValue::Str(c.value.clone()))
                    .unwrap_or_else(|| {
                        JsValue::Str(dom.get_attribute(*nid, "value").unwrap_or_default())
                    }),
                "checked" => JsValue::Bool(
                    dom.forms
                        .get_control(*nid)
                        .map(|c| c.checked)
                        .unwrap_or(false),
                ),
                "classList" => {
                    let mut m = HashMap::new();
                    m.insert("__manta_ns".to_string(), JsValue::Str("classList".to_string()));
                    m.insert("__nid".to_string(), JsValue::DomNode(*nid));
                    JsValue::Object(self.alloc_obj_map(m))
                }
                "style" => {
                    let mut m = HashMap::new();
                    m.insert("__manta_ns".to_string(), JsValue::Str("dom_style".to_string()));
                    m.insert("__nid".to_string(), JsValue::DomNode(*nid));
                    JsValue::Object(self.alloc_obj_map(m))
                }
                _ => dom
                    .get_attribute(*nid, prop)
                    .map(JsValue::Str)
                    .unwrap_or(JsValue::Undefined),
            },
            JsValue::Object(oid) => {
                if let Some(ns) = self
                    .obj_map(*oid)
                    .and_then(|m| m.get("__manta_ns"))
                    .map(|v| v.as_str())
                {
                    match (ns.as_str(), prop) {
                        ("document", "title") => return JsValue::Str(dom.page_title.clone()),
                        ("document", "cookie") => {
                            return JsValue::Str(
                                storage.get_cookie_header_for_url(origin_url, false),
                            );
                        }
                        ("document", "body") => {
                            return dom
                                .find_first_tag("body")
                                .map(JsValue::DomNode)
                                .unwrap_or(JsValue::DomNode(0));
                        }
                        ("document", "head") => {
                            return dom
                                .find_first_tag("head")
                                .map(JsValue::DomNode)
                                .unwrap_or(JsValue::Null);
                        }
                        ("document", "documentElement") => {
                            return dom
                                .find_first_tag("html")
                                .map(JsValue::DomNode)
                                .unwrap_or(JsValue::DomNode(0));
                        }
                        ("location", "href") => return JsValue::Str(origin_url.to_string()),
                        ("window", "location") => {
                            return self
                                .scope_ref(0)
                                .and_then(|g| g.vars.get("location"))
                                .cloned()
                                .unwrap_or(JsValue::Undefined);
                        }
                        ("Math", "PI") => return JsValue::Number(std::f64::consts::PI),
                        ("Math", "E") => return JsValue::Number(std::f64::consts::E),
                        _ => {}
                    }
                }
                self.lookup_proto_prop(*oid, prop).unwrap_or(JsValue::Undefined)
            }
            _ => JsValue::Undefined,
        }
    }

    fn set_property(
        &mut self,
        obj: &JsValue,
        prop: &str,
        val: JsValue,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        storage: &mut MantaStorageEngine,
    ) {
        match obj {
            JsValue::Regex(rid) if prop == "lastIndex" => {
                if let Some(r) = self.regexes.get_mut(*rid as usize) {
                    r.last_index = val.as_number().max(0.0) as usize;
                }
            }
            JsValue::DomNode(nid) => match prop {
                "innerHTML" => {
                    dom.parse_html_fragment_into(*nid, &val.as_str());
                }
                "textContent" | "innerText" => {
                    dom.set_text_content(*nid, val.as_str());
                }
                "value" => {
                    let s = val.as_str();
                    let _ = dom.forms.set_value(*nid, &s);
                    let key = dom
                        .node_id_attr(*nid)
                        .map(|id| id.to_string())
                        .or_else(|| dom.forms.get_control(*nid).map(|c| c.name.clone()))
                        .unwrap_or_else(|| format!("node_{}", nid));
                    storage.save_form_draft(origin_url, &key, &s);
                }
                "id" => {
                    dom.set_attribute(*nid, "id", &val.as_str());
                }
                _ => {
                    dom.set_attribute(*nid, prop, &val.as_str());
                }
            },
            JsValue::Object(oid) => {
                if let Some(ns) = self
                    .obj_map(*oid)
                    .and_then(|m| m.get("__manta_ns"))
                    .map(|v| v.as_str())
                {
                    match (ns.as_str(), prop) {
                        ("document", "title") => {
                            dom.page_title = val.as_str();
                            return;
                        }
                        ("document", "cookie") => {
                            storage.set_cookie_from_header(origin_url, &val.as_str());
                            return;
                        }
                        ("dom_style", css_prop) => {
                            let nid_opt =
                                self.obj_map(*oid).and_then(|m| m.get("__nid")).cloned();
                            if let Some(JsValue::DomNode(nid)) = nid_opt {
                                let prev = dom.get_attribute(nid, "style").unwrap_or_default();
                                let updated = if prev.is_empty() {
                                    format!("{}: {};", css_prop, val.as_str())
                                } else {
                                    format!(
                                        "{}; {}: {};",
                                        prev.trim_end_matches(';'),
                                        css_prop,
                                        val.as_str()
                                    )
                                };
                                dom.set_attribute(nid, "style", &updated);
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                if let Some(m) = self.obj_map_mut(*oid) {
                    m.insert(prop.to_string(), val);
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_builtin_fn(
        &mut self,
        program: &CompiledJsProgram,
        name: &str,
        args: &[JsValue],
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<JsValue, &'static str> {
        match name {
            "parseInt" => {
                let s = args.first().map(|a| a.as_str()).unwrap_or_default();
                let t = s.trim();
                let radix = args.get(1).map(|a| a.as_number() as u32).unwrap_or(10);
                if radix == 16 || t.starts_with("0x") || t.starts_with("0X") {
                    let hex = t.trim_start_matches("0x").trim_start_matches("0X");
                    Ok(JsValue::Number(
                        i64::from_str_radix(hex, 16)
                            .map(|n| n as f64)
                            .unwrap_or(f64::NAN),
                    ))
                } else {
                    Ok(JsValue::Number(
                        t.parse::<i64>().map(|n| n as f64).unwrap_or(f64::NAN),
                    ))
                }
            }
            "parseFloat" => Ok(JsValue::Number(
                args.first().map(|a| a.as_number()).unwrap_or(f64::NAN),
            )),
            "isNaN" => Ok(JsValue::Bool(
                args.first().map(|a| a.as_number().is_nan()).unwrap_or(true),
            )),
            "isFinite" => Ok(JsValue::Bool(
                args.first()
                    .map(|a| a.as_number().is_finite())
                    .unwrap_or(false),
            )),
            "Number" => Ok(JsValue::Number(
                args.first().map(|a| a.as_number()).unwrap_or(0.0),
            )),
            "String" => Ok(JsValue::Str(
                args.first().map(|a| a.as_str()).unwrap_or_default(),
            )),
            "Boolean" => Ok(JsValue::Bool(
                args.first().map(|a| a.is_truthy()).unwrap_or(false),
            )),
            "structuredClone" => {
                let val = args.first().unwrap_or(&JsValue::Undefined);
                let json = self.json_stringify(val);
                Ok(self.json_parse(&json).unwrap_or_else(|_| val.clone()))
            }
            "encodeURIComponent" => {
                let raw = args.first().map(|a| a.as_str()).unwrap_or_default();
                Ok(JsValue::Str(percent_encode(&raw)))
            }
            "decodeURIComponent" => {
                let raw = args.first().map(|a| a.as_str()).unwrap_or_default();
                Ok(JsValue::Str(percent_decode(&raw)))
            }
            "btoa" => {
                let raw = args.first().map(|a| a.as_str()).unwrap_or_default();
                Ok(JsValue::Str(base64_encode(raw.as_bytes())))
            }
            "atob" => {
                let raw = args.first().map(|a| a.as_str()).unwrap_or_default();
                let bytes = base64_decode(&raw);
                Ok(JsValue::Str(String::from_utf8_lossy(&bytes).into_owned()))
            }
            "queueMicrotask" => {
                if let Some(cb) = args.first().cloned() {
                    self.enqueue_timer(
                        0,
                        Microtask::Callback {
                            cb,
                            arg: JsValue::Undefined,
                            resolve_promise_oid: None,
                        },
                    );
                }
                Ok(JsValue::Undefined)
            }
            "setTimeout" => {
                let delay = args
                    .get(1)
                    .map(|a| a.as_number().max(0.0) as u64)
                    .unwrap_or(0);
                if let Some(cb) = args.first().cloned() {
                    self.enqueue_timer(
                        delay.max(1),
                        Microtask::Callback {
                            cb,
                            arg: JsValue::Undefined,
                            resolve_promise_oid: None,
                        },
                    );
                }
                Ok(JsValue::Number(self.timer_seq as f64))
            }
            "clearTimeout" => Ok(JsValue::Undefined),
            "AbortController" => {
                let mut sig = HashMap::new();
                sig.insert("aborted".to_string(), JsValue::Bool(false));
                let sig_oid = self.alloc_obj_map(sig);
                let mut ctrl = HashMap::new();
                ctrl.insert("__manta_ns".to_string(), JsValue::Str("abort_ctrl".to_string()));
                ctrl.insert("signal".to_string(), JsValue::Object(sig_oid));
                Ok(JsValue::Object(self.alloc_obj_map(ctrl)))
            }
            "Headers" | "FormData" => {
                let ns = if name == "Headers" { "headers" } else { "formdata" };
                let mut m = HashMap::new();
                m.insert("__manta_ns".to_string(), JsValue::Str(ns.to_string()));
                if let Some(JsValue::Object(init_oid)) = args.first() {
                    if let Some(init_map) = self.obj_map(*init_oid).cloned() {
                        for (k, v) in init_map {
                            if !k.starts_with("__") {
                                m.insert(k.to_ascii_lowercase(), v);
                            }
                        }
                    }
                }
                Ok(JsValue::Object(self.alloc_obj_map(m)))
            }
            "Event" | "CustomEvent" => {
                let mut m = HashMap::new();
                m.insert("__manta_ns".to_string(), JsValue::Str("dom_event".to_string()));
                m.insert(
                    "type".to_string(),
                    JsValue::Str(args.first().map(|a| a.as_str()).unwrap_or_default()),
                );
                m.insert("defaultPrevented".to_string(), JsValue::Bool(false));
                if let Some(JsValue::Object(opt_oid)) = args.get(1) {
                    if let Some(detail) = self.obj_map(*opt_oid).and_then(|o| o.get("detail")).cloned()
                    {
                        m.insert("detail".to_string(), detail);
                    }
                }
                Ok(JsValue::Object(self.alloc_obj_map(m)))
            }
            "MutationObserver" | "ResizeObserver" | "IntersectionObserver" => {
                let mut m = HashMap::new();
                m.insert("__manta_ns".to_string(), JsValue::Str("observer".to_string()));
                if let Some(cb) = args.first().cloned() {
                    m.insert("__cb".to_string(), cb);
                }
                Ok(JsValue::Object(self.alloc_obj_map(m)))
            }
            "XMLHttpRequest" => {
                let mut m = HashMap::new();
                m.insert("__manta_ns".to_string(), JsValue::Str("xhr".to_string()));
                m.insert("readyState".to_string(), JsValue::Number(0.0));
                m.insert("status".to_string(), JsValue::Number(0.0));
                m.insert("responseText".to_string(), JsValue::Str(String::new()));
                let hdr_oid = self.alloc_obj()?;
                m.insert("__req_headers".to_string(), JsValue::Object(hdr_oid));
                Ok(JsValue::Object(self.alloc_obj_map(m)))
            }
            "Map" => {
                let mid = self.maps.len() as u32;
                let mut entries = Vec::new();
                if let Some(JsValue::Array(aid)) = args.first() {
                    if let Some(items) = self.arr_vec(*aid).cloned() {
                        for pair in items {
                            if let JsValue::Array(p_aid) = pair {
                                if let Some(p_vec) = self.arr_vec(p_aid) {
                                    if p_vec.len() >= 2 {
                                        entries.push((p_vec[0].as_str(), p_vec[1].clone()));
                                    }
                                }
                            }
                        }
                    }
                }
                self.maps.push(entries);
                Ok(JsValue::Map(mid))
            }
            "Set" => {
                let sid = self.sets.len() as u32;
                let mut entries = Vec::new();
                if let Some(JsValue::Array(aid)) = args.first() {
                    if let Some(items) = self.arr_vec(*aid) {
                        for it in items {
                            if !entries.contains(it) {
                                entries.push(it.clone());
                            }
                        }
                    }
                }
                self.sets.push(entries);
                Ok(JsValue::Set(sid))
            }
            "RegExp" => {
                let pat = args.first().map(|a| a.as_str()).unwrap_or_default();
                let flags = args.get(1).map(|a| a.as_str()).unwrap_or_default();
                let rid = self.regexes.len() as u32;
                self.regexes.push(RegexEntry {
                    pattern: pat,
                    flags,
                    last_index: 0,
                });
                Ok(JsValue::Regex(rid))
            }
            "Float64Array" | "Int32Array" | "Uint8Array" => {
                let vec = match args.first() {
                    Some(JsValue::Number(len)) => vec![0.0; (*len as usize).min(65536)],
                    Some(JsValue::Array(aid)) => self
                        .arr_vec(*aid)
                        .map(|a| a.iter().map(|v| v.as_number()).collect())
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                let tid = self.typed_arrays.len() as u32;
                self.typed_arrays.push(vec);
                Ok(JsValue::TypedArray(tid))
            }
            "Promise" => {
                let p_oid = self.alloc_promise(None, JsValue::Undefined)?;
                if let Some(executor) = args.first().cloned() {
                    let res_cid = self.callables.len() as u32;
                    self.callables.push(Callable::BoundResolver {
                        promise_oid: p_oid,
                        is_reject: false,
                    });
                    let rej_cid = self.callables.len() as u32;
                    self.callables.push(Callable::BoundResolver {
                        promise_oid: p_oid,
                        is_reject: true,
                    });
                    if let Err(e) = self.invoke_callable_val(
                        program,
                        &executor,
                        &[JsValue::Fn(res_cid), JsValue::Fn(rej_cid)],
                        None,
                        origin_url,
                        dom,
                        clipboard,
                        storage,
                        downloads,
                    ) {
                        self.settle_promise(p_oid, false, JsValue::Str(e.to_string()));
                    }
                }
                Ok(JsValue::Object(p_oid))
            }
            "URL" => {
                let raw = args.first().map(|a| a.as_str()).unwrap_or_default();
                let (protocol, rest) = raw
                    .split_once("://")
                    .map(|(s, r)| (format!("{}:", s), r))
                    .unwrap_or(("https:".to_string(), raw.as_str()));
                let (host, path_q) = match rest.find('/') {
                    Some(idx) => (&rest[..idx], &rest[idx..]),
                    None => (rest, "/"),
                };
                let (pathname, search) = match path_q.find('?') {
                    Some(q) => (&path_q[..q], &path_q[q..]),
                    None => (path_q, ""),
                };
                let mut m = HashMap::new();
                m.insert("href".to_string(), JsValue::Str(raw.clone()));
                m.insert("protocol".to_string(), JsValue::Str(protocol.clone()));
                m.insert("host".to_string(), JsValue::Str(host.to_string()));
                m.insert(
                    "hostname".to_string(),
                    JsValue::Str(host.split(':').next().unwrap_or(host).to_string()),
                );
                m.insert("pathname".to_string(), JsValue::Str(pathname.to_string()));
                m.insert("search".to_string(), JsValue::Str(search.to_string()));
                m.insert(
                    "origin".to_string(),
                    JsValue::Str(format!("{}//{}", protocol, host)),
                );
                Ok(JsValue::Object(self.alloc_obj_map(m)))
            }
            "URLSearchParams" => {
                let raw = args.first().map(|a| a.as_str()).unwrap_or_default();
                let mut m = HashMap::new();
                m.insert("__manta_ns".to_string(), JsValue::Str("urlparams".to_string()));
                for pair in raw.trim_start_matches('?').split('&') {
                    if let Some((k, v)) = pair.split_once('=') {
                        m.insert(percent_decode(k), JsValue::Str(percent_decode(v)));
                    }
                }
                Ok(JsValue::Object(self.alloc_obj_map(m)))
            }
            "TextEncoder" => {
                let mut m = HashMap::new();
                m.insert("__manta_ns".to_string(), JsValue::Str("textencoder".to_string()));
                Ok(JsValue::Object(self.alloc_obj_map(m)))
            }
            "TextDecoder" => {
                let mut m = HashMap::new();
                m.insert("__manta_ns".to_string(), JsValue::Str("textdecoder".to_string()));
                Ok(JsValue::Object(self.alloc_obj_map(m)))
            }
            "Error" => {
                let msg = args.first().map(|a| a.as_str()).unwrap_or_default();
                let mut m = HashMap::new();
                m.insert("message".to_string(), JsValue::Str(msg));
                m.insert("name".to_string(), JsValue::Str("Error".to_string()));
                Ok(JsValue::Object(self.alloc_obj_map(m)))
            }
            "fetch" => {
                let url = args.first().map(|a| a.as_str()).unwrap_or_default();
                let mut method = String::from("GET");
                let mut headers = HashMap::new();
                let mut req_body: Option<Vec<u8>> = None;
                if !origin_url.is_empty() {
                    headers.insert("Origin".to_string(), origin_url.to_string());
                }
                if let Some(JsValue::Object(opt_oid)) = args.get(1) {
                    if let Some(opts) = self.obj_map(*opt_oid).cloned() {
                        if let Some(m) = opts.get("method") {
                            method = m.as_str().to_ascii_uppercase();
                        }
                        if let Some(b_val) = opts.get("body") {
                            if !matches!(b_val, JsValue::Undefined | JsValue::Null) {
                                req_body = Some(b_val.as_str().into_bytes());
                            }
                        }
                        if let Some(JsValue::Object(h_oid)) = opts.get("headers") {
                            if let Some(hm) = self.obj_map(*h_oid) {
                                for (k, v) in hm {
                                    if !k.starts_with("__") {
                                        headers.insert(k.clone(), v.as_str());
                                    }
                                }
                            }
                        }
                    }
                }

                let mut modules = ModuleBus::new();
                modules.register(Box::new(SiteIsolationModule::new()));
                let mut body = Vec::new();
                match StreamingNetworkPipe::stream_http_with_body(
                    &method,
                    &url,
                    headers,
                    req_body.as_deref(),
                    &modules,
                    4 * 1024 * 1024,
                    |chunk, _| body.extend_from_slice(chunk),
                ) {
                    Ok(outcome) => {
                        let body_text = String::from_utf8_lossy(&body).into_owned();
                        let mut resp_map = HashMap::new();
                        resp_map.insert(
                            "__manta_ns".to_string(),
                            JsValue::Str("fetch_resp".to_string()),
                        );
                        resp_map.insert(
                            "ok".to_string(),
                            JsValue::Bool((200..300).contains(&outcome.status_code)),
                        );
                        resp_map.insert(
                            "status".to_string(),
                            JsValue::Number(outcome.status_code as f64),
                        );
                        resp_map.insert("__body".to_string(), JsValue::Str(body_text));
                        let r_oid = self.alloc_obj_map(resp_map);
                        let p_oid = self.alloc_promise(Some(true), JsValue::Object(r_oid))?;
                        Ok(JsValue::Object(p_oid))
                    }
                    Err(e) => {
                        let p_oid = self.alloc_promise(Some(false), JsValue::Str(e))?;
                        Ok(JsValue::Object(p_oid))
                    }
                }
            }
            _ => Ok(JsValue::Undefined),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_method(
        &mut self,
        program: &CompiledJsProgram,
        receiver: &JsValue,
        method: &str,
        args: &[JsValue],
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<JsValue, &'static str> {
        match receiver {
            JsValue::Fn(cid) => {
                match self.callables.get(*cid as usize).cloned() {
                    Some(Callable::Builtin("String")) if method == "fromCharCode" => {
                        let s: String = args
                            .iter()
                            .filter_map(|a| char::from_u32(a.as_number() as u32))
                            .collect();
                        return Ok(JsValue::Str(s));
                    }
                    Some(Callable::Builtin("Number")) => match method {
                        "isInteger" => {
                            let ok = matches!(
                                args.first(),
                                Some(JsValue::Number(n)) if n.is_finite() && n.fract() == 0.0
                            );
                            return Ok(JsValue::Bool(ok));
                        }
                        "isNaN" => {
                            let ok = matches!(args.first(), Some(JsValue::Number(n)) if n.is_nan());
                            return Ok(JsValue::Bool(ok));
                        }
                        "isFinite" => {
                            let ok =
                                matches!(args.first(), Some(JsValue::Number(n)) if n.is_finite());
                            return Ok(JsValue::Bool(ok));
                        }
                        _ => {}
                    },
                    Some(Callable::Builtin("Promise")) => match method {
                        "resolve" => {
                            let val = args.first().cloned().unwrap_or(JsValue::Undefined);
                            let p = self.alloc_promise(Some(true), val)?;
                            return Ok(JsValue::Object(p));
                        }
                        "reject" => {
                            let err = args.first().cloned().unwrap_or(JsValue::Undefined);
                            let p = self.alloc_promise(Some(false), err)?;
                            return Ok(JsValue::Object(p));
                        }
                        "withResolvers" => {
                            let p_oid = self.alloc_promise(None, JsValue::Undefined)?;
                            let res_cid = self.callables.len() as u32;
                            self.callables.push(Callable::BoundResolver {
                                promise_oid: p_oid,
                                is_reject: false,
                            });
                            let rej_cid = self.callables.len() as u32;
                            self.callables.push(Callable::BoundResolver {
                                promise_oid: p_oid,
                                is_reject: true,
                            });
                            let mut m = HashMap::new();
                            m.insert("promise".to_string(), JsValue::Object(p_oid));
                            m.insert("resolve".to_string(), JsValue::Fn(res_cid));
                            m.insert("reject".to_string(), JsValue::Fn(rej_cid));
                            return Ok(JsValue::Object(self.alloc_obj_map(m)));
                        }
                        "all" => {
                            if let Some(JsValue::Array(aid)) = args.first() {
                                let items = self.arr_vec(*aid).cloned().unwrap_or_default();
                                let mut results = Vec::with_capacity(items.len());
                                for it in items {
                                    if let JsValue::Object(poid) = it {
                                        if let Some(rec) = self.promises.get(&poid) {
                                            if rec.fulfilled == Some(false) {
                                                let rej = self.alloc_promise(
                                                    Some(false),
                                                    rec.value.clone(),
                                                )?;
                                                return Ok(JsValue::Object(rej));
                                            }
                                            results.push(rec.value.clone());
                                            continue;
                                        }
                                    }
                                    results.push(it);
                                }
                                let out_aid = self.alloc_arr(results)?;
                                let p = self.alloc_promise(Some(true), JsValue::Array(out_aid))?;
                                return Ok(JsValue::Object(p));
                            }
                        }
                        "allSettled" => {
                            if let Some(JsValue::Array(aid)) = args.first() {
                                let items = self.arr_vec(*aid).cloned().unwrap_or_default();
                                let mut results = Vec::with_capacity(items.len());
                                for it in items {
                                    let mut entry = HashMap::new();
                                    if let JsValue::Object(poid) = it {
                                        if let Some(rec) = self.promises.get(&poid).cloned() {
                                            if rec.fulfilled == Some(false) {
                                                entry.insert(
                                                    "status".to_string(),
                                                    JsValue::Str("rejected".to_string()),
                                                );
                                                entry.insert("reason".to_string(), rec.value);
                                            } else {
                                                entry.insert(
                                                    "status".to_string(),
                                                    JsValue::Str("fulfilled".to_string()),
                                                );
                                                entry.insert("value".to_string(), rec.value);
                                            }
                                            results.push(JsValue::Object(self.alloc_obj_map(entry)));
                                            continue;
                                        }
                                    }
                                    entry.insert(
                                        "status".to_string(),
                                        JsValue::Str("fulfilled".to_string()),
                                    );
                                    entry.insert("value".to_string(), it);
                                    results.push(JsValue::Object(self.alloc_obj_map(entry)));
                                }
                                let out_aid = self.alloc_arr(results)?;
                                let p = self.alloc_promise(Some(true), JsValue::Array(out_aid))?;
                                return Ok(JsValue::Object(p));
                            }
                        }
                        "race" => {
                            if let Some(JsValue::Array(aid)) = args.first() {
                                if let Some(first) =
                                    self.arr_vec(*aid).and_then(|a| a.first()).cloned()
                                {
                                    if let JsValue::Object(poid) = first {
                                        if let Some(rec) = self.promises.get(&poid).cloned() {
                                            let p = self.alloc_promise(rec.fulfilled, rec.value)?;
                                            return Ok(JsValue::Object(p));
                                        }
                                    }
                                    let p = self.alloc_promise(Some(true), first)?;
                                    return Ok(JsValue::Object(p));
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
                Ok(JsValue::Undefined)
            }
            JsValue::Regex(rid) => {
                let text = args.first().map(|a| a.as_str()).unwrap_or_default();
                let Some(entry) = self.regexes.get(*rid as usize).cloned() else {
                    return Ok(JsValue::Null);
                };
                let is_global = entry.flags.contains('g');
                let start_pos = if is_global {
                    entry.last_index.min(text.len())
                } else {
                    0
                };
                match method {
                    "test" => {
                        let matched = regex_find(&entry.pattern, &entry.flags, &text, start_pos);
                        if let Some((_, end)) = matched {
                            if is_global {
                                self.regexes[*rid as usize].last_index = end;
                            }
                            Ok(JsValue::Bool(true))
                        } else {
                            if is_global {
                                self.regexes[*rid as usize].last_index = 0;
                            }
                            Ok(JsValue::Bool(false))
                        }
                    }
                    "exec" => {
                        if let Some((s, e)) =
                            regex_find(&entry.pattern, &entry.flags, &text, start_pos)
                        {
                            if is_global {
                                self.regexes[*rid as usize].last_index = e;
                            }
                            let sub = text.get(s..e).unwrap_or("").to_string();
                            Ok(JsValue::Array(self.alloc_arr(vec![JsValue::Str(sub)])?))
                        } else {
                            if is_global {
                                self.regexes[*rid as usize].last_index = 0;
                            }
                            Ok(JsValue::Null)
                        }
                    }
                    _ => Ok(JsValue::Undefined),
                }
            }
            JsValue::Map(mid) => match method {
                "set" => {
                    let k = args.first().map(|a| a.as_str()).unwrap_or_default();
                    let v = args.get(1).cloned().unwrap_or(JsValue::Undefined);
                    if let Some(m) = self.maps.get_mut(*mid as usize) {
                        if let Some(entry) = m.iter_mut().find(|(ek, _)| ek == &k) {
                            entry.1 = v;
                        } else {
                            m.push((k, v));
                        }
                    }
                    Ok(JsValue::Map(*mid))
                }
                "get" => {
                    let k = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(self
                        .maps
                        .get(*mid as usize)
                        .and_then(|m| m.iter().find(|(ek, _)| ek == &k).map(|(_, v)| v.clone()))
                        .unwrap_or(JsValue::Undefined))
                }
                "has" => {
                    let k = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(JsValue::Bool(
                        self.maps
                            .get(*mid as usize)
                            .map(|m| m.iter().any(|(ek, _)| ek == &k))
                            .unwrap_or(false),
                    ))
                }
                "delete" => {
                    let k = args.first().map(|a| a.as_str()).unwrap_or_default();
                    if let Some(m) = self.maps.get_mut(*mid as usize) {
                        let before = m.len();
                        m.retain(|(ek, _)| ek != &k);
                        return Ok(JsValue::Bool(m.len() != before));
                    }
                    Ok(JsValue::Bool(false))
                }
                "clear" => {
                    if let Some(m) = self.maps.get_mut(*mid as usize) {
                        m.clear();
                    }
                    Ok(JsValue::Undefined)
                }
                _ => Ok(JsValue::Undefined),
            },
            JsValue::Set(sid) => match method {
                "add" => {
                    let val = args.first().cloned().unwrap_or(JsValue::Undefined);
                    if let Some(s) = self.sets.get_mut(*sid as usize) {
                        if !s.contains(&val) {
                            s.push(val);
                        }
                    }
                    Ok(JsValue::Set(*sid))
                }
                "has" => {
                    let val = args.first().cloned().unwrap_or(JsValue::Undefined);
                    Ok(JsValue::Bool(
                        self.sets
                            .get(*sid as usize)
                            .map(|s| s.contains(&val))
                            .unwrap_or(false),
                    ))
                }
                "delete" => {
                    let val = args.first().cloned().unwrap_or(JsValue::Undefined);
                    if let Some(s) = self.sets.get_mut(*sid as usize) {
                        let before = s.len();
                        s.retain(|x| x != &val);
                        return Ok(JsValue::Bool(s.len() != before));
                    }
                    Ok(JsValue::Bool(false))
                }
                "clear" => {
                    if let Some(s) = self.sets.get_mut(*sid as usize) {
                        s.clear();
                    }
                    Ok(JsValue::Undefined)
                }
                _ => Ok(JsValue::Undefined),
            },
            JsValue::TypedArray(tid) => match method {
                "sum" => Ok(JsValue::Number(
                    self.typed_arrays
                        .get(*tid as usize)
                        .map(|a| fast_f64_sum(a))
                        .unwrap_or(0.0),
                )),
                "min" => Ok(JsValue::Number(
                    self.typed_arrays
                        .get(*tid as usize)
                        .map(|a| fast_f64_min(a))
                        .unwrap_or(0.0),
                )),
                "max" => Ok(JsValue::Number(
                    self.typed_arrays
                        .get(*tid as usize)
                        .map(|a| fast_f64_max(a))
                        .unwrap_or(0.0),
                )),
                "dot" => {
                    if let Some(JsValue::TypedArray(other_tid)) = args.first() {
                        let a = self.typed_arrays.get(*tid as usize);
                        let b = self.typed_arrays.get(*other_tid as usize);
                        if let (Some(av), Some(bv)) = (a, b) {
                            return Ok(JsValue::Number(fast_f64_dot(av, bv)));
                        }
                    }
                    Ok(JsValue::Number(0.0))
                }
                "sort" => {
                    if let Some(arr) = self.typed_arrays.get_mut(*tid as usize) {
                        fast_f64_sort(arr);
                    }
                    Ok(JsValue::TypedArray(*tid))
                }
                "reduce" => {
                    let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                    let mut acc = args.get(1).cloned().unwrap_or(JsValue::Number(0.0));
                    let items = self
                        .typed_arrays
                        .get(*tid as usize)
                        .cloned()
                        .unwrap_or_default();
                    for (i, n) in items.into_iter().enumerate() {
                        acc = self.invoke_callable_val(
                            program,
                            &cb,
                            &[acc, JsValue::Number(n), JsValue::Number(i as f64)],
                            None,
                            origin_url,
                            dom,
                            clipboard,
                            storage,
                            downloads,
                        )?;
                    }
                    Ok(acc)
                }
                "scaleAdd" => {
                    let scale = args.first().map(|a| a.as_number()).unwrap_or(1.0);
                    let bias = args.get(1).map(|a| a.as_number()).unwrap_or(0.0);
                    if let Some(arr) = self.typed_arrays.get_mut(*tid as usize) {
                        let src = arr.clone();
                        fast_f64_scale_add(arr, &src, scale, bias);
                    }
                    Ok(JsValue::TypedArray(*tid))
                }
                _ => Ok(JsValue::Undefined),
            },
            JsValue::Str(s) => match method {
                "trim" => Ok(JsValue::Str(s.trim().to_string())),
                "trimStart" => Ok(JsValue::Str(s.trim_start().to_string())),
                "trimEnd" => Ok(JsValue::Str(s.trim_end().to_string())),
                "toLowerCase" => Ok(JsValue::Str(s.to_ascii_lowercase())),
                "toUpperCase" => Ok(JsValue::Str(s.to_ascii_uppercase())),
                "charAt" => {
                    let idx = args.first().map(|a| a.as_number() as usize).unwrap_or(0);
                    Ok(JsValue::Str(
                        s.chars().nth(idx).map(|c| c.to_string()).unwrap_or_default(),
                    ))
                }
                "charCodeAt" => {
                    let idx = args.first().map(|a| a.as_number() as usize).unwrap_or(0);
                    Ok(JsValue::Number(
                        s.chars().nth(idx).map(|c| c as u32 as f64).unwrap_or(f64::NAN),
                    ))
                }
                "padStart" => {
                    let target = (args.first().map(|a| a.as_number() as usize).unwrap_or(0)).min(4096);
                    let pad = args
                        .get(1)
                        .map(|a| a.as_str())
                        .filter(|p| !p.is_empty())
                        .unwrap_or_else(|| " ".to_string());
                    let cur_len = s.chars().count();
                    if cur_len >= target {
                        Ok(JsValue::Str(s.clone()))
                    } else {
                        let need = target - cur_len;
                        let prefix: String = pad.chars().cycle().take(need).collect();
                        Ok(JsValue::Str(format!("{}{}", prefix, s)))
                    }
                }
                "padEnd" => {
                    let target = (args.first().map(|a| a.as_number() as usize).unwrap_or(0)).min(4096);
                    let pad = args
                        .get(1)
                        .map(|a| a.as_str())
                        .filter(|p| !p.is_empty())
                        .unwrap_or_else(|| " ".to_string());
                    let cur_len = s.chars().count();
                    if cur_len >= target {
                        Ok(JsValue::Str(s.clone()))
                    } else {
                        let need = target - cur_len;
                        let suffix: String = pad.chars().cycle().take(need).collect();
                        Ok(JsValue::Str(format!("{}{}", s, suffix)))
                    }
                }
                "repeat" => {
                    let cnt = (args.first().map(|a| a.as_number() as usize).unwrap_or(0)).min(4096);
                    Ok(JsValue::Str(s.repeat(cnt)))
                }
                "includes" => {
                    let sub = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(JsValue::Bool(find_substr(s, &sub, 0).is_some()))
                }
                "startsWith" => {
                    let sub = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(JsValue::Bool(s.starts_with(&sub)))
                }
                "endsWith" => {
                    let sub = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(JsValue::Bool(s.ends_with(&sub)))
                }
                "indexOf" => {
                    let sub = args.first().map(|a| a.as_str()).unwrap_or_default();
                    let from = args
                        .get(1)
                        .map(|a| a.as_number().max(0.0) as usize)
                        .unwrap_or(0);
                    Ok(JsValue::Number(
                        find_substr(s, &sub, from).map(|i| i as f64).unwrap_or(-1.0),
                    ))
                }
                "search" => {
                    if let Some(JsValue::Regex(rid)) = args.first() {
                        if let Some(r) = self.regexes.get(*rid as usize) {
                            return Ok(JsValue::Number(
                                regex_find(&r.pattern, &r.flags, s, 0)
                                    .map(|(st, _)| st as f64)
                                    .unwrap_or(-1.0),
                            ));
                        }
                    }
                    let sub = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(JsValue::Number(
                        find_substr(s, &sub, 0).map(|i| i as f64).unwrap_or(-1.0),
                    ))
                }
                "match" => {
                    if let Some(JsValue::Regex(rid)) = args.first() {
                        if let Some(r) = self.regexes.get(*rid as usize).cloned() {
                            if r.flags.contains('g') {
                                let mut out = Vec::new();
                                let mut pos = 0usize;
                                while pos <= s.len() {
                                    if let Some((st, en)) =
                                        regex_find(&r.pattern, &r.flags, s, pos)
                                    {
                                        if let Some(sub) = s.get(st..en) {
                                            out.push(JsValue::Str(sub.to_string()));
                                        }
                                        pos = if en == st {
                                            next_utf8_boundary(s, st)
                                        } else {
                                            en
                                        };
                                    } else {
                                        break;
                                    }
                                }
                                return if out.is_empty() {
                                    Ok(JsValue::Null)
                                } else {
                                    Ok(JsValue::Array(self.alloc_arr(out)?))
                                };
                            } else if let Some((st, en)) = regex_find(&r.pattern, &r.flags, s, 0) {
                                let sub = s.get(st..en).unwrap_or("").to_string();
                                return Ok(JsValue::Array(self.alloc_arr(vec![JsValue::Str(sub)])?));
                            }
                            return Ok(JsValue::Null);
                        }
                    }
                    Ok(JsValue::Null)
                }
                "replace" | "replaceAll" => {
                    let rep = args.get(1).map(|a| a.as_str()).unwrap_or_default();
                    if let Some(JsValue::Regex(rid)) = args.first() {
                        if let Some(r) = self.regexes.get(*rid as usize).cloned() {
                            let is_all = method == "replaceAll" || r.flags.contains('g');
                            let mut out = String::with_capacity(s.len());
                            let mut pos = 0usize;
                            while pos <= s.len() {
                                if let Some((st, en)) = regex_find(&r.pattern, &r.flags, s, pos) {
                                    if let Some(prefix) = s.get(pos..st) {
                                        out.push_str(prefix);
                                    }
                                    out.push_str(&rep);
                                    pos = if en == st {
                                        next_utf8_boundary(s, st)
                                    } else {
                                        en
                                    };
                                    if !is_all {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }
                            if let Some(tail) = s.get(pos..) {
                                out.push_str(tail);
                            }
                            return Ok(JsValue::Str(out));
                        }
                    }
                    let pat = args.first().map(|a| a.as_str()).unwrap_or_default();
                    if method == "replaceAll" {
                        Ok(JsValue::Str(s.replace(&pat, &rep)))
                    } else {
                        Ok(JsValue::Str(s.replacen(&pat, &rep, 1)))
                    }
                }
                "split" => {
                    let sep = args.first().map(|a| a.as_str()).unwrap_or_default();
                    let items: Vec<JsValue> = if sep.is_empty() {
                        s.chars().map(|c| JsValue::Str(c.to_string())).collect()
                    } else {
                        s.split(&sep).map(|p| JsValue::Str(p.to_string())).collect()
                    };
                    Ok(JsValue::Array(self.alloc_arr(items)?))
                }
                "slice" | "substring" => {
                    let len = s.chars().count();
                    let start = args.first().map(|a| a.as_number() as isize).unwrap_or(0);
                    let end = args
                        .get(1)
                        .map(|a| a.as_number() as isize)
                        .unwrap_or(len as isize);
                    let st = if start < 0 {
                        (len as isize + start).max(0) as usize
                    } else {
                        (start as usize).min(len)
                    };
                    let en = if end < 0 {
                        (len as isize + end).max(0) as usize
                    } else {
                        (end as usize).min(len)
                    };
                    let sliced: String = if st < en {
                        s.chars().skip(st).take(en - st).collect()
                    } else {
                        String::new()
                    };
                    Ok(JsValue::Str(sliced))
                }
                _ => Ok(JsValue::Undefined),
            },
            JsValue::Array(aid) => {
                let items_snapshot = self.arr_vec(*aid).cloned().unwrap_or_default();
                match method {
                    "at" => {
                        let idx = args.first().map(|a| a.as_number() as isize).unwrap_or(0);
                        let len = items_snapshot.len() as isize;
                        let pos = if idx < 0 { len + idx } else { idx };
                        if pos >= 0 && (pos as usize) < items_snapshot.len() {
                            Ok(items_snapshot[pos as usize].clone())
                        } else {
                            Ok(JsValue::Undefined)
                        }
                    }
                    "push" => {
                        let new_len = if let Some(arr) = self.arr_vec_mut(*aid) {
                            arr.extend_from_slice(args);
                            arr.len()
                        } else {
                            0
                        };
                        Ok(JsValue::Number(new_len as f64))
                    }
                    "pop" => Ok(self
                        .arr_vec_mut(*aid)
                        .and_then(|a| a.pop())
                        .unwrap_or(JsValue::Undefined)),
                    "shift" => {
                        if let Some(arr) = self.arr_vec_mut(*aid) {
                            if !arr.is_empty() {
                                return Ok(arr.remove(0));
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    "unshift" => {
                        if let Some(arr) = self.arr_vec_mut(*aid) {
                            for (i, v) in args.iter().enumerate() {
                                arr.insert(i, v.clone());
                            }
                            return Ok(JsValue::Number(arr.len() as f64));
                        }
                        Ok(JsValue::Number(0.0))
                    }
                    "sort" | "toSorted" => {
                        let mut copy = items_snapshot;
                        if args.is_empty() && copy.iter().all(|v| matches!(v, JsValue::Number(_))) {
                            let mut nums: Vec<f64> = copy.iter().map(|v| v.as_number()).collect();
                            fast_f64_sort(&mut nums);
                            copy = nums.into_iter().map(JsValue::Number).collect();
                        } else {
                            copy.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
                        }
                        if method == "toSorted" {
                            Ok(JsValue::Array(self.alloc_arr(copy)?))
                        } else {
                            if let Some(arr) = self.arr_vec_mut(*aid) {
                                *arr = copy;
                            }
                            Ok(JsValue::Array(*aid))
                        }
                    }
                    "reverse" | "toReversed" => {
                        let mut copy = items_snapshot;
                        copy.reverse();
                        if method == "toReversed" {
                            Ok(JsValue::Array(self.alloc_arr(copy)?))
                        } else {
                            if let Some(arr) = self.arr_vec_mut(*aid) {
                                *arr = copy;
                            }
                            Ok(JsValue::Array(*aid))
                        }
                    }
                    "splice" => {
                        let len = items_snapshot.len();
                        let s = args.first().map(|a| a.as_number() as isize).unwrap_or(0);
                        let start = if s < 0 {
                            (len as isize + s).max(0) as usize
                        } else {
                            (s as usize).min(len)
                        };
                        let del_cnt = args
                            .get(1)
                            .map(|a| a.as_number().max(0.0) as usize)
                            .unwrap_or(len.saturating_sub(start))
                            .min(len.saturating_sub(start));
                        let inserts = if args.len() > 2 { &args[2..] } else { &[] };
                        let removed: Vec<JsValue> = if let Some(arr) = self.arr_vec_mut(*aid) {
                            arr.splice(start..start + del_cnt, inserts.iter().cloned())
                                .collect()
                        } else {
                            Vec::new()
                        };
                        Ok(JsValue::Array(self.alloc_arr(removed)?))
                    }
                    "flat" => {
                        let mut out = Vec::with_capacity(items_snapshot.len());
                        for item in items_snapshot {
                            match item {
                                JsValue::Array(sub_id) => {
                                    if let Some(sub) = self.arr_vec(sub_id) {
                                        out.extend(sub.iter().cloned());
                                    }
                                }
                                other => out.push(other),
                            }
                        }
                        Ok(JsValue::Array(self.alloc_arr(out)?))
                    }
                    "flatMap" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        let mut out = Vec::with_capacity(items_snapshot.len());
                        for (idx, item) in items_snapshot.into_iter().enumerate() {
                            let mapped = self.invoke_callable_val(
                                program,
                                &cb,
                                &[item, JsValue::Number(idx as f64)],
                                None,
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                            match mapped {
                                JsValue::Array(sub_id) => {
                                    if let Some(sub) = self.arr_vec(sub_id) {
                                        out.extend(sub.iter().cloned());
                                    }
                                }
                                other => out.push(other),
                            }
                        }
                        Ok(JsValue::Array(self.alloc_arr(out)?))
                    }
                    "fill" => {
                        let fill_val = args.first().cloned().unwrap_or(JsValue::Undefined);
                        if let Some(arr) = self.arr_vec_mut(*aid) {
                            for slot in arr.iter_mut() {
                                *slot = fill_val.clone();
                            }
                        }
                        Ok(JsValue::Array(*aid))
                    }
                    "join" => {
                        let sep = args
                            .first()
                            .map(|a| a.as_str())
                            .unwrap_or_else(|| ",".to_string());
                        let joined: Vec<String> =
                            items_snapshot.iter().map(|v| v.as_str()).collect();
                        Ok(JsValue::Str(joined.join(&sep)))
                    }
                    "includes" => {
                        let target = args.first().cloned().unwrap_or(JsValue::Undefined);
                        Ok(JsValue::Bool(items_snapshot.contains(&target)))
                    }
                    "indexOf" => {
                        let target = args.first().cloned().unwrap_or(JsValue::Undefined);
                        let pos = items_snapshot
                            .iter()
                            .position(|x| x == &target)
                            .map(|i| i as f64)
                            .unwrap_or(-1.0);
                        Ok(JsValue::Number(pos))
                    }
                    "concat" => {
                        let mut out = items_snapshot;
                        for arg in args {
                            match arg {
                                JsValue::Array(other_id) => {
                                    if let Some(other) = self.arr_vec(*other_id) {
                                        out.extend(other.iter().cloned());
                                    }
                                }
                                other => out.push(other.clone()),
                            }
                        }
                        Ok(JsValue::Array(self.alloc_arr(out)?))
                    }
                    "slice" => {
                        let len = items_snapshot.len();
                        let s = args.first().map(|a| a.as_number() as isize).unwrap_or(0);
                        let e = args
                            .get(1)
                            .map(|a| a.as_number() as isize)
                            .unwrap_or(len as isize);
                        let st = if s < 0 {
                            (len as isize + s).max(0) as usize
                        } else {
                            (s as usize).min(len)
                        };
                        let en = if e < 0 {
                            (len as isize + e).max(0) as usize
                        } else {
                            (e as usize).min(len)
                        };
                        let sliced = if st < en {
                            items_snapshot[st..en].to_vec()
                        } else {
                            Vec::new()
                        };
                        Ok(JsValue::Array(self.alloc_arr(sliced)?))
                    }
                    "map" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        let mut out = Vec::with_capacity(items_snapshot.len());
                        for (idx, item) in items_snapshot.into_iter().enumerate() {
                            let mapped = self.invoke_callable_val(
                                program,
                                &cb,
                                &[item, JsValue::Number(idx as f64)],
                                None,
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                            out.push(mapped);
                        }
                        Ok(JsValue::Array(self.alloc_arr(out)?))
                    }
                    "filter" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        let mut out = Vec::new();
                        for (idx, item) in items_snapshot.into_iter().enumerate() {
                            let keep = self.invoke_callable_val(
                                program,
                                &cb,
                                &[item.clone(), JsValue::Number(idx as f64)],
                                None,
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                            if keep.is_truthy() {
                                out.push(item);
                            }
                        }
                        Ok(JsValue::Array(self.alloc_arr(out)?))
                    }
                    "reduce" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        let mut iter = items_snapshot.into_iter().enumerate();
                        let mut acc = if args.len() >= 2 {
                            args[1].clone()
                        } else {
                            iter.next().map(|(_, v)| v).unwrap_or(JsValue::Undefined)
                        };
                        for (idx, item) in iter {
                            acc = self.invoke_callable_val(
                                program,
                                &cb,
                                &[acc, item, JsValue::Number(idx as f64)],
                                None,
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                        }
                        Ok(acc)
                    }
                    "some" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        for (idx, item) in items_snapshot.into_iter().enumerate() {
                            if self
                                .invoke_callable_val(
                                    program,
                                    &cb,
                                    &[item, JsValue::Number(idx as f64)],
                                    None,
                                    origin_url,
                                    dom,
                                    clipboard,
                                    storage,
                                    downloads,
                                )?
                                .is_truthy()
                            {
                                return Ok(JsValue::Bool(true));
                            }
                        }
                        Ok(JsValue::Bool(false))
                    }
                    "every" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        for (idx, item) in items_snapshot.into_iter().enumerate() {
                            if !self
                                .invoke_callable_val(
                                    program,
                                    &cb,
                                    &[item, JsValue::Number(idx as f64)],
                                    None,
                                    origin_url,
                                    dom,
                                    clipboard,
                                    storage,
                                    downloads,
                                )?
                                .is_truthy()
                            {
                                return Ok(JsValue::Bool(false));
                            }
                        }
                        Ok(JsValue::Bool(true))
                    }
                    "forEach" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        for (idx, item) in items_snapshot.into_iter().enumerate() {
                            self.invoke_callable_val(
                                program,
                                &cb,
                                &[item, JsValue::Number(idx as f64)],
                                None,
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                        }
                        Ok(JsValue::Undefined)
                    }
                    "find" | "findIndex" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        let is_idx = method == "findIndex";
                        for (idx, item) in items_snapshot.into_iter().enumerate() {
                            let matched = self.invoke_callable_val(
                                program,
                                &cb,
                                &[item.clone(), JsValue::Number(idx as f64)],
                                None,
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                            if matched.is_truthy() {
                                return Ok(if is_idx {
                                    JsValue::Number(idx as f64)
                                } else {
                                    item
                                });
                            }
                        }
                        Ok(if is_idx {
                            JsValue::Number(-1.0)
                        } else {
                            JsValue::Undefined
                        })
                    }
                    _ => Ok(JsValue::Undefined),
                }
            }
            JsValue::DomNode(nid) => match method {
                "getAttribute" => {
                    let k = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(dom
                        .get_attribute(*nid, &k)
                        .map(JsValue::Str)
                        .unwrap_or(JsValue::Null))
                }
                "setAttribute" => {
                    if args.len() >= 2 {
                        dom.set_attribute(*nid, &args[0].as_str(), &args[1].as_str());
                    }
                    Ok(JsValue::Undefined)
                }
                "removeAttribute" => {
                    if let Some(k) = args.first() {
                        dom.remove_attribute(*nid, &k.as_str());
                    }
                    Ok(JsValue::Undefined)
                }
                "appendChild" => {
                    if let Some(JsValue::DomNode(child_id)) = args.first() {
                        dom.append_child(*nid, *child_id);
                        return Ok(JsValue::DomNode(*child_id));
                    }
                    Ok(JsValue::Undefined)
                }
                "removeChild" => {
                    if let Some(JsValue::DomNode(child_id)) = args.first() {
                        dom.remove_child(*nid, *child_id);
                        return Ok(JsValue::DomNode(*child_id));
                    }
                    Ok(JsValue::Undefined)
                }
                "addEventListener" => {
                    if args.len() >= 2 {
                        let ev = args[0].as_str().to_ascii_lowercase();
                        match &args[1] {
                            JsValue::Fn(cid) => {
                                if !self.pinned_callables.contains(cid) {
                                    self.pinned_callables.push(*cid);
                                }
                                dom.add_event_listener(*nid, &ev, &format!("__manta_fn:{}", cid));
                            }
                            JsValue::Str(code) => {
                                dom.add_event_listener(*nid, &ev, code);
                            }
                            _ => {}
                        }
                    }
                    Ok(JsValue::Undefined)
                }
                "dispatchEvent" => {
                    let ev_type = match args.first() {
                        Some(JsValue::Object(eoid)) => self
                            .obj_map(*eoid)
                            .and_then(|m| m.get("type"))
                            .map(|v| v.as_str())
                            .unwrap_or_else(|| "click".to_string()),
                        Some(other) => other.as_str(),
                        None => "click".to_string(),
                    };
                    let ev_arg = args.first().cloned().unwrap_or(JsValue::DomNode(*nid));
                    for target in dom.event_bubbling_path(*nid) {
                        for handler in dom.get_event_listeners(target, &ev_type) {
                            if let Some(cid_str) = handler.strip_prefix("__manta_fn:") {
                                if let Ok(cid) = cid_str.parse::<u32>() {
                                    let _ = self.invoke_callable_val(
                                        program,
                                        &JsValue::Fn(cid),
                                        &[ev_arg.clone()],
                                        None,
                                        origin_url,
                                        dom,
                                        clipboard,
                                        storage,
                                        downloads,
                                    )?;
                                }
                            } else if let Ok(inline_prog) = JsBytecodeCompiler::compile(&handler) {
                                let _ = self.run_vm_loop(
                                    &inline_prog,
                                    0,
                                    Vec::new(),
                                    0,
                                    Vec::new(),
                                    Vec::new(),
                                    origin_url,
                                    dom,
                                    clipboard,
                                    storage,
                                    downloads,
                                );
                            }
                        }
                    }
                    Ok(JsValue::Bool(true))
                }
                "focus" => {
                    dom.forms.focus_control(*nid);
                    Ok(JsValue::Undefined)
                }
                _ => Ok(JsValue::Undefined),
            },
            JsValue::Object(oid) => {
                let ns = self
                    .obj_map(*oid)
                    .and_then(|m| m.get("__manta_ns"))
                    .map(|v| v.as_str())
                    .unwrap_or_default();

                match (ns.as_str(), method) {
                    ("crypto", "randomUUID") => {
                        let mut r = generate_volatile_entropy_32();
                        r[6] = (r[6] & 0x0F) | 0x40;
                        r[8] = (r[8] & 0x3F) | 0x80;
                        let uuid = format!(
                            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                            r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7],
                            r[8], r[9], r[10], r[11], r[12], r[13], r[14], r[15]
                        );
                        Ok(JsValue::Str(uuid))
                    }
                    ("crypto_subtle", "digest") => {
                        let payload = args.get(1).map(|a| a.as_str()).unwrap_or_default();
                        let hash = sha256(payload.as_bytes());
                        let mut hex = String::with_capacity(64);
                        for b in hash {
                            hex.push_str(&format!("{:02x}", b));
                        }
                        let p = self.alloc_promise(Some(true), JsValue::Str(hex))?;
                        Ok(JsValue::Object(p))
                    }
                    ("abort_ctrl", "abort") => {
                        if let Some(JsValue::Object(sig_oid)) =
                            self.obj_map(*oid).and_then(|m| m.get("signal")).cloned()
                        {
                            if let Some(sm) = self.obj_map_mut(sig_oid) {
                                sm.insert("aborted".to_string(), JsValue::Bool(true));
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("headers" | "formdata", "set" | "append") => {
                        if args.len() >= 2 {
                            let k = if ns == "headers" {
                                args[0].as_str().to_ascii_lowercase()
                            } else {
                                args[0].as_str()
                            };
                            if let Some(m) = self.obj_map_mut(*oid) {
                                m.insert(k, args[1].clone());
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("headers" | "formdata", "get") => {
                        let k = args.first().map(|a| a.as_str()).unwrap_or_default();
                        let lookup = if ns == "headers" {
                            k.to_ascii_lowercase()
                        } else {
                            k
                        };
                        Ok(self
                            .obj_map(*oid)
                            .and_then(|m| m.get(&lookup))
                            .cloned()
                            .unwrap_or(JsValue::Null))
                    }
                    ("headers" | "formdata", "has") => {
                        let k = args.first().map(|a| a.as_str()).unwrap_or_default();
                        let lookup = if ns == "headers" {
                            k.to_ascii_lowercase()
                        } else {
                            k
                        };
                        Ok(JsValue::Bool(
                            self.obj_map(*oid)
                                .map(|m| m.contains_key(&lookup))
                                .unwrap_or(false),
                        ))
                    }
                    ("headers" | "formdata", "delete") => {
                        let k = args.first().map(|a| a.as_str()).unwrap_or_default();
                        let lookup = if ns == "headers" {
                            k.to_ascii_lowercase()
                        } else {
                            k
                        };
                        if let Some(m) = self.obj_map_mut(*oid) {
                            m.remove(&lookup);
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("dom_event", "preventDefault") => {
                        if let Some(m) = self.obj_map_mut(*oid) {
                            m.insert("defaultPrevented".to_string(), JsValue::Bool(true));
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("dom_event", "stopPropagation") => Ok(JsValue::Undefined),
                    ("observer", "observe" | "unobserve" | "disconnect") => Ok(JsValue::Undefined),
                    ("xhr", "open") => {
                        let m_str = args
                            .first()
                            .map(|a| a.as_str().to_ascii_uppercase())
                            .unwrap_or_else(|| "GET".to_string());
                        let url_str = args.get(1).map(|a| a.as_str()).unwrap_or_default();
                        if let Some(m) = self.obj_map_mut(*oid) {
                            m.insert("__method".to_string(), JsValue::Str(m_str));
                            m.insert("__url".to_string(), JsValue::Str(url_str));
                            m.insert("readyState".to_string(), JsValue::Number(1.0));
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("xhr", "setRequestHeader") => {
                        if args.len() >= 2 {
                            if let Some(JsValue::Object(h_oid)) = self
                                .obj_map(*oid)
                                .and_then(|m| m.get("__req_headers"))
                                .cloned()
                            {
                                if let Some(hm) = self.obj_map_mut(h_oid) {
                                    hm.insert(args[0].as_str(), args[1].clone());
                                }
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("xhr", "send") => {
                        let req_body = args
                            .first()
                            .filter(|v| !matches!(v, JsValue::Undefined | JsValue::Null))
                            .map(|v| v.as_str().into_bytes());
                        let (m_str, url_str, hdrs, onload_cb) = {
                            let map = self.obj_map(*oid).cloned().unwrap_or_default();
                            let method = map
                                .get("__method")
                                .map(|v| v.as_str())
                                .unwrap_or_else(|| "GET".to_string());
                            let url = map.get("__url").map(|v| v.as_str()).unwrap_or_default();
                            let mut h = HashMap::new();
                            if !origin_url.is_empty() {
                                h.insert("Origin".to_string(), origin_url.to_string());
                            }
                            if let Some(JsValue::Object(h_oid)) = map.get("__req_headers") {
                                if let Some(hm) = self.obj_map(*h_oid) {
                                    for (k, v) in hm {
                                        if !k.starts_with("__") {
                                            h.insert(k.clone(), v.as_str());
                                        }
                                    }
                                }
                            }
                            (method, url, h, map.get("onload").cloned())
                        };
                        let mut modules = ModuleBus::new();
                        modules.register(Box::new(SiteIsolationModule::new()));
                        let mut body = Vec::new();
                        if let Ok(out) = StreamingNetworkPipe::stream_http_with_body(
                            &m_str,
                            &url_str,
                            hdrs,
                            req_body.as_deref(),
                            &modules,
                            4 * 1024 * 1024,
                            |c, _| body.extend_from_slice(c),
                        ) {
                            if let Some(m) = self.obj_map_mut(*oid) {
                                m.insert("status".to_string(), JsValue::Number(out.status_code as f64));
                                m.insert("readyState".to_string(), JsValue::Number(4.0));
                                m.insert(
                                    "responseText".to_string(),
                                    JsValue::Str(String::from_utf8_lossy(&body).into_owned()),
                                );
                            }
                            if let Some(cb) = onload_cb {
                                let _ = self.invoke_callable_val(
                                    program,
                                    &cb,
                                    &[JsValue::Object(*oid)],
                                    Some(JsValue::Object(*oid)),
                                    origin_url,
                                    dom,
                                    clipboard,
                                    storage,
                                    downloads,
                                );
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("performance", "now") => {
                        let ms = now_us().saturating_sub(self.vm_boot_us) as f64 / 1000.0;
                        Ok(JsValue::Number(ms))
                    }
                    ("window", "matchMedia") => {
                        let q = args.first().map(|a| a.as_str()).unwrap_or_default();
                        let matches = q.contains("prefers-color-scheme: dark")
                            || q.contains("max-width");
                        let mut m = HashMap::new();
                        m.insert("matches".to_string(), JsValue::Bool(matches));
                        m.insert("media".to_string(), JsValue::Str(q));
                        Ok(JsValue::Object(self.alloc_obj_map(m)))
                    }
                    ("classList", "add") => {
                        if let Some(JsValue::DomNode(nid)) =
                            self.obj_map(*oid).and_then(|m| m.get("__nid")).cloned()
                        {
                            for a in args {
                                dom.add_class(nid, &a.as_str());
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("classList", "remove") => {
                        if let Some(JsValue::DomNode(nid)) =
                            self.obj_map(*oid).and_then(|m| m.get("__nid")).cloned()
                        {
                            for a in args {
                                dom.remove_class(nid, &a.as_str());
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("classList", "contains") => {
                        if let Some(JsValue::DomNode(nid)) =
                            self.obj_map(*oid).and_then(|m| m.get("__nid")).cloned()
                        {
                            let cls = args.first().map(|a| a.as_str()).unwrap_or_default();
                            return Ok(JsValue::Bool(dom.node_has_class(nid, &cls)));
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("classList", "toggle") => {
                        if let Some(JsValue::DomNode(nid)) =
                            self.obj_map(*oid).and_then(|m| m.get("__nid")).cloned()
                        {
                            let cls = args.first().map(|a| a.as_str()).unwrap_or_default();
                            if dom.node_has_class(nid, &cls) {
                                dom.remove_class(nid, &cls);
                                return Ok(JsValue::Bool(false));
                            } else {
                                dom.add_class(nid, &cls);
                                return Ok(JsValue::Bool(true));
                            }
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("urlparams", "get") => {
                        let k = args.first().map(|a| a.as_str()).unwrap_or_default();
                        Ok(self
                            .obj_map(*oid)
                            .and_then(|m| m.get(&k))
                            .cloned()
                            .unwrap_or(JsValue::Null))
                    }
                    ("textencoder", "encode") => {
                        let raw = args.first().map(|a| a.as_str()).unwrap_or_default();
                        let vals: Vec<f64> = raw.as_bytes().iter().map(|&b| b as f64).collect();
                        let tid = self.typed_arrays.len() as u32;
                        self.typed_arrays.push(vals);
                        Ok(JsValue::TypedArray(tid))
                    }
                    ("textdecoder", "decode") => {
                        if let Some(JsValue::TypedArray(tid)) = args.first() {
                            let bytes: Vec<u8> = self
                                .typed_arrays
                                .get(*tid as usize)
                                .map(|v| v.iter().map(|&x| x as u8).collect())
                                .unwrap_or_default();
                            return Ok(JsValue::Str(String::from_utf8_lossy(&bytes).into_owned()));
                        }
                        Ok(JsValue::Str(String::new()))
                    }
                    ("fetch_resp", "text") => {
                        let body = self
                            .obj_map(*oid)
                            .and_then(|m| m.get("__body"))
                            .cloned()
                            .unwrap_or_else(|| JsValue::Str(String::new()));
                        let p = self.alloc_promise(Some(true), body)?;
                        Ok(JsValue::Object(p))
                    }
                    ("fetch_resp", "json") => {
                        let body = self
                            .obj_map(*oid)
                            .and_then(|m| m.get("__body"))
                            .map(|v| v.as_str())
                            .unwrap_or_default();
                        let parsed = self.json_parse(&body).unwrap_or(JsValue::Null);
                        let p = self.alloc_promise(Some(true), parsed)?;
                        Ok(JsValue::Object(p))
                    }
                    ("promise", "then" | "catch" | "finally") => {
                        let is_catch = method == "catch";
                        let on_ok = if is_catch { None } else { args.first().cloned() };
                        let on_err = if is_catch {
                            args.first().cloned()
                        } else {
                            args.get(1).cloned()
                        };
                        let child_oid = self.alloc_promise(None, JsValue::Undefined)?;
                        let state_snap = self
                            .promises
                            .get(oid)
                            .map(|r| (r.fulfilled, r.value.clone()));
                        match state_snap {
                            Some((Some(true), val)) => {
                                if let Some(cb) = on_ok {
                                    self.enqueue_timer(
                                        0,
                                        Microtask::Callback {
                                            cb,
                                            arg: val,
                                            resolve_promise_oid: Some(child_oid),
                                        },
                                    );
                                } else {
                                    self.settle_promise(child_oid, true, val);
                                }
                            }
                            Some((Some(false), err)) => {
                                if let Some(cb) = on_err {
                                    self.enqueue_timer(
                                        0,
                                        Microtask::Callback {
                                            cb,
                                            arg: err,
                                            resolve_promise_oid: Some(child_oid),
                                        },
                                    );
                                } else {
                                    self.settle_promise(child_oid, false, err);
                                }
                            }
                            Some((None, _)) => {
                                if let Some(rec) = self.promises.get_mut(oid) {
                                    rec.then_waiters.push((on_ok, on_err, child_oid));
                                }
                            }
                            None => {}
                        }
                        Ok(JsValue::Object(child_oid))
                    }
                    ("document", "getElementById") => {
                        let id = args.first().map(|a| a.as_str()).unwrap_or_default();
                        Ok(dom
                            .find_by_id(&id)
                            .map(JsValue::DomNode)
                            .unwrap_or(JsValue::Null))
                    }
                    ("document", "querySelector") => {
                        let sel = args.first().map(|a| a.as_str()).unwrap_or_default();
                        Ok(dom
                            .query_selector(&sel)
                            .map(JsValue::DomNode)
                            .unwrap_or(JsValue::Null))
                    }
                    ("document", "querySelectorAll") => {
                        let sel = args.first().map(|a| a.as_str()).unwrap_or_default();
                        let nodes: Vec<JsValue> = dom
                            .query_selector_all(&sel)
                            .into_iter()
                            .map(JsValue::DomNode)
                            .collect();
                        Ok(JsValue::Array(self.alloc_arr(nodes)?))
                    }
                    ("document", "createElement") => {
                        let tag = args
                            .first()
                            .map(|a| a.as_str())
                            .unwrap_or_else(|| "div".to_string());
                        Ok(JsValue::DomNode(dom.create_element(&tag)))
                    }
                    ("localStorage", "setItem") => {
                        if args.len() >= 2 {
                            storage.set_local_storage(
                                origin_url,
                                &args[0].as_str(),
                                &args[1].as_str(),
                            );
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("localStorage", "getItem") => {
                        if let Some(k) = args.first() {
                            return Ok(storage
                                .get_local_storage(origin_url, &k.as_str())
                                .map(JsValue::Str)
                                .unwrap_or(JsValue::Null));
                        }
                        Ok(JsValue::Null)
                    }
                    ("localStorage", "removeItem") => {
                        if let Some(k) = args.first() {
                            storage.remove_local_storage(origin_url, &k.as_str());
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("localStorage", "clear") => {
                        storage.clear_local_storage_only(origin_url);
                        Ok(JsValue::Undefined)
                    }
                    ("sessionStorage", "setItem") => {
                        if args.len() >= 2 {
                            storage.set_session_storage(
                                origin_url,
                                &args[0].as_str(),
                                &args[1].as_str(),
                            );
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("sessionStorage", "getItem") => {
                        if let Some(k) = args.first() {
                            return Ok(storage
                                .get_session_storage(origin_url, &k.as_str())
                                .map(JsValue::Str)
                                .unwrap_or(JsValue::Null));
                        }
                        Ok(JsValue::Null)
                    }
                    ("sessionStorage", "removeItem") => {
                        if let Some(k) = args.first() {
                            storage.remove_session_storage(origin_url, &k.as_str());
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("sessionStorage", "clear") => {
                        storage.clear_session_storage_only(origin_url);
                        Ok(JsValue::Undefined)
                    }
                    ("MantaForm", "saveDraft") => {
                        if args.len() >= 2 {
                            return Ok(JsValue::Bool(storage.save_form_draft(
                                origin_url,
                                &args[0].as_str(),
                                &args[1].as_str(),
                            )));
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("MantaForm", "getDraft") => {
                        if let Some(k) = args.first() {
                            return Ok(storage
                                .get_form_draft(origin_url, &k.as_str())
                                .map(JsValue::Str)
                                .unwrap_or(JsValue::Null));
                        }
                        Ok(JsValue::Null)
                    }
                    ("indexedDB", "putObject" | "put") => {
                        if args.len() >= 3 {
                            let ok = storage.put_idb_object(
                                origin_url,
                                &args[0].as_str(),
                                &args[1].as_str(),
                                &args[2].as_str(),
                            );
                            return Ok(JsValue::Bool(ok));
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("indexedDB", "getObject" | "get") => {
                        if args.len() >= 2 {
                            return Ok(storage
                                .get_idb_object(origin_url, &args[0].as_str(), &args[1].as_str())
                                .map(JsValue::Str)
                                .unwrap_or(JsValue::Null));
                        }
                        Ok(JsValue::Null)
                    }
                    ("indexedDB", "deleteObject" | "delete") => {
                        if args.len() >= 2 {
                            return Ok(JsValue::Bool(storage.delete_idb_object(
                                origin_url,
                                &args[0].as_str(),
                                &args[1].as_str(),
                            )));
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("MantaFS", "writeFile") => {
                        if args.len() >= 2 {
                            let mime = args
                                .get(2)
                                .map(|a| a.as_str())
                                .unwrap_or_else(|| "text/plain".to_string());
                            let ok = storage.save_vfs_file(
                                origin_url,
                                &args[0].as_str(),
                                &mime,
                                args[1].as_str().as_bytes(),
                            );
                            return Ok(JsValue::Bool(ok));
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("MantaFS", "readFile") => {
                        if let Some(p) = args.first() {
                            if let Some(bytes) = storage.read_vfs_file(origin_url, &p.as_str()) {
                                return Ok(JsValue::Str(
                                    String::from_utf8_lossy(&bytes).into_owned(),
                                ));
                            }
                        }
                        Ok(JsValue::Null)
                    }
                    ("MantaFS", "deleteFile") => {
                        if let Some(p) = args.first() {
                            return Ok(JsValue::Bool(
                                storage.delete_vfs_file(origin_url, &p.as_str()),
                            ));
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("MantaDownloads", "save") => {
                        if args.len() >= 2 {
                            let mime = args
                                .get(2)
                                .map(|a| a.as_str())
                                .unwrap_or_else(|| "text/plain".to_string());
                            if let Ok(dl_id) = downloads.save_direct_bytes(
                                origin_url,
                                &args[0].as_str(),
                                &mime,
                                args[1].as_str().as_bytes(),
                            ) {
                                return Ok(JsValue::Number(dl_id as f64));
                            }
                        }
                        Ok(JsValue::Number(-1.0))
                    }
                    ("clipboard", "writeText") => {
                        if let Some(v) = args.first() {
                            clipboard.copy_text(&v.as_str());
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("clipboard", "readText") => {
                        Ok(JsValue::Str(clipboard.paste_text().to_string()))
                    }
                    ("console", "log" | "info" | "warn" | "error") => {
                        if self.console_logs.len() < 64 {
                            let joined: Vec<String> = args.iter().map(|a| a.as_str()).collect();
                            self.console_logs.push(joined.join(" "));
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("Math", "max") => Ok(JsValue::Number(
                        args.iter()
                            .map(|a| a.as_number())
                            .fold(f64::NEG_INFINITY, f64::max),
                    )),
                    ("Math", "min") => Ok(JsValue::Number(
                        args.iter()
                            .map(|a| a.as_number())
                            .fold(f64::INFINITY, f64::min),
                    )),
                    ("Math", "floor") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).floor(),
                    )),
                    ("Math", "ceil") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).ceil(),
                    )),
                    ("Math", "round") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).round(),
                    )),
                    ("Math", "abs") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).abs(),
                    )),
                    ("Math", "sqrt") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).sqrt(),
                    )),
                    ("Math", "sin") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).sin(),
                    )),
                    ("Math", "cos") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).cos(),
                    )),
                    ("Math", "tan") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).tan(),
                    )),
                    ("Math", "log") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).ln(),
                    )),
                    ("Math", "exp") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).exp(),
                    )),
                    ("Math", "sign") => {
                        let v = args.first().map(|a| a.as_number()).unwrap_or(0.0);
                        let s = if v > 0.0 {
                            1.0
                        } else if v < 0.0 {
                            -1.0
                        } else {
                            0.0
                        };
                        Ok(JsValue::Number(s))
                    }
                    ("Math", "random") => {
                        let mut x = self.rng_state;
                        if x == 0 {
                            x = 0x2545_F491_4F6C_DD1D;
                        }
                        x ^= x >> 12;
                        x ^= x << 25;
                        x ^= x >> 27;
                        self.rng_state = x;
                        let r = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64
                            / ((1u64 << 53) as f64);
                        Ok(JsValue::Number(r))
                    }
                    ("Math", "pow") => {
                        let a = args.first().map(|x| x.as_number()).unwrap_or(0.0);
                        let b = args.get(1).map(|x| x.as_number()).unwrap_or(1.0);
                        Ok(JsValue::Number(a.powf(b)))
                    }
                    ("Math", "trunc") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).trunc(),
                    )),
                    ("JSON", "stringify") => {
                        let val = args.first().unwrap_or(&JsValue::Undefined);
                        Ok(JsValue::Str(self.json_stringify(val)))
                    }
                    ("JSON", "parse") => {
                        let raw = args.first().map(|a| a.as_str()).unwrap_or_default();
                        Ok(self.json_parse(&raw).unwrap_or(JsValue::Null))
                    }
                    ("Object", "keys" | "values" | "entries") => {
                        if let Some(JsValue::Object(target_oid)) = args.first() {
                            let entries: Vec<(String, JsValue)> = self
                                .obj_map(*target_oid)
                                .map(|m| {
                                    m.iter()
                                        .filter(|(k, _)| !k.starts_with("__"))
                                        .map(|(k, v)| (k.clone(), v.clone()))
                                        .collect()
                                })
                                .unwrap_or_default();
                            let mut items = Vec::with_capacity(entries.len());
                            for (k, v) in entries {
                                match method {
                                    "keys" => items.push(JsValue::Str(k)),
                                    "values" => items.push(v),
                                    _ => items.push(JsValue::Array(
                                        self.alloc_arr(vec![JsValue::Str(k), v])?,
                                    )),
                                }
                            }
                            return Ok(JsValue::Array(self.alloc_arr(items)?));
                        }
                        Ok(JsValue::Array(self.alloc_arr(Vec::new())?))
                    }
                    ("Object", "fromEntries") => {
                        let mut m = HashMap::new();
                        if let Some(JsValue::Array(aid)) = args.first() {
                            if let Some(pairs) = self.arr_vec(*aid).cloned() {
                                for pair in pairs {
                                    if let JsValue::Array(p_aid) = pair {
                                        if let Some(pv) = self.arr_vec(p_aid) {
                                            if pv.len() >= 2 {
                                                m.insert(pv[0].as_str(), pv[1].clone());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Ok(JsValue::Object(self.alloc_obj_map(m)))
                    }
                    ("Object", "freeze") => {
                        Ok(args.first().cloned().unwrap_or(JsValue::Undefined))
                    }
                    ("Object", "create") => {
                        let oid = self.alloc_obj()?;
                        if let Some(proto @ JsValue::Object(_)) = args.first().cloned() {
                            if let Some(m) = self.obj_map_mut(oid) {
                                m.insert("__proto".to_string(), proto);
                            }
                        }
                        Ok(JsValue::Object(oid))
                    }
                    ("Object", "assign") => {
                        if let Some(JsValue::Object(dst_oid)) = args.first().cloned() {
                            for src in &args[1..] {
                                if let JsValue::Object(src_oid) = src {
                                    if let Some(src_map) = self.obj_map(*src_oid).cloned() {
                                        if let Some(dst_map) = self.obj_map_mut(dst_oid) {
                                            for (k, v) in src_map {
                                                if !k.starts_with("__") {
                                                    dst_map.insert(k, v);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            return Ok(JsValue::Object(dst_oid));
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("Array", "isArray") => Ok(JsValue::Bool(matches!(
                        args.first(),
                        Some(JsValue::Array(_))
                    ))),
                    ("Array", "from") => match args.first() {
                        Some(JsValue::Array(aid)) => {
                            let items = self.arr_vec(*aid).cloned().unwrap_or_default();
                            Ok(JsValue::Array(self.alloc_arr(items)?))
                        }
                        Some(JsValue::Set(sid)) => {
                            let items = self.sets.get(*sid as usize).cloned().unwrap_or_default();
                            Ok(JsValue::Array(self.alloc_arr(items)?))
                        }
                        Some(JsValue::Str(s)) => {
                            let items: Vec<JsValue> =
                                s.chars().map(|c| JsValue::Str(c.to_string())).collect();
                            Ok(JsValue::Array(self.alloc_arr(items)?))
                        }
                        _ => Ok(JsValue::Array(self.alloc_arr(Vec::new())?)),
                    },
                    ("Date", "now") => {
                        let ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as f64)
                            .unwrap_or(0.0);
                        Ok(JsValue::Number(ms))
                    }
                    _ => {
                        let member = self.lookup_proto_prop(*oid, method);
                        if let Some(fn_val) = member {
                            self.invoke_callable_val(
                                program,
                                &fn_val,
                                args,
                                Some(JsValue::Object(*oid)),
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )
                        } else {
                            Ok(JsValue::Undefined)
                        }
                    }
                }
            }
            _ => Ok(JsValue::Undefined),
        }
    }

    fn json_stringify(&self, val: &JsValue) -> String {
        match val {
            JsValue::Null | JsValue::Undefined => "null".to_string(),
            JsValue::Bool(b) => b.to_string(),
            JsValue::Number(n) => {
                if n.is_finite() {
                    val.as_str()
                } else {
                    "null".to_string()
                }
            }
            JsValue::Str(s) => format!("\"{}\"", json_escape_str(s)),
            JsValue::Array(aid) => {
                let Some(items) = self.arr_vec(*aid) else {
                    return "[]".to_string();
                };
                let parts: Vec<String> = items.iter().map(|v| self.json_stringify(v)).collect();
                format!("[{}]", parts.join(","))
            }
            JsValue::Object(oid) => {
                let Some(map) = self.obj_map(*oid) else {
                    return "{}".to_string();
                };
                let mut pairs = Vec::new();
                for (k, v) in map {
                    if !k.starts_with("__") {
                        pairs.push(format!(
                            "\"{}\":{}",
                            json_escape_str(k),
                            self.json_stringify(v)
                        ));
                    }
                }
                format!("{{{}}}", pairs.join(","))
            }
            _ => "{}".to_string(),
        }
    }

    fn json_parse(&mut self, input: &str) -> Result<JsValue, &'static str> {
        let mut cursor = 0usize;
        self.parse_json_val(input.as_bytes(), &mut cursor)
    }

    fn parse_json_val(&mut self, b: &[u8], cur: &mut usize) -> Result<JsValue, &'static str> {
        skip_ws(b, cur);
        if *cur >= b.len() {
            return Err("Empty JSON");
        }
        match b[*cur] {
            b'n' if b[*cur..].starts_with(b"null") => {
                *cur += 4;
                Ok(JsValue::Null)
            }
            b't' if b[*cur..].starts_with(b"true") => {
                *cur += 4;
                Ok(JsValue::Bool(true))
            }
            b'f' if b[*cur..].starts_with(b"false") => {
                *cur += 5;
                Ok(JsValue::Bool(false))
            }
            b'"' => Ok(JsValue::Str(parse_json_str(b, cur)?)),
            b'[' => {
                *cur += 1;
                let mut arr = Vec::new();
                skip_ws(b, cur);
                if *cur < b.len() && b[*cur] == b']' {
                    *cur += 1;
                    return Ok(JsValue::Array(self.alloc_arr(arr)?));
                }
                while *cur < b.len() {
                    arr.push(self.parse_json_val(b, cur)?);
                    skip_ws(b, cur);
                    if *cur < b.len() && b[*cur] == b',' {
                        *cur += 1;
                    } else if *cur < b.len() && b[*cur] == b']' {
                        *cur += 1;
                        break;
                    } else {
                        return Err("Malformed JSON array");
                    }
                }
                Ok(JsValue::Array(self.alloc_arr(arr)?))
            }
            b'{' => {
                *cur += 1;
                let oid = self.alloc_obj()?;
                skip_ws(b, cur);
                if *cur < b.len() && b[*cur] == b'}' {
                    *cur += 1;
                    return Ok(JsValue::Object(oid));
                }
                while *cur < b.len() {
                    skip_ws(b, cur);
                    let key = parse_json_str(b, cur)?;
                    skip_ws(b, cur);
                    if *cur >= b.len() || b[*cur] != b':' {
                        return Err("Expected ':' in JSON object");
                    }
                    *cur += 1;
                    let val = self.parse_json_val(b, cur)?;
                    if let Some(m) = self.obj_map_mut(oid) {
                        m.insert(key, val);
                    }
                    skip_ws(b, cur);
                    if *cur < b.len() && b[*cur] == b',' {
                        *cur += 1;
                    } else if *cur < b.len() && b[*cur] == b'}' {
                        *cur += 1;
                        break;
                    } else {
                        return Err("Malformed JSON object");
                    }
                }
                Ok(JsValue::Object(oid))
            }
            _ => {
                let start = *cur;
                while *cur < b.len()
                    && (b[*cur].is_ascii_digit()
                        || matches!(b[*cur], b'-' | b'+' | b'.' | b'e' | b'E'))
                {
                    *cur += 1;
                }
                let s = std::str::from_utf8(&b[start..*cur]).map_err(|_| "Bad UTF8")?;
                let num = s.parse::<f64>().map_err(|_| "Bad JSON number")?;
                Ok(JsValue::Number(num))
            }
        }
    }

    pub fn get_global(&self, name: &str) -> Option<&JsValue> {
        self.scope_ref(0).and_then(|s| s.vars.get(name))
    }

    pub fn reset(&mut self) {
        self.scopes.clear();
        self.free_scope_slots.clear();
        self.scopes.push(Some(ScopeFrame {
            parent: None,
            vars: HashMap::with_capacity(64),
        }));
        self.objects.clear();
        self.free_obj_slots.clear();
        self.arrays.clear();
        self.free_arr_slots.clear();
        self.callables.clear();
        self.pinned_callables.clear();
        self.regexes.clear();
        self.maps.clear();
        self.sets.clear();
        self.typed_arrays.clear();
        self.promises.clear();
        self.timers.clear();
        self.gc_alloc_counter = 0;
        self.console_logs.clear();
        self.fuel_remaining = self.max_fuel;
        self.install_default_globals();
    }
}

fn next_utf8_boundary(s: &str, byte_idx: usize) -> usize {
    let mut i = byte_idx.saturating_add(1);
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn skip_ws(b: &[u8], cur: &mut usize) {
    while *cur < b.len() && b[*cur].is_ascii_whitespace() {
        *cur += 1;
    }
}

fn parse_json_str(b: &[u8], cur: &mut usize) -> Result<String, &'static str> {
    if *cur >= b.len() || b[*cur] != b'"' {
        return Err("Expected JSON string quote");
    }
    *cur += 1;
    let mut s = String::new();
    while *cur < b.len() {
        let byte = b[*cur];
        if byte == b'"' {
            *cur += 1;
            return Ok(s);
        }
        if byte == b'\\' {
            *cur += 1;
            if *cur >= b.len() {
                return Err("Truncated JSON escape");
            }
            match b[*cur] {
                b'n' => s.push('\n'),
                b'r' => s.push('\r'),
                b't' => s.push('\t'),
                other => s.push(other as char),
            }
            *cur += 1;
        } else {
            let rest = std::str::from_utf8(&b[*cur..]).map_err(|_| "Bad UTF8")?;
            let ch = rest.chars().next().unwrap();
            s.push(ch);
            *cur += ch.len_utf8();
        }
    }
    Err("Unterminated JSON string")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let raw = s.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            if let Ok(hex_str) = std::str::from_utf8(&raw[i + 1..i + 3]) {
                if let Ok(b) = u8::from_str_radix(hex_str, 16) {
                    bytes.push(b);
                    i += 3;
                    continue;
                }
            }
        } else if raw[i] == b'+' {
            bytes.push(b' ');
            i += 1;
            continue;
        }
        bytes.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
