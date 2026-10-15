//! Only process bootstrap lives outside generated product instrumentation.
include!(concat!(env!("OUT_DIR"), "/instrumented/main.rs"));

fn main() {
    let install = platform::default_install();
    let _telemetry = coreq::telemetry::capture::Runtime::install(
        &install.join("telemetry.sqlite3"), "taskq", platform::VERSION,
    ).map_err(|e| eprintln!("Telemetry: unknown ({e})")).ok();
    application();
}
