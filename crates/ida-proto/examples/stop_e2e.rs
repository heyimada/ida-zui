//! End-to-end check of stopping an analysis that is still running (GAPS S1).
//!
//! Usage: cargo run -p ida-proto --example stop_e2e -- <big-binary-copy> [artifact-dir]
//! (`scripts/e2e.ps1` passes a fresh copy of shell32.dll in its own folder).
//!
//! Opens the copy with analysis on (about two minutes on shell32.dll), waits
//! until the host reports progress, sends `analysis_stop`, then expects the
//! stop to be answered at once, the `open` request to return within seconds
//! instead of running the queues to the end, `close` without saving to be
//! acknowledged, the host process to exit and no database component to be left
//! next to the input. Output: `<artifact-dir>/stop.txt` (no timings, so it can
//! be diffed).

use ida_proto::client::HostConfig;
use ida_proto::*;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DB_PARTS: &[&str] = &["id0", "id1", "id2", "nam", "til", "i64", "idb"];

fn leftovers(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| {
                    n.rsplit_once('.').is_some_and(|(_, ext)| {
                        DB_PARTS.contains(&ext.to_ascii_lowercase().as_str())
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(
        args.next()
            .expect("usage: stop_e2e <binary-copy> [artifact-dir]"),
    );
    let artifacts = PathBuf::from(args.next().unwrap_or_else(|| "artifacts/e2e".into()));
    let dir = input.parent().expect("input has a folder").to_owned();
    assert!(
        leftovers(&dir).is_empty(),
        "the work folder must start without database files"
    );

    let (client, logs, mut events) =
        Client::spawn_with_events(&HostConfig::discover()).expect("spawn host");
    std::thread::spawn(move || futures::executor::block_on_stream(logs).for_each(drop));

    let open = client.call(Open {
        path: input.display().to_string(),
        auto_analyze: true,
        base: None,
    });
    let (open_tx, open_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = open_tx.send(futures::executor::block_on(open));
    });

    // Wait for the analysis to be under way, as the UI's progress bar is.
    let mut report = String::new();
    let started = Instant::now();
    let mut progress = None;
    while progress.is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "no analysis progress within 60 s"
        );
        match events.try_recv() {
            Ok(HostEvent::Analysis(a)) if !a.done => progress = Some(a),
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    let progress = progress.unwrap();
    assert!(
        open_rx.try_recv().is_err(),
        "the open request had already returned: the sample is too small to test a stop"
    );
    writeln!(report, "analysis running when stopped: true").unwrap();
    writeln!(
        report,
        "functions found so far > 0: {}",
        progress.functions > 0
    )
    .unwrap();

    let t = Instant::now();
    client
        .call_blocking(AnalysisStop {})
        .expect("analysis_stop");
    let stop_ms = t.elapsed().as_millis();
    assert!(stop_ms < 2000, "analysis_stop took {stop_ms} ms");
    writeln!(report, "analysis_stop answered within 2 s: true").unwrap();

    let t = Instant::now();
    let opened = open_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("open did not return within 30 s of the stop");
    let open_ms = t.elapsed().as_millis();
    opened.expect("open failed after the stop");
    writeln!(report, "open returned within 30 s of the stop: true").unwrap();

    let mut done_event = false;
    while let Ok(ev) = events.try_recv() {
        if let HostEvent::Analysis(a) = ev {
            done_event |= a.done;
        }
    }
    writeln!(report, "analysis reported finished: {done_event}").unwrap();
    assert!(done_event, "no final analysis event after the stop");

    let t = Instant::now();
    client
        .call_blocking(Close { save: false })
        .expect("close after the stop");
    let exited = client.wait_exit(Duration::from_secs(15));
    let close_ms = t.elapsed().as_millis();
    writeln!(report, "close acknowledged and host exited: {exited}").unwrap();
    assert!(exited, "the host did not exit after close");

    let left = leftovers(&dir);
    writeln!(
        report,
        "database files left next to the input: {}",
        left.len()
    )
    .unwrap();
    assert!(left.is_empty(), "left behind: {left:?}");

    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(artifacts.join("stop.txt"), &report).unwrap();
    print!("{report}");
    println!("stop {stop_ms} ms, open {open_ms} ms after it, close {close_ms} ms");
    println!("STOP E2E OK");
}
