//! Bakes the llama.cpp engine's location (from omafind-embed's build script) into this crate's binaries, so they
//! load the pinned engine rather than whatever llama.cpp the system has.

fn main() {
    println!("cargo:rerun-if-env-changed=DEP_LLAMA_RPATH");
    for dir in std::env::var("DEP_LLAMA_RPATH")
        .unwrap_or_default()
        .split(':')
        .filter(|d| !d.is_empty())
    {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{dir}");
    }
}
