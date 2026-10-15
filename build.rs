use std::process::Command;

fn main() {
    let root=std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    coreq_instrument::generate(&root.join("src"),"main.rs",&std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("instrumented")).expect("automatic telemetry source instrumentation");
    let git = |args: &[&str]| Command::new("git").args(args).output().ok()
        .filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    for name in [Some("HEAD".into()), git(&["symbolic-ref", "-q", "HEAD"])].into_iter().flatten() {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let sha = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=TASKQ_REVISION={sha}");
}
