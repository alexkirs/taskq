//! Only process bootstrap lives outside generated product instrumentation.
include!(concat!(env!("OUT_DIR"), "/instrumented/main.rs"));

fn main() {
    use std::io::Write;
    let install = platform::default_install();
    let maintenance=Args::parse(&std::env::args().skip(1).collect::<Vec<_>>()).is_ok_and(|a|a.pos==["profile","migrate"]);
    let _telemetry = (!maintenance).then(||telemetry::budget(&install,"telemetry.sqlite3").and_then(|budget| coreq::telemetry::capture::Runtime::install_bounded(
        &install.join("telemetry.log/universal.sqlite3"), "taskq", platform::VERSION, budget,
    ))).transpose().map_err(|e| {let _=writeln!(std::io::stderr(),"Telemetry: unknown ({e})");}).ok();
    application();
}
