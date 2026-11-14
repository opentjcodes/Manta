use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap_or_else(|_| String::from("x86_64-unknown-linux-gnu"));
    let target_under = target.replace('-', "_");

    let cpp_src = manifest_dir.join("../cpp-accel/src/manta_accel.cpp");
    let cpp_inc = manifest_dir.join("../cpp-accel/include");

    println!("cargo:rerun-if-changed={}", cpp_src.display());
    println!(
        "cargo:rerun-if-changed={}",
        cpp_inc.join("manta_accel.h").display()
    );
    for env_key in [
        format!("CXX_{}", target_under),
        format!("CXX_{}", target),
        format!("CXXFLAGS_{}", target_under),
        format!("CXXFLAGS_{}", target),
        "CXX".to_string(),
        "CXXFLAGS".to_string(),
        "ANDROID_NDK_HOME".to_string(),
        "ANDROID_NDK_ROOT".to_string(),
    ] {
        println!("cargo:rerun-if-env-changed={}", env_key);
    }

    let cxx = resolve_cxx(&target, &target_under);
    let ar = resolve_ar(&target, &target_under);
    let extra_flags = collect_cxxflags(&target, &target_under);

    let obj_path = out_dir.join("manta_accel.o");
    let lib_path = out_dir.join("libmanta_cpp.a");

    let mut compile_cmd = Command::new(&cxx);
    for flag in &extra_flags {
        compile_cmd.arg(flag);
    }

    // Ensure Android builds always pass an explicit `--target=<triple><api>` when using generic NDK clang++
    if target.contains("android") {
        let cxx_name = Path::new(&cxx)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let has_target_in_name = cxx_name.contains("android");
        let has_target_in_flags = extra_flags
            .iter()
            .any(|f| f == "--target" || f.starts_with("--target="));

        if !has_target_in_name && !has_target_in_flags {
            let api = env::var("CARGO_NDK_ANDROID_PLATFORM")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| String::from("21"));
            let clang_triple = if target.starts_with("armv7") {
                "armv7a-linux-androideabi"
            } else {
                target.as_str()
            };
            compile_cmd.arg(format!("--target={}{}", clang_triple, api.trim()));
        }
    }

    compile_cmd
        .arg("-std=c++17")
        .arg("-O3")
        .arg("-fPIC")
        .arg("-fno-exceptions")
        .arg("-fno-rtti")
        .arg("-ffunction-sections")
        .arg("-fdata-sections")
        .arg("-Wall")
        .arg("-Wextra")
        .arg(format!("-I{}", cpp_inc.display()))
        .arg("-c")
        .arg(&cpp_src)
        .arg("-o")
        .arg(&obj_path);

    if target.contains("armv7") && !extra_flags.iter().any(|f| f == "-mthumb") {
        compile_cmd.arg("-mthumb");
    }

    let status = compile_cmd
        .status()
        .unwrap_or_else(|e| panic!("Failed launching C++ compiler `{}`: {}", cxx, e));
    assert!(
        status.success(),
        "C++ compiler `{}` failed compiling {}",
        cxx,
        cpp_src.display()
    );

    if lib_path.exists() {
        let _ = std::fs::remove_file(&lib_path);
    }

    let ar_status = Command::new(&ar)
        .arg("rcs")
        .arg(&lib_path)
        .arg(&obj_path)
        .status()
        .unwrap_or_else(|e| panic!("Failed launching archiver `{}`: {}", ar, e));
    assert!(
        ar_status.success(),
        "Archiver `{}` failed creating {}",
        ar,
        lib_path.display()
    );

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=manta_cpp");
    if target.contains("linux") || target.contains("android") {
        println!("cargo:rustc-link-lib=dl");
    }
}

fn collect_cxxflags(target: &str, target_under: &str) -> Vec<String> {
    for key in [
        format!("CXXFLAGS_{}", target_under),
        format!("CXXFLAGS_{}", target),
        "TARGET_CXXFLAGS".to_string(),
        "CXXFLAGS".to_string(),
    ] {
        if let Ok(val) = env::var(&key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return trimmed.split_whitespace().map(|s| s.to_string()).collect();
            }
        }
    }
    Vec::new()
}

fn ndk_prebuilt_bin_dir() -> Option<PathBuf> {
    for ndk_var in ["ANDROID_NDK_ROOT", "ANDROID_NDK_HOME"] {
        if let Ok(ndk_home) = env::var(ndk_var) {
            let prebuilt = Path::new(&ndk_home).join("toolchains/llvm/prebuilt");
            if let Ok(entries) = std::fs::read_dir(&prebuilt) {
                for entry in entries.flatten() {
                    let bin = entry.path().join("bin");
                    if bin.is_dir() {
                        return Some(bin);
                    }
                }
            }
        }
    }
    None
}

fn resolve_cxx(target: &str, target_under: &str) -> String {
    for key in [
        format!("CXX_{}", target_under),
        format!("CXX_{}", target),
        "TARGET_CXX".to_string(),
        "CXX".to_string(),
    ] {
        if let Ok(val) = env::var(&key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() && !(target.contains("android") && matches!(trimmed, "c++" | "g++"))
            {
                return trimmed.to_string();
            }
        }
    }

    for key in [
        format!("CC_{}", target_under),
        format!("CC_{}", target),
        "TARGET_CC".to_string(),
        "CC".to_string(),
    ] {
        if let Ok(val) = env::var(&key) {
            let trimmed = val.trim();
            if trimmed.ends_with("clang") {
                return format!("{}++", trimmed);
            }
            if trimmed.ends_with("gcc") && !target.contains("android") {
                return format!("{}++", &trimmed[..trimmed.len() - 3]);
            }
        }
    }

    if target.contains("android") {
        if let Some(bin_dir) = ndk_prebuilt_bin_dir() {
            let prefix = if target.starts_with("armv7") {
                "armv7a-linux-androideabi"
            } else {
                target
            };
            for api in [24, 21, 26, 28, 29, 30, 31, 33, 34, 35] {
                let candidate = bin_dir.join(format!("{}{}-clang++", prefix, api));
                if candidate.exists() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
            let generic_clangxx = bin_dir.join("clang++");
            if generic_clangxx.exists() {
                return generic_clangxx.to_string_lossy().into_owned();
            }
        }
    }

    if command_exists("c++") {
        "c++".to_string()
    } else if command_exists("g++") {
        "g++".to_string()
    } else {
        "clang++".to_string()
    }
}

fn resolve_ar(target: &str, target_under: &str) -> String {
    for key in [
        format!("AR_{}", target_under),
        format!("AR_{}", target),
        "TARGET_AR".to_string(),
        "AR".to_string(),
    ] {
        if let Ok(val) = env::var(&key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    if target.contains("android") {
        if let Some(bin_dir) = ndk_prebuilt_bin_dir() {
            let candidate = bin_dir.join("llvm-ar");
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    if command_exists("ar") {
        "ar".to_string()
    } else {
        "llvm-ar".to_string()
    }
}

fn command_exists(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
