//! Binds llama.cpp (llama, mtmd, ggml) with bindgen and links its shared libraries. The engine is upstream's own
//! prebuilt release, fetched by `scripts/fetch-engine.sh` into `~/.local/share/omafind/engine/current` (`lib/` + `include/`);
//! `OMAFIND_LLAMA_DIR` points at another such prefix, and an installed llama.cpp package (`llama.pc`) is used when neither
//! is there.

use std::path::PathBuf;
use std::process::Command;

fn pkg_config(args: &[&str]) -> Option<String> {
    let output = Command::new("pkg-config")
        .args(args)
        .arg("llama")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn fetched_engine() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    let dir = data.join("omafind/engine/current");
    dir.join("include/llama.h").exists().then_some(dir)
}

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=OMAFIND_LLAMA_DIR");
    // A fetch replaces the `current` link: build against the new engine.
    if let Some(data) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    {
        println!(
            "cargo:rerun-if-changed={}",
            data.join("omafind/engine/current").display()
        );
    }

    // (include dirs, lib dirs, version, lib dir to bake in for loading backends at run time)
    let prefix = std::env::var_os("OMAFIND_LLAMA_DIR")
        .map(PathBuf::from)
        .or_else(fetched_engine);
    let (include_dirs, lib_dirs, version): (Vec<PathBuf>, Vec<PathBuf>, String) = if let Some(
        prefix,
    ) = &prefix
    {
        let version =
            std::fs::read_to_string(prefix.join("VERSION")).unwrap_or_else(|_| "custom".into());
        (
            vec![prefix.join("include")],
            vec![prefix.join("lib")],
            version.trim().to_owned(),
        )
    } else if let Some(cflags) = pkg_config(&["--cflags-only-I"]) {
        // Builds, but against whatever llama.cpp the system has rather than the pinned tag: say so.
        println!(
            "cargo:warning=no engine in ~/.local/share/omafind/engine/current: building against the system llama.cpp \
             (run scripts/fetch-engine.sh for the pinned one)"
        );
        let includes = cflags
            .split_whitespace()
            .filter_map(|f| f.strip_prefix("-I"))
            .map(PathBuf::from)
            .collect();
        let libs = pkg_config(&["--libs-only-L"])
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|f| f.strip_prefix("-L"))
            .map(PathBuf::from)
            .collect();
        (
            includes,
            libs,
            pkg_config(&["--modversion"]).unwrap_or_default(),
        )
    } else {
        println!(
            "cargo:warning=no llama.cpp engine: run scripts/fetch-engine.sh, or set OMAFIND_LLAMA_DIR, or install a llama.cpp package"
        );
        (
            vec![PathBuf::from("/usr/include")],
            vec![],
            "unknown".into(),
        )
    };

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut builder = bindgen::Builder::default()
        .header(manifest.join("wrapper.h").to_string_lossy())
        .allowlist_function("(llama|mtmd|ggml_backend|ggml_log)_.*")
        .allowlist_type("(llama|mtmd|ggml)_.*")
        .allowlist_var("(LLAMA|MTMD|GGML)_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false);
    for dir in &include_dirs {
        builder = builder.clang_arg(format!("-I{}", dir.display()));
        println!("cargo:rerun-if-changed={}", dir.display());
    }
    let bindings = builder
        .generate()
        .expect("bindgen over llama.h, mtmd.h and ggml-backend.h");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings.write_to_file(out.join("bindings.rs")).unwrap();

    for dir in &lib_dirs {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    // A fetched engine isn't on the loader path: its location is baked into every binary. A package install sets
    // its own (`OMAFIND_LLAMA_RPATH`, /usr/lib/omafind/engine for the bundled copy) instead of the build directory.
    // A library's link args don't reach other crates' binaries, so the path goes out as `links` metadata
    // (DEP_LLAMA_RPATH) and each binary crate's build.rs bakes it in.
    let rpath: Vec<String> = match std::env::var("OMAFIND_LLAMA_RPATH") {
        Ok(rpath) if !rpath.is_empty() => vec![rpath],
        _ if prefix.is_some() => lib_dirs.iter().map(|d| d.display().to_string()).collect(),
        _ => Vec::new(),
    };
    for dir in &rpath {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
    println!("cargo:rpath={}", rpath.join(":"));
    println!("cargo:rerun-if-env-changed=OMAFIND_LLAMA_RPATH");
    for name in ["llama", "mtmd", "ggml", "ggml-base"] {
        println!("cargo:rustc-link-lib=dylib={name}");
    }
    // Where ggml's backend modules (libggml-vulkan.so, libggml-cpu-*.so) are, when the engine is a prebuilt
    // directory rather than a package that installed them where ggml looks on its own.
    let backends = lib_dirs
        .first()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=OMAFIND_ENGINE_LIB={backends}");
    println!("cargo:rustc-env=OMAFIND_LLAMA_VERSION={version}");
}
