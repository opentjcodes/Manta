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

    let cxx = resolve_cxx(&target, &target_under);
    let ar = resolve_ar(&target, &target_under);

    let obj_path = out_dir.join("manta_accel.o");
    let lib_path = out_dir.join("libmanta_cpp.a");

    let mut compile_cmd = Command::new(&cxx);
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

    if target.contains("armv7") {
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
    if target.contains("linux") && !target.contains("android") {
        println!("cargo:rustc-link-lib=dl");
    } else if target.contains("android") {
        println!("cargo:rustc-link-lib=dl");
    }
}

fn resolve_cxx(target: &str, target_under: &str) -> String {
    for key in [
        format!("CXX_{}", target_under),
        format!("CXX_{}", target),
        "TARGET_CXX".to_string(),
        "CXX".to_string(),
    ] {
        if let Ok(val) = env::var(&key) {
            if !val.trim().is_empty() {
                return val;
            }
        }
    }

    // If cargo-ndk or environment set CC_* (e.g. aarch64-linux-android21-clang), derive clang++
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
            if trimmed.ends_with("gcc") {
                return format!("{}++", &trimmed[..trimmed.len() - 3]);
            }
        }
    }

    // Fallback to ANDROID_NDK_HOME toolchain
    if target.contains("android") {
        if let Ok(ndk_home) = env::var("ANDROID_NDK_HOME") {
            let bin_dir = Path::new(&ndk_home).join("toolchains/llvm/prebuilt/linux-x86_64/bin");
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
            if !val.trim().is_empty() {
                return val;
            }
        }
    }

    if target.contains("android") {
        if let Ok(ndk_home) = env::var("ANDROID_NDK_HOME") {
            let candidate =
                Path::new(&ndk_home).join("toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar");
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
