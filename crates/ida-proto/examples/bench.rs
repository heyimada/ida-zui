//! Open + auto-analysis timing on fresh copies of a binary.
//!
//! Usage: cargo run -p ida-proto --example bench -- <binary> [runs]
//! Every run gets a new directory under testdata/bench so no old database is
//! reused. IZS_AUTO_MODE=native (IDA opens and analyses in one call) or
//! IZS_AUTO_MODE=wait (auto_wait after opening) time those instead of the
//! stepped analysis; IZS_AUTO_TRACE=1 prints the stage trace.

use ida_proto::client::HostConfig;
use ida_proto::*;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let binary = PathBuf::from(args.next().expect("usage: bench <binary> [runs]"));
    let runs: usize = args.next().and_then(|r| r.parse().ok()).unwrap_or(2);
    let name = binary.file_name().unwrap().to_owned();
    for run in 1..=runs {
        let dir = PathBuf::from("testdata")
            .join("bench")
            .join(format!("{}-{run}", name.to_string_lossy()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let copy = dir.join(&name);
        std::fs::copy(&binary, &copy).unwrap();
        let (c, logs, mut events) =
            Client::spawn_with_events(&HostConfig::discover()).expect("spawn host");
        let trace = std::thread::spawn(move || {
            for l in futures::executor::block_on_stream(logs) {
                if l.starts_with("auto:") {
                    println!("  {l}");
                }
            }
        });
        let t0 = std::time::Instant::now();
        let info = c
            .call_blocking(Open {
                path: copy.display().to_string(),
                auto_analyze: true,
                base: None,
            })
            .expect("open");
        let secs = t0.elapsed().as_secs_f32();
        let funcs = c.call_blocking(Functions {}).unwrap().len();
        let mut analysis = Vec::new();
        while let Ok(e) = events.try_recv() {
            if let HostEvent::Analysis(a) = e {
                analysis.push(a);
            }
        }
        let last = analysis.last().map(|a| (a.percent, a.done, a.functions));
        println!(
            "{} run {run}: open+analysis {secs:.2}s, {funcs} functions, max_ea {:#x}, {} analysis events, last {last:?}",
            name.to_string_lossy(),
            info.max_ea,
            analysis.len()
        );
        let _ = c.call_blocking(Close { save: false });
        drop(c);
        let _ = trace.join();
    }
}
