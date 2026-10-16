//! Only process bootstrap lives outside generated product instrumentation.
include!(concat!(env!("OUT_DIR"), "/instrumented/main.rs"));

fn main() {
    let install = platform::default_install();
    let _telemetry = telemetry::budget(&install,"telemetry.sqlite3").and_then(|budget| coreq::telemetry::capture::Runtime::install_bounded(
        &install.join("telemetry.log/universal.sqlite3"), "taskq", platform::VERSION, budget,
    )).map_err(|e| eprintln!("Telemetry: unknown ({e})")).ok();
    application();
}
