pub mod compiler;
pub mod lexer;
pub mod value;
pub mod vm;

pub use compiler::JsBytecodeCompiler;
pub use value::{CompiledJsProgram, JsFunctionDef, JsParam, JsValue, OpCode};
pub use vm::MantaJsSandbox;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::ClipboardSandbox;
    use crate::dom_arena::ZeroCopyDomArena;
    use crate::downloads_engine::MantaDownloadManager;
    use crate::html_tokenizer::StreamingHtmlTokenizer;
    use crate::storage_engine::MantaStorageEngine;

    #[test]
    fn test_modular_js_engine_web_apis_crypto_gc_and_fixed_budget() {
        let mut dom = ZeroCopyDomArena::new(128 * 1024);
        let mut tokenizer = StreamingHtmlTokenizer::new();
        tokenizer.feed_chunk(
            b"<body><div id=\"mount\"></div><input id=\"email\" type=\"email\" value=\"\" /><button id=\"btn\">Go</button></body>",
            true,
            |tok| dom.ingest_token(tok),
        );

        let mut vm = MantaJsSandbox::new(200_000);
        let mut clip = ClipboardSandbox::new(1024);
        let mut storage = MantaStorageEngine::new(128 * 1024, 512 * 1024);
        let mut downloads = MantaDownloadManager::new("", 1024 * 1024);

        let script = r#"
            class Engine {
                constructor(name) { this.name = name; }
                tag() { return `[${this.name}]`; }
            }
            class MantaCore extends Engine {
                constructor(name, ver) { super(name); this.ver = ver; }
                describe() { return `${this.tag()} v${this.ver}`; }
            }
            const core = new MantaCore("Manta", 4);
            let clsOut = core.describe();

            const mergedObj = { a: 1, b: 2, ...{ c: 3 } };
            let objLitSum = mergedObj.a + mergedObj.b + mergedObj.c;
            const cloned = structuredClone(mergedObj);
            cloned.a = 99;
            let cloneIsolated = mergedObj.a;

            // Modern Web APIs: AbortController, Headers, FormData, CustomEvent, Crypto
            const ac = new AbortController();
            ac.abort();
            let aborted = ac.signal.aborted;

            const hdrs = new Headers({ "Content-Type": "application/json" });
            let ct = hdrs.get("content-type");

            const fd = new FormData();
            fd.append("user", "manta");
            let fdUser = fd.get("user");

            let uuidLen = crypto.randomUUID().length;
            let digestHex = "";
            crypto.subtle.digest("SHA-256", "abc").then((h) => { digestHex = h; });

            document.getElementById("mount").innerHTML = "<span id=\"injected\">Live DOM</span>";
            let injectedTxt = document.getElementById("injected").textContent;

            // Scope GC stress loop
            function inc(x) { return x + 1; }
            let loopTotal = 0;
            for (let i = 0; i < 600; i++) {
                loopTotal = inc(loopTotal);
            }

            let asyncRes = 0;
            async function computeAsync(base) {
                let added = await Promise.resolve(15);
                return base + added;
            }
            computeAsync(35).then((v) => { asyncRes = v; });

            let btn = document.getElementById("btn");
            let evDetail = "";
            btn.addEventListener("custom", (ev) => { evDetail = ev.detail; });
            btn.dispatchEvent(new CustomEvent("custom", { detail: "ok_40" }));

            indexedDB.putObject("users", "u1", JSON.stringify({ email: "dev@manta.browser" }));
        "#;

        vm.execute_script_full(
            script,
            "https://app.manta.dev/index.html",
            &mut dom,
            &mut clip,
            &mut storage,
            &mut downloads,
        )
        .unwrap();

        assert_eq!(
            vm.get_global("clsOut"),
            Some(&JsValue::Str("[Manta] v4".to_string()))
        );
        assert_eq!(vm.get_global("objLitSum"), Some(&JsValue::Number(6.0)));
        assert_eq!(vm.get_global("cloneIsolated"), Some(&JsValue::Number(1.0)));
        assert_eq!(vm.get_global("aborted"), Some(&JsValue::Bool(true)));
        assert_eq!(
            vm.get_global("ct"),
            Some(&JsValue::Str("application/json".to_string()))
        );
        assert_eq!(
            vm.get_global("fdUser"),
            Some(&JsValue::Str("manta".to_string()))
        );
        assert_eq!(vm.get_global("uuidLen"), Some(&JsValue::Number(36.0)));
        assert_eq!(
            vm.get_global("digestHex"),
            Some(&JsValue::Str(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string()
            ))
        );
        assert_eq!(
            vm.get_global("injectedTxt"),
            Some(&JsValue::Str("Live DOM".to_string()))
        );
        assert_eq!(vm.get_global("loopTotal"), Some(&JsValue::Number(600.0)));
        assert_eq!(vm.get_global("asyncRes"), Some(&JsValue::Number(50.0)));
        assert_eq!(
            vm.get_global("evDetail"),
            Some(&JsValue::Str("ok_40".to_string()))
        );
        assert_eq!(
            storage.get_idb_object("https://app.manta.dev", "users", "u1"),
            Some("{\"email\":\"dev@manta.browser\"}".to_string())
        );

        // Fixed-latency infinite loop guard
        vm.set_time_budget_us(2_000);
        assert!(vm
            .execute_script_full(
                "while (true) {}",
                "https://app.manta.dev",
                &mut dom,
                &mut clip,
                &mut storage,
                &mut downloads,
            )
            .is_err());
    }
}
