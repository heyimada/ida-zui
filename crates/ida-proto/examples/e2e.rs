//! End-to-end check of the host protocol against a real binary.
//!
//! Usage: cargo run -p ida-proto --example e2e -- <binary> [artifact-dir]
//! (build the workspace first so `ida-host.exe` exists).
//!
//! Opens the binary through `ida-host.exe`, exercises every read method and a
//! rename/comment/undo round trip, asserts on the results and writes a
//! deterministic dump to `<artifact-dir>/<binary>.txt` for diffing across runs.

use ida_proto::client::HostConfig;
use ida_proto::*;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Files IDA leaves next to the input while a database is unpacked.
fn unpacked_files(input: &std::path::Path) -> Vec<String> {
    let dir = input.parent().unwrap();
    let name = input.file_name().unwrap().to_string_lossy().into_owned();
    ["id0", "id1", "id2", "nam", "til"]
        .iter()
        .map(|ext| format!("{name}.{ext}"))
        .filter(|f| dir.join(f).exists())
        .collect()
}

/// Four AArch64 instructions that we control: mov x0,#1 / add x1,x0,#2 / nop / ret.
const ARM64_CODE: [u8; 16] = [
    0x20, 0x00, 0x80, 0xD2, 0x01, 0x08, 0x00, 0x91, 0x1F, 0x20, 0x03, 0xD5, 0xC0, 0x03, 0x5F, 0xD6,
];

/// Loader options: opens a raw blob with the Binary loader as ARM at a chosen
/// base, in its own host, and checks the processor and the disassembly.
fn raw_blob_as_arm(binary: &std::path::Path, out: &mut String, check: &mut impl FnMut(bool, &str)) {
    let dir = binary.parent().unwrap().join("raw");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let blob = dir.join("blob.bin");
    let code: Vec<u8> = ARM64_CODE.iter().copied().cycle().take(16 * 24).collect();
    std::fs::write(&blob, &code).unwrap();
    // Same mechanism as the app: a private copy under the processor's extension.
    let input = dir.join(format!("blob.bin.{}", client::processor_extension("arm")));
    std::fs::copy(&blob, &input).unwrap();
    let cfg = HostConfig::discover()
        .raw_binary(&dir.join("idausr"), &["arm", "mips"])
        .expect("write IDAUSR");
    let (c, _logs) = Client::spawn(&cfg).expect("spawn raw host");
    let base = 0x0800_0000u64;
    let opened = c.call_blocking(Open {
        path: input.display().to_string(),
        auto_analyze: false,
        base: Some(base),
    });
    writeln!(out, "\n== raw blob as arm").unwrap();
    let info = match opened {
        Ok(i) => i,
        Err(e) => {
            writeln!(out, "open failed: {e}").unwrap();
            check(false, "the raw blob opens with the Binary loader");
            return;
        }
    };
    writeln!(
        out,
        "processor={} bitness={} file_type={} min={:#x}",
        info.processor, info.bitness, info.file_type, info.min_ea
    )
    .unwrap();
    check(
        info.processor.eq_ignore_ascii_case("arm"),
        "the forced processor is ARM",
    );
    check(
        info.min_ea == base,
        "rebase moves the blob to the requested base",
    );
    check(
        info.file_type.to_lowercase().contains("binary"),
        "the Binary loader was used",
    );
    c.call_blocking(MakeCode { ea: base }).unwrap();
    let list = c
        .call_blocking(ListingDown {
            ea: base,
            count: 40,
        })
        .unwrap();
    let text: Vec<String> = list.lines.iter().map(|l| tags::strip(&l.tagged)).collect();
    for l in &text {
        writeln!(out, "{l}").unwrap();
    }
    let joined = text.join("\n").to_uppercase();
    check(
        joined.contains("MOV") && joined.contains("X0"),
        "the first instruction disassembles as ARM64 mov x0",
    );
    check(
        joined.contains("ADD") && joined.contains("X1"),
        "the second instruction disassembles as ARM64 add x1",
    );
    let asm = c.call_blocking(Assemble {
        ea: base,
        text: "nop".into(),
        pad: false,
    });
    writeln!(out, "assemble on arm: {asm:?}").unwrap();
    check(
        matches!(&asm, Err(ClientError::Ida(m)) if m.contains("no assembler")),
        "assemble names the missing assembler on a processor without one",
    );
    let _ = c.call_blocking(Close { save: false });
}

/// Raw blobs for processors whose modules need a key the map can only carry
/// in a short extension (IDA drops longer ones): each opens as the forced
/// processor and a few instructions of that architecture disassemble.
fn raw_blobs_more(binary: &std::path::Path, out: &mut String, check: &mut impl FnMut(bool, &str)) {
    // (processor name, procname IDA reports, code, mnemonics expected in the listing)
    let cases: [(&str, &str, &[u8], &[&str]); 3] = [
        // mov d15,1 / nop / ret
        (
            "tricore",
            "tricore",
            &[0xDA, 0x01, 0x00, 0x00, 0x00, 0x90],
            &["MOV", "RET"],
        ),
        // entry a1,32 / nop / ret
        (
            "xtensa",
            "xtensa",
            &[0x36, 0x41, 0x00, 0xF0, 0x20, 0x00, 0x80, 0x00, 0x00],
            &["ENTRY", "RET"],
        ),
        // mov #1,r15 / nop / ret
        (
            "msp430",
            "msp430",
            &[0x1F, 0x43, 0x03, 0x43, 0x30, 0x41],
            &["MOV", "RET"],
        ),
    ];
    let names: Vec<&str> = ["arm", "mipsl", "tricore", "msp430", "xtensa"].to_vec();
    for (proc, expect, code, mnemonics) in cases {
        let dir = binary.parent().unwrap().join(format!("raw_{proc}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join(format!("blob.bin.{}", client::processor_extension(proc)));
        std::fs::write(&input, code).unwrap();
        let cfg = HostConfig::discover()
            .raw_binary(&dir.join("idausr"), &names)
            .expect("write IDAUSR");
        let (c, _logs) = Client::spawn(&cfg).expect("spawn raw host");
        writeln!(
            out,
            "
== raw blob as {proc}"
        )
        .unwrap();
        let info = match c.call_blocking(Open {
            path: input.display().to_string(),
            auto_analyze: false,
            base: None,
        }) {
            Ok(i) => i,
            Err(e) => {
                writeln!(out, "open failed: {e}").unwrap();
                check(false, &format!("the {proc} blob opens"));
                continue;
            }
        };
        writeln!(out, "processor={} bitness={}", info.processor, info.bitness).unwrap();
        check(
            info.processor.eq_ignore_ascii_case(expect),
            &format!("the forced processor is {proc}"),
        );
        c.call_blocking(MakeCode { ea: info.min_ea }).unwrap();
        let list = c
            .call_blocking(ListingDown {
                ea: info.min_ea,
                count: 60,
            })
            .unwrap();
        let text: Vec<String> = list.lines.iter().map(|l| tags::strip(&l.tagged)).collect();
        for l in &text {
            writeln!(out, "{l}").unwrap();
        }
        let joined = text
            .join(
                "
",
            )
            .to_uppercase();
        check(
            mnemonics.iter().all(|m| joined.contains(m)),
            &format!("the {proc} blob disassembles ({})", mnemonics.join(", ")),
        );
        let _ = c.call_blocking(Close { save: false });
    }
}

const MARKER_PLUGIN: &str = r#"
import ida_idaapi, ida_kernwin, ida_ida, idc

class izs_marker_t(ida_idaapi.plugin_t):
    flags = 0
    comment = "ida-zui test plugin"
    help = "writes a marker comment"
    wanted_name = "izs test marker"
    wanted_hotkey = "Ctrl-Alt-Shift-Z"
    def init(self):
        return ida_idaapi.PLUGIN_KEEP
    def run(self, arg):
        ida_kernwin.msg("izs-marker plugin ran, arg=%d\n" % arg)
        idc.set_cmt(ida_ida.inf_get_min_ea(), "izs-plugin-marker", 0)

def PLUGIN_ENTRY():
    return izs_marker_t()
"#;

const GUI_PLUGIN: &str = r#"
import ida_idaapi

class izs_gui_t(ida_idaapi.plugin_t):
    flags = 0
    comment = "ida-zui test plugin that needs Qt"
    help = ""
    wanted_name = "izs test gui"
    wanted_hotkey = ""
    def init(self):
        return ida_idaapi.PLUGIN_KEEP
    def run(self, arg):
        from PyQt5 import QtWidgets
        QtWidgets.QMessageBox().exec_()

def PLUGIN_ENTRY():
    return izs_gui_t()
"#;

/// Plugins: the list, running a harmless test plugin that lives only in a
/// private IDAUSR next to the test copy (never the user's), and a plugin that
/// needs the Qt UI. The user's own plugins are listed, never run.
fn plugins(binary: &std::path::Path, out: &mut String, check: &mut impl FnMut(bool, &str)) {
    let dir = binary.parent().unwrap().join("plugins_run");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("usr").join("plugins")).unwrap();
    std::fs::write(
        dir.join("usr").join("plugins").join("izs_marker.py"),
        MARKER_PLUGIN,
    )
    .unwrap();
    std::fs::write(
        dir.join("usr").join("plugins").join("izs_gui.py"),
        GUI_PLUGIN,
    )
    .unwrap();
    let input = dir.join(binary.file_name().unwrap());
    std::fs::copy(binary, &input).unwrap();
    let cfg = HostConfig::discover().with_idausr(&dir.join("usr"));
    let (c, mut plugin_logs) = Client::spawn(&cfg).expect("spawn plugin host");
    c.call_blocking(Open {
        path: input.display().to_string(),
        auto_analyze: true,
        base: None,
    })
    .expect("open for plugins");
    writeln!(
        out,
        "
== plugins"
    )
    .unwrap();
    let list = c.call_blocking(PluginList {}).unwrap();
    for p in &list {
        writeln!(
            out,
            "{} | id={} | {} | hotkey={:?} code={} | flags={:#x} {:?} | loaded={}",
            p.name,
            p.id,
            p.path.rsplit(['\\', '/']).next().unwrap_or(""),
            p.hotkey,
            p.hotkey_code,
            p.flags,
            plugin_flags::names(p.flags),
            p.loaded
        )
        .unwrap();
    }
    let marker = list.iter().find(|p| p.path.ends_with("izs_marker.py"));
    let gui = list.iter().find(|p| p.path.ends_with("izs_gui.py"));
    check(
        marker.is_some() && gui.is_some(),
        "plugin_list finds the plugins of a private IDAUSR",
    );
    check(
        list.iter().all(|p| !p.path.is_empty()),
        "every listed plugin has a path",
    );
    if let Some(m) = marker {
        let r = c
            .call_blocking(PluginRun {
                plugin: m.path.clone(),
                arg: 7,
            })
            .unwrap();
        writeln!(
            out,
            "run marker: failed={} changed={} needs_gui={} log={:?}",
            r.failed, r.changed, r.needs_gui, r.log
        )
        .unwrap();
        check(r.loadable && !r.failed, "plugin_run runs the test plugin");
        check(
            r.log
                .iter()
                .any(|l| l.contains("izs-marker plugin ran, arg=7")),
            "plugin_run captures the plugin's log with its argument",
        );
        check(
            !r.needs_gui,
            "a plugin without UI is not flagged as needing it",
        );
        let info = c.call_blocking(Info {}).unwrap();
        let cmt = c
            .call_blocking(GetComment {
                ea: info.min_ea,
                repeatable: false,
            })
            .unwrap();
        writeln!(out, "comment at min_ea: {cmt:?}").unwrap();
        check(
            cmt == "izs-plugin-marker",
            "the plugin's marker comment is in the database",
        );
    }
    if let Some(g) = gui {
        let r = c
            .call_blocking(PluginRun {
                plugin: g.path.clone(),
                arg: 0,
            })
            .unwrap();
        writeln!(
            out,
            "run gui: failed={} changed={} needs_gui={} log={:?}",
            r.failed, r.changed, r.needs_gui, r.log
        )
        .unwrap();
        check(
            r.needs_gui && r.failed && !r.changed,
            "a plugin that needs Qt is flagged as failed, needing the UI, and changes nothing",
        );
    }
    check(
        c.call_blocking(PluginRun {
            plugin: "no/such/plugin".into(),
            arg: 0,
        })
        .is_err(),
        "plugin_run rejects an unknown plugin",
    );
    let _ = c.call_blocking(Close { save: false });
    while let Ok(l) = plugin_logs.try_recv() {
        if l.to_lowercase().contains("izs") {
            eprintln!("plugin host: {l}");
        }
    }
}

/// FLIRT lists and apply, the Lumina refusal path (and a pull on the public
/// sample only) and the proximity neighbourhood. A Lumina push is never sent:
/// only the refusal of an unconfirmed one is exercised.
fn signatures_lumina_proximity(
    c: &Client,
    binary: &std::path::Path,
    out: &mut String,
    check: &mut impl FnMut(bool, &str),
) {
    writeln!(
        out,
        "
== signatures"
    )
    .unwrap();
    let before = c.call_blocking(SigList {}).unwrap();
    writeln!(
        out,
        "processor={} lib_functions={}",
        before.processor, before.lib_functions
    )
    .unwrap();
    for a in &before.applied {
        writeln!(
            out,
            "applied: {} | {} | {} | matched {}",
            a.name, a.title, a.state, a.matched
        )
        .unwrap();
    }
    writeln!(
        out,
        "available: {} files, first: {} ({})",
        before.available.len(),
        before.available[0].name,
        before.available[0].title
    )
    .unwrap();
    let library_rows = |c: &Client| {
        c.call_blocking(Functions {})
            .unwrap()
            .iter()
            .filter(|f| f.library)
            .count() as u64
    };
    check(
        !before.applied.is_empty()
            && before
                .applied
                .iter()
                .all(|a| !a.title.is_empty() && a.state == "applied"),
        "sig_list lists applied signatures with titles",
    );
    check(
        before.available.len() > 50
            && before
                .available
                .iter()
                .all(|a| !a.name.is_empty() && !a.title.is_empty()),
        "sig_list lists the .sig files with titles",
    );
    check(
        before.available.iter().filter(|a| a.applied).count() == before.applied.len(),
        "available signatures are marked applied",
    );
    check(
        before.lib_functions == library_rows(c),
        "the library function count matches the function list",
    );

    let pick = ["vc64rtf", "vc64ucrt", "vc32rtf", "vc32ucrt"]
        .into_iter()
        .find(|n| before.available.iter().any(|a| a.name == *n && !a.applied))
        .expect("a runtime signature that is not applied yet");
    let r = c
        .call_blocking(SigApply {
            names: vec![pick.into(), "no_such_signature".into()],
        })
        .unwrap();
    let applied = r.state.applied.iter().find(|a| a.name == pick);
    writeln!(
        out,
        "apply {pick}: planned={} failed={:?} lib {} -> {} matched={:?}",
        r.planned,
        r.failed,
        r.lib_before,
        r.lib_after,
        applied.map(|a| a.matched)
    )
    .unwrap();
    check(
        r.planned == 1 && r.failed == ["no_such_signature"],
        "sig_apply plans the signature and reports the unknown one",
    );
    check(
        applied.is_some_and(|a| a.state == "applied"),
        "the signature is applied after the analysis ran",
    );
    check(
        applied.is_some_and(|a| a.matched > 0) && r.lib_after > r.lib_before,
        "a matching signature adds library functions",
    );
    check(
        r.lib_after == library_rows(c) && r.state.lib_functions == r.lib_after,
        "library count after apply equals the library rows",
    );
    check(
        r.state
            .available
            .iter()
            .any(|a| a.name == pick && a.applied),
        "the applied file is marked in the available list",
    );
    check(
        c.call_blocking(SigApply { names: vec![] }).is_err(),
        "sig_apply needs a name",
    );

    writeln!(
        out,
        "
== lumina"
    )
    .unwrap();
    let st = c.call_blocking(LuminaStatus {}).unwrap();
    writeln!(
        out,
        "server={:?} auto_use={} functions={} push_blocked={}",
        st.server, st.auto_use, st.functions, st.push_blocked
    )
    .unwrap();
    let funcs = c.call_blocking(Functions {}).unwrap();
    let search = funcs
        .iter()
        .find(|f| f.name.starts_with("FIND::SearchFiles"));
    for action in [LuminaAction::PushAll, LuminaAction::PushFunc] {
        let r = c.call_blocking(Lumina {
            action,
            ea: search.map_or(0, |f| f.start),
            confirmed: false,
        });
        writeln!(out, "{action:?} without confirmation -> {r:?}").unwrap();
        check(
            matches!(&r, Err(ClientError::Ida(m)) if m.contains("refused")),
            &format!("{action:?} is refused without confirmation"),
        );
    }
    // A pull sends function hashes to the server: only on the public sample.
    if binary.file_name().is_some_and(|n| n == "find.exe") {
        let r = c
            .call_blocking(Lumina {
                action: LuminaAction::PullAll,
                ea: 0,
                confirmed: false,
            })
            .unwrap();
        writeln!(
            out,
            "pull all: action={} dispatched={} functions={} lumina {} -> {} log={:?}",
            r.action, r.dispatched, r.functions, r.lumina_before, r.lumina_after, r.log
        )
        .unwrap();
        check(
            r.action == "LuminaPullAllMds"
                && (!r.log.is_empty() || r.lumina_after > r.lumina_before),
            "a pull reports what IDA did",
        );
        let one = c
            .call_blocking(Lumina {
                action: LuminaAction::PullFunc,
                ea: search.unwrap().start,
                confirmed: false,
            })
            .unwrap();
        writeln!(
            out,
            "pull function: action={} dispatched={} lumina {} -> {} log={:?}",
            one.action, one.dispatched, one.lumina_before, one.lumina_after, one.log
        )
        .unwrap();
        check(
            one.action == "LuminaPullMd",
            "a single-function pull runs LuminaPullMd",
        );
        check(
            c.call_blocking(Lumina {
                action: LuminaAction::PullFunc,
                ea: 0,
                confirmed: false,
            })
            .is_err(),
            "a single-function pull needs a function",
        );
    }

    writeln!(
        out,
        "
== proximity"
    )
    .unwrap();
    let search = search.expect("FIND::SearchFiles");
    let g = c
        .call_blocking(Proximity {
            ea: search.start + 3,
            up: 2,
            down: 1,
            fanout: 20,
            max_nodes: 200,
            data: false,
        })
        .unwrap();
    let name = |ea: Ea| {
        g.nodes
            .iter()
            .find(|n| n.ea == ea)
            .map(|n| n.name.clone())
            .unwrap_or_default()
    };
    for n in &g.nodes {
        writeln!(
            out,
            "{:>3} {:<7} {} (callers {}, callees {})",
            n.depth, n.kind, n.name, n.callers, n.callees
        )
        .unwrap();
    }
    writeln!(out, "{} edges", g.edges.len()).unwrap();
    check(
        g.centre == search.start && g.nodes[0].ea == search.start && g.nodes[0].depth == 0,
        "the neighbourhood is centred on the containing function",
    );
    check(
        g.nodes.iter().any(|n| n.depth == -1 && n.name == "main"),
        "callers of FIND::SearchFiles include main",
    );
    check(
        g.nodes
            .iter()
            .any(|n| n.depth == 1 && n.name.starts_with("FIND::SearchStream")),
        "callees of FIND::SearchFiles include FIND::SearchStream",
    );
    check(
        g.nodes.iter().any(|n| n.depth == -2),
        "two caller levels reach main's caller",
    );
    check(
        g.nodes.iter().all(|n| (-2..=1).contains(&n.depth)),
        "depths stay within the requested levels",
    );
    let mut eas: Vec<Ea> = g.nodes.iter().map(|n| n.ea).collect();
    eas.sort_unstable();
    eas.dedup();
    check(eas.len() == g.nodes.len(), "every function appears once");
    check(
        g.edges
            .iter()
            .all(|e| eas.contains(&e.from) && eas.contains(&e.to)),
        "edges connect listed nodes",
    );
    check(
        g.edges
            .iter()
            .any(|e| name(e.from) == "main" && e.to == search.start),
        "main -> FIND::SearchFiles is an edge",
    );
    let small = c
        .call_blocking(Proximity {
            ea: search.start,
            up: 1,
            down: 1,
            fanout: 2,
            max_nodes: 200,
            data: false,
        })
        .unwrap();
    check(
        small.nodes.iter().filter(|n| n.depth == 1).count() <= 2 && small.nodes[0].callees > 2,
        "the fan-out limit cuts the callees but still counts them",
    );
    let capped = c
        .call_blocking(Proximity {
            ea: search.start,
            up: 2,
            down: 2,
            fanout: 20,
            max_nodes: 3,
            data: false,
        })
        .unwrap();
    check(
        capped.nodes.len() == 3 && capped.truncated,
        "the node limit truncates the neighbourhood",
    );
    check(
        c.call_blocking(Proximity {
            ea: 0,
            up: 1,
            down: 1,
            fanout: 5,
            max_nodes: 10,
            data: false,
        })
        .is_err(),
        "an unmapped address has no neighbourhood",
    );
}

fn member_names(c: &Client, ty: &str) -> Vec<String> {
    c.call_blocking(UdtInfo { name: ty.into() })
        .unwrap()
        .members
        .iter()
        .filter(|m| !m.gap)
        .map(|m| m.name.clone())
        .collect()
}

fn type_names(c: &Client) -> Vec<String> {
    c.call_blocking(TypeList {})
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect()
}

/// Local types, structures, unions, enums: edit through the protocol and read
/// everything back.
fn type_editing(c: &Client, out: &mut String, check: &mut impl FnMut(bool, &str)) {
    writeln!(out, "\n== types").unwrap();
    let list = c.call_blocking(TypeList {}).unwrap();
    let guid = list.iter().find(|t| t.name == "GUID");
    check(
        guid.is_some_and(|t| t.kind == "typedef" && t.members == 0),
        "type_list reports a typedef without members",
    );
    check(
        list.iter().any(|t| {
            t.name == "_GUID" && t.kind == "struct" && t.members == 4 && t.size == Some(16)
        }),
        "type_list reports kind, size and member count",
    );

    // A multi-line declaration creates a struct; the printed declaration is multi-line again.
    let decl = "struct E2eS {\n  int id;\n  char name[16];\n  void *next;\n};";
    let r = c.call_blocking(TypeApply { decl: decl.into() }).unwrap();
    check(
        r.applied && r.names == ["E2eS"],
        "type_apply creates a struct from a multi-line declaration",
    );
    let d = c
        .call_blocking(TypeDecl {
            name: "E2eS".into(),
        })
        .unwrap();
    writeln!(out, "E2eS declaration:\n{}", d.decl).unwrap();
    check(
        d.kind == "struct"
            && d.decl.lines().count() >= 6
            && d.decl.contains("int id;")
            && d.decl.contains("char name[16];"),
        "type_decl returns the multi-line declaration",
    );
    let info = c
        .call_blocking(UdtInfo {
            name: "E2eS".into(),
        })
        .unwrap();
    check(
        info.size == 32 && info.members.iter().filter(|m| !m.gap).count() == 3,
        "udt_info lists the members with the struct size",
    );
    check(
        info.members
            .iter()
            .any(|m| m.name == "name" && m.offset == 4 && m.size == 16 && m.ty.contains("char")),
        "udt_info gives offset, size and type",
    );

    // Applying the same text again changes nothing.
    let before = type_names(c);
    let again = c.call_blocking(TypeApply { decl: decl.into() }).unwrap();
    let info2 = c
        .call_blocking(UdtInfo {
            name: "E2eS".into(),
        })
        .unwrap();
    check(
        again.applied
            && type_names(c) == before
            && info2.members.len() == info.members.len()
            && info2.size == info.size,
        "applying an unchanged declaration is idempotent",
    );
    // Editing the body replaces the type in place.
    let edited =
        "struct E2eS {\n  int id;\n  char name[16];\n  void *next;\n  unsigned int flags;\n};";
    let r = c
        .call_blocking(TypeApply {
            decl: edited.into(),
        })
        .unwrap();
    check(
        r.applied
            && member_names(c, "E2eS").contains(&"flags".to_owned())
            && type_names(c).len() == before.len(),
        "an edited declaration replaces the type",
    );

    // Parser errors come back with their line.
    let bad = "struct E2eBad {\n  int a;\n  nosuch_t b;\n};";
    let r = c.call_blocking(TypeApply { decl: bad.into() }).unwrap();
    let errs = r.errors(bad);
    writeln!(
        out,
        "parse error for the bad declaration: {:?} -> {:?}",
        r.messages, errs
    )
    .unwrap();
    check(
        !r.applied && r.errors > 0,
        "a declaration with errors is not applied",
    );
    check(
        errs.first()
            .is_some_and(|e| e.line == 2 && e.message.contains("nosuch_t")),
        "the parser error is reported on line 3 (0-based 2)",
    );
    check(
        !type_names(c).contains(&"E2eBad".to_owned()),
        "a failed declaration stores nothing",
    );
    let mixed = "struct E2eOk {\n  int a;\n};\nstruct E2eBad2 {\n  int a\n};";
    let r = c.call_blocking(TypeApply { decl: mixed.into() }).unwrap();
    check(
        !r.applied && r.names == ["E2eOk"] && r.errors(mixed).iter().all(|e| e.line >= 3),
        "valid statements apply, the broken one is reported on its own lines",
    );

    // Errors are attributed per declaration (GAPS E1): each declaration alone,
    // each message inside its own declaration's lines.
    let several = "struct E1a {
  int a;
};
struct E1b {
  int b
};
struct E1c {
  nosuch2_t c;
};";
    let spans = declerr::split_declarations(several);
    let mut lines_by_decl = Vec::new();
    for span in &spans {
        let r = c
            .call_blocking(TypeApply {
                decl: several[span.start..span.end].into(),
            })
            .unwrap();
        lines_by_decl.push(
            declerr::locate_in(several, span, &r.messages)
                .into_iter()
                .map(|e| e.line)
                .collect::<Vec<_>>(),
        );
    }
    writeln!(out, "per-declaration error lines: {lines_by_decl:?}").unwrap();
    check(
        spans.len() == 3
            && lines_by_decl[0].is_empty()
            && !lines_by_decl[1].is_empty()
            && lines_by_decl[1].iter().all(|l| (3..=5).contains(l))
            && !lines_by_decl[2].is_empty()
            && lines_by_decl[2].iter().all(|l| (6..=8).contains(l)),
        "type-parser errors are attributed to the line range of their own declaration",
    );

    // Member editing.
    let add = c.call_blocking(UdtAdd {
        name: "E2eS".into(),
        member: "tail".into(),
        decl: "unsigned char [3]".into(),
        offset: None,
    });
    let m = c
        .call_blocking(UdtInfo {
            name: "E2eS".into(),
        })
        .unwrap();
    check(
        add.is_ok() && m.members.iter().any(|m| m.name == "tail" && m.size == 3),
        "udt_add appends an array member",
    );
    check(
        c.call_blocking(UdtAdd {
            name: "E2eS".into(),
            member: "clash".into(),
            decl: "int".into(),
            offset: Some(4),
        })
        .is_err(),
        "udt_add refuses an overlapping member",
    );
    check(
        c.call_blocking(UdtAdd {
            name: "E2eS".into(),
            member: "id".into(),
            decl: "int".into(),
            offset: Some(0x40),
        })
        .is_err(),
        "udt_add refuses a duplicate name",
    );
    let idx = |c: &Client, name: &str| {
        c.call_blocking(UdtInfo {
            name: "E2eS".into(),
        })
        .unwrap()
        .members
        .iter()
        .find(|m| m.name == name)
        .map(|m| m.index)
    };
    c.call_blocking(UdtRename {
        name: "E2eS".into(),
        index: idx(c, "tail").unwrap(),
        new: "pad".into(),
    })
    .unwrap();
    check(
        idx(c, "pad").is_some() && idx(c, "tail").is_none(),
        "udt_rename renames a member",
    );
    c.call_blocking(UdtRetype {
        name: "E2eS".into(),
        index: idx(c, "flags").unwrap(),
        decl: "unsigned __int16".into(),
        force: false,
    })
    .unwrap();
    let m = c
        .call_blocking(UdtInfo {
            name: "E2eS".into(),
        })
        .unwrap();
    check(
        m.members
            .iter()
            .any(|m| m.name == "flags" && m.size == 2 && m.ty.contains("__int16")),
        "udt_retype changes the member type",
    );
    c.call_blocking(UdtComment {
        name: "E2eS".into(),
        index: idx(c, "id").unwrap(),
        text: "the id".into(),
    })
    .unwrap();
    let d = c
        .call_blocking(TypeDecl {
            name: "E2eS".into(),
        })
        .unwrap();
    check(
        c.call_blocking(UdtInfo {
            name: "E2eS".into(),
        })
        .unwrap()
        .members
        .iter()
        .any(|m| m.name == "id" && m.cmt == "the id")
            && d.decl.contains("the id"),
        "udt_comment shows in the member and the declaration",
    );
    c.call_blocking(UdtDelete {
        name: "E2eS".into(),
        index: idx(c, "pad").unwrap(),
    })
    .unwrap();
    check(idx(c, "pad").is_none(), "udt_delete removes a member");
    // Undo covers the type library.
    let n = member_names(c, "E2eS").len();
    c.call_blocking(UdtAdd {
        name: "E2eS".into(),
        member: "undoable".into(),
        decl: "int".into(),
        offset: None,
    })
    .unwrap();
    check(
        member_names(c, "E2eS").len() == n + 1,
        "a member was added before undo",
    );
    check(
        c.call_blocking(Undo {}).unwrap() && member_names(c, "E2eS").len() == n,
        "undo reverts a type edit",
    );
    check(
        c.call_blocking(Redo {}).unwrap() && member_names(c, "E2eS").len() == n + 1,
        "redo restores it",
    );
    // Struct-member xrefs with a real struct offset (GAPS E3): apply
    // `E3S.f6` (offset 0x18) to an operand `[reg+18h]` of the sample, let the
    // analysis queue process it (IDA adds the member xref there, not inside
    // `op_stroff`, which only links the struct), then ask for the member's xrefs.
    let r = c
        .call_blocking(TypeApply {
            decl: "struct E3S { int p0; int p1; int p2; int p3; int p4; int p5; int f6; int f7; };"
                .into(),
        })
        .unwrap();
    check(r.applied, "the member-xrefs struct E3S is declared");
    let found = py(
        c,
        "import idautils, ida_ua
found = None
for i, f in enumerate(idautils.Functions()):
    if i > 80: break
    for ea in idautils.FuncItems(f):
        insn = ida_ua.insn_t()
        if ida_ua.decode_insn(insn, ea) == 0: continue
        for n in range(2):
            if insn.ops[n].type == ida_ua.o_displ and insn.ops[n].addr == 0x18:
                found = (ea, n); break
        if found: break
    if found: break
print(found[0] if found else 0, found[1] if found else 0)",
        0,
        true,
    );
    let (op_ea, op_n) = text_of(&found, "out")
        .split_once(' ')
        .map(|(a, b)| (a.trim().parse::<u64>().unwrap_or(0), b.trim().to_owned()))
        .unwrap_or((0, "0".into()));
    if op_ea == 0 {
        writeln!(
            out,
            "no [reg+18h] operand in this sample: struct-offset xrefs not exercised"
        )
        .unwrap();
    } else {
        let applied = py(
            c,
            &format!(
                "import idc, ida_typeinf
tid = ida_typeinf.get_named_type_tid('E3S')
print(idc.op_stroff({op_ea}, {op_n}, tid, 0))
import ida_auto
ida_auto.plan_ea({op_ea})
ida_auto.auto_wait()"
            ),
            0,
            true,
        );
        writeln!(
            out,
            "op_stroff on operand {op_n}: {}",
            text_of(&applied, "out").trim()
        )
        .unwrap();
        let xr = c
            .call_blocking(UdtXrefs {
                name: "E3S".into(),
                index: 6,
            })
            .unwrap();
        writeln!(out, "E3S.f6 xrefs: {}", xr.len()).unwrap();
        check(
            xr.iter().any(|x| x.from == op_ea),
            "udt_xrefs lists the operand a struct offset was applied to",
        );
        let other = c
            .call_blocking(UdtXrefs {
                name: "E3S".into(),
                index: 0,
            })
            .unwrap();
        check(
            other.iter().all(|x| x.from != op_ea),
            "udt_xrefs of another member does not list it",
        );
    }

    // Creating, unions and dependents.
    check(
        c.call_blocking(TypeCreate {
            name: "E2eNew".into(),
            kind: "struct".into(),
        })
        .is_ok()
            && c.call_blocking(UdtInfo {
                name: "E2eNew".into(),
            })
            .unwrap()
            .members
            .is_empty(),
        "type_create makes an empty struct",
    );
    check(
        c.call_blocking(TypeCreate {
            name: "E2eNew".into(),
            kind: "struct".into(),
        })
        .is_err(),
        "type_create refuses an existing name",
    );
    c.call_blocking(TypeCreate {
        name: "E2eU".into(),
        kind: "union".into(),
    })
    .unwrap();
    c.call_blocking(UdtAdd {
        name: "E2eU".into(),
        member: "i".into(),
        decl: "int".into(),
        offset: None,
    })
    .unwrap();
    c.call_blocking(UdtAdd {
        name: "E2eU".into(),
        member: "q".into(),
        decl: "__int64".into(),
        offset: None,
    })
    .unwrap();
    let u = c
        .call_blocking(UdtInfo {
            name: "E2eU".into(),
        })
        .unwrap();
    check(
        u.is_union && u.size == 8 && u.members.iter().all(|m| m.offset == 0),
        "a union's members share offset 0 and its size is the largest",
    );
    let holder = "struct E2eHolder {\n  E2eS s;\n  E2eS *p;\n};\ntypedef E2eS E2eAlias;";
    check(
        c.call_blocking(TypeApply {
            decl: holder.into(),
        })
        .unwrap()
        .applied,
        "a struct can contain another",
    );
    let del = c
        .call_blocking(TypeDelete {
            name: "E2eS".into(),
            force: false,
        })
        .unwrap();
    writeln!(out, "dependents of E2eS: {:?}", del.dependents).unwrap();
    check(
        !del.deleted
            && del.dependents.contains(&"E2eHolder".to_owned())
            && del.dependents.contains(&"E2eAlias".to_owned()),
        "type_delete reports dependents and refuses",
    );
    check(
        type_names(c).contains(&"E2eS".to_owned()),
        "the blocked type still exists",
    );
    let del = c
        .call_blocking(TypeDelete {
            name: "E2eS".into(),
            force: true,
        })
        .unwrap();
    check(
        del.deleted && !type_names(c).contains(&"E2eS".to_owned()),
        "a forced delete removes the type",
    );
    check(
        c.call_blocking(TypeDelete {
            name: "E2eU".into(),
            force: false,
        })
        .unwrap()
        .deleted,
        "an unreferenced type deletes",
    );

    // Enumerations.
    let r = c
        .call_blocking(TypeApply {
            decl: "enum E2eE {\n  A = 1,\n  B = 2\n};".into(),
        })
        .unwrap();
    check(r.applied, "type_apply creates an enum");
    let e = |c: &Client| {
        c.call_blocking(EnumInfo {
            name: "E2eE".into(),
        })
        .unwrap()
    };
    let kinds = c.call_blocking(TypeList {}).unwrap();
    check(
        kinds
            .iter()
            .any(|t| t.name == "E2eE" && t.kind == "enum" && t.members == 2),
        "type_list reports an enum with its member count",
    );
    c.call_blocking(EnumAdd {
        name: "E2eE".into(),
        member: "C".into(),
        value: 4,
    })
    .unwrap();
    c.call_blocking(EnumRename {
        name: "E2eE".into(),
        index: 0,
        new: "AA".into(),
    })
    .unwrap();
    c.call_blocking(EnumSetValue {
        name: "E2eE".into(),
        index: 1,
        value: 0x20,
    })
    .unwrap();
    c.call_blocking(EnumComment {
        name: "E2eE".into(),
        index: 1,
        text: "bee".into(),
    })
    .unwrap();
    let info = e(c);
    writeln!(
        out,
        "enum after edits: {:?}",
        info.members
            .iter()
            .map(|m| (m.name.as_str(), m.value))
            .collect::<Vec<_>>()
    )
    .unwrap();
    check(
        info.members.len() == 3
            && info.members[0].name == "AA"
            && info.members[1].value == 0x20
            && info.members[1].cmt == "bee"
            && info.members[2].name == "C"
            && info.members[2].value == 4,
        "enum add, rename, value and comment edits read back",
    );
    c.call_blocking(EnumSetBitfield {
        name: "E2eE".into(),
        on: true,
    })
    .unwrap();
    check(
        e(c).bitfield
            && c.call_blocking(TypeDecl {
                name: "E2eE".into(),
            })
            .unwrap()
            .decl
            .contains("bitmask"),
        "enum_set_bitfield switches to a flag enum",
    );
    c.call_blocking(EnumDelete {
        name: "E2eE".into(),
        index: 0,
    })
    .unwrap();
    check(e(c).members.len() == 2, "enum_delete removes a member");
    check(
        c.call_blocking(EnumAdd {
            name: "E2eE".into(),
            member: "B".into(),
            value: 9,
        })
        .is_err(),
        "enum_add refuses a duplicate name",
    );
    check(
        c.call_blocking(EnumInfo {
            name: "E2eNew".into(),
        })
        .is_err(),
        "enum_info refuses a struct",
    );
    check(
        c.call_blocking(UdtInfo {
            name: "E2eE".into(),
        })
        .is_err(),
        "udt_info refuses an enum",
    );
}

/// Stack frames: read, cross-check against the listing, edit, and see the
/// edits in the listing and the pseudocode.
fn frame_editing(
    c: &Client,
    funcs: &[FuncRow],
    skip: Ea,
    hexrays: bool,
    out: &mut String,
    check: &mut impl FnMut(bool, &str),
) {
    writeln!(out, "\n== frames").unwrap();
    let listing = |c: &Client, f: Ea| -> String {
        let l = c.call_blocking(ListingDown { ea: f, count: 400 }).unwrap();
        l.lines
            .iter()
            .map(|l| tags::strip(&l.tagged))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let pseudo = |c: &Client, f: Ea| -> String {
        let p = c.call_blocking(Decompile { ea: f }).unwrap();
        p.lines
            .iter()
            .map(|l| tags::strip(l))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut candidates: Vec<&FuncRow> = funcs
        .iter()
        .filter(|f| !f.library && !f.thunk && f.start != skip)
        .collect();
    candidates.sort_by_key(|f| std::cmp::Reverse(f.end - f.start));
    let mut chosen = None;
    for f in candidates.into_iter().take(12) {
        let Ok(fr) = c.call_blocking(FrameInfo { func: f.start }) else {
            continue;
        };
        if !hexrays {
            if let Some(m) = fr
                .members
                .iter()
                .find(|m| m.kind == "local" && m.name.starts_with("var_") && m.size > 0)
            {
                chosen = Some((f.start, fr.clone(), m.offset));
                break;
            }
            continue;
        }
        // A variable shows up in pseudocode under its frame name if the decompiler uses it.
        for m in fr
            .members
            .iter()
            .filter(|m| m.kind == "local" && m.name.starts_with("var_"))
        {
            c.call_blocking(FrameRename {
                func: f.start,
                offset: m.offset,
                new: "e2e_probe".into(),
            })
            .unwrap();
            let seen = pseudo(c, f.start).contains("e2e_probe");
            assert!(c.call_blocking(Undo {}).unwrap());
            if seen {
                chosen = Some((f.start, fr.clone(), m.offset));
                break;
            }
        }
        if chosen.is_some() {
            break;
        }
    }
    let Some((func, fr, off)) = chosen else {
        check(
            false,
            "a function with a decompiler-visible stack variable exists in the sample",
        );
        return;
    };
    writeln!(
        out,
        "frame of {} ({func:#x}): size={} locals={} saved={} ret={} args={} fpd={}",
        fr.name, fr.size, fr.locals_size, fr.saved_regs_size, fr.ret_size, fr.args_size, fr.fpd
    )
    .unwrap();
    for m in &fr.members {
        let fp = format!(
            "{}{:#x}",
            if m.fp_off < 0 { '-' } else { '+' },
            m.fp_off.abs()
        );
        writeln!(
            out,
            "  +{:#05x} fp={:<6} size={:<3} {:<14} {:<18} {}",
            m.offset, fp, m.size, m.kind, m.name, m.ty
        )
        .unwrap();
    }
    check(
        fr.lvars.end - fr.lvars.start == fr.locals_size
            && fr.saved_regs.end - fr.saved_regs.start == fr.saved_regs_size
            && fr.return_address.end - fr.return_address.start == fr.ret_size
            && fr.args.end - fr.args.start == fr.args_size
            && fr.locals_size + fr.saved_regs_size + fr.ret_size + fr.args_size == fr.size,
        "frame regions add up to the frame size",
    );
    check(
        fr.members.windows(2).all(|w| w[0].offset < w[1].offset),
        "frame members come in offset order",
    );
    check(
        fr.members.iter().any(|m| m.kind == "return_address")
            && fr
                .members
                .iter()
                .any(|m| m.kind == "arg" || m.kind == "local"),
        "the return address and variables are classified",
    );
    // IDA names automatic variables after their frame-pointer offset: var_50 is at -0x50.
    let auto: Vec<_> = fr
        .members
        .iter()
        .filter(|m| m.kind == "local" && m.name.starts_with("var_") && !m.name.starts_with("var_s"))
        .collect();
    check(
        !auto.is_empty()
            && auto
                .iter()
                .all(|m| m.name == format!("var_{:X}", -m.fp_off)),
        "fp-relative offsets agree with IDA's var_XX names",
    );

    let before = listing(c, func);
    let old = fr.members.iter().find(|m| m.offset == off).unwrap().clone();
    check(
        before.contains(&old.name),
        "the listing shows the automatic variable name",
    );
    c.call_blocking(FrameRename {
        func,
        offset: off,
        new: "e2e_stkvar".into(),
    })
    .unwrap();
    let after = listing(c, func);
    check(
        after.contains("e2e_stkvar") && !after.contains(&format!("+{}]", old.name)),
        "a renamed frame variable shows in the listing",
    );
    if hexrays {
        check(
            pseudo(c, func).contains("e2e_stkvar"),
            "a renamed frame variable shows in the pseudocode",
        );
    }
    let xr = c.call_blocking(FrameXrefs { func, offset: off }).unwrap();
    writeln!(out, "xrefs to e2e_stkvar: {}", xr.len()).unwrap();
    check(
        !xr.is_empty() && xr.iter().all(|x| x.text.contains("e2e_stkvar")),
        "frame_xrefs lists the instructions that use the variable",
    );
    c.call_blocking(FrameRetype {
        func,
        offset: off,
        decl: "char [8]".into(),
        force: true,
    })
    .unwrap();
    let fr2 = c.call_blocking(FrameInfo { func }).unwrap();
    check(
        fr2.members.iter().any(|m| {
            m.offset == off && m.name == "e2e_stkvar" && m.ty.contains("char") && m.size == 8
        }),
        "frame_retype changes the type",
    );
    if hexrays {
        let p = pseudo(c, func);
        check(
            p.lines()
                .any(|l| l.trim_start().starts_with("char e2e_stkvar[8];")),
            "a retyped frame variable shows in the pseudocode",
        );
    }
    c.call_blocking(FrameComment {
        func,
        offset: off,
        text: "from e2e".into(),
    })
    .unwrap();
    check(
        c.call_blocking(FrameInfo { func })
            .unwrap()
            .members
            .iter()
            .any(|m| m.offset == off && m.cmt == "from e2e"),
        "frame_comment reads back",
    );
    check(
        c.call_blocking(Undo {}).unwrap()
            && c.call_blocking(FrameInfo { func })
                .unwrap()
                .members
                .iter()
                .all(|m| m.offset != off || m.cmt.is_empty()),
        "undo reverts a frame edit",
    );
    // Define a variable in a gap between members, then delete it.
    let gap = fr2
        .members
        .windows(2)
        .find(|w| {
            w[0].kind == "local"
                && w[1].kind == "local"
                && w[1].offset >= w[0].offset + w[0].size + 4
        })
        .map(|w| w[0].offset + w[0].size);
    if let Some(at) = gap {
        c.call_blocking(FrameDefine {
            func,
            offset: at,
            name: "e2e_defined".into(),
            decl: "int".into(),
        })
        .unwrap();
        let f3 = c.call_blocking(FrameInfo { func }).unwrap();
        check(
            f3.members
                .iter()
                .any(|m| m.offset == at && m.name == "e2e_defined" && m.size == 4),
            "frame_define adds a variable at an offset",
        );
        c.call_blocking(FrameDelete { func, offset: at }).unwrap();
        check(
            c.call_blocking(FrameInfo { func })
                .unwrap()
                .members
                .iter()
                .all(|m| m.name != "e2e_defined"),
            "frame_delete removes it",
        );
    } else {
        check(false, "the chosen frame has a gap to define a variable in");
    }
    check(
        c.call_blocking(FrameDefine {
            func,
            offset: off + 1,
            name: "e2e_overlap".into(),
            decl: "int".into(),
        })
        .is_err(),
        "frame_define refuses an overlapping variable",
    );
    check(
        c.call_blocking(FrameInfo { func: 0 }).is_err(),
        "frame_info fails outside functions",
    );
}

type Events = futures::channel::mpsc::UnboundedReceiver<HostEvent>;

/// Everything the host has sent so far. Events are queued before the reply of
/// the request that caused them, so after `call_blocking` returns they are
/// all here: no sleeping.
fn drain(events: &mut Events) -> Vec<HostEvent> {
    let mut all = Vec::new();
    while let Ok(e) = events.try_recv() {
        all.push(e);
    }
    all
}

fn changes(events: &[HostEvent]) -> Vec<&DbChanged> {
    events
        .iter()
        .filter_map(|e| {
            if let HostEvent::DbChanged(d) = e {
                Some(d)
            } else {
                None
            }
        })
        .collect()
}

fn py(c: &Client, source: &str, ea: Ea, force: bool) -> PyReply {
    c.call_blocking(PyEval {
        source: source.into(),
        ea,
        force,
    })
    .unwrap()
}

/// The listing columns the kernel holds, as `indent,cmt_indent`.
fn inf_columns(c: &Client) -> String {
    let r = py(
        c,
        "import ida_ida; print(ida_ida.inf_get_indent(), ida_ida.inf_get_cmt_indent(), sep=',')",
        0,
        true,
    );
    text_of(&r, "out").trim().to_owned()
}

fn text_of(r: &PyReply, stream: &str) -> String {
    r.chunks
        .iter()
        .filter(|c| c.0 == stream)
        .map(|c| c.1.as_str())
        .collect()
}

// ---- Edit menu: operand representation, arrays, strings, function
// properties, patching and marks -------------------------------------------

/// The text of the first listing line of the item at `ea` (no comments,
/// no labels on a line of their own).
fn item_text(c: &Client, ea: Ea) -> String {
    let l = c.call_blocking(ListingDown { ea, count: 1 }).unwrap();
    l.lines
        .iter()
        .filter(|l| l.ea == ea)
        .map(|l| tags::strip(&l.tagged))
        .find(|t| {
            let code = t.split(';').next().unwrap_or("").trim();
            !code.is_empty() && !code.ends_with(':') && !code.contains(" proc ")
        })
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// Every listing line of the item at `ea`, comments and all.
fn item_block(c: &Client, ea: Ea) -> String {
    let l = c.call_blocking(ListingDown { ea, count: 1 }).unwrap();
    l.lines
        .iter()
        .map(|l| tags::strip(&l.tagged))
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

/// Mnemonic and comma-separated operands of an instruction line.
fn split_insn(text: &str) -> (String, Vec<String>) {
    let code = text.split(';').next().unwrap_or("").trim();
    let (m, rest) = code.split_once(char::is_whitespace).unwrap_or((code, ""));
    let ops = if rest.trim().is_empty() {
        Vec::new()
    } else {
        rest.split(',').map(|o| o.trim().to_owned()).collect()
    };
    (m.to_owned(), ops)
}

fn operand_text(c: &Client, ea: Ea, op: usize) -> String {
    split_insn(&item_text(c, ea))
        .1
        .get(op)
        .cloned()
        .unwrap_or_default()
}

fn repr_row(c: &Client, ea: Ea, op: u32) -> Option<OpReprRow> {
    c.call_blocking(GetOpRepr { ea })
        .unwrap()
        .into_iter()
        .find(|r| r.op == op)
}

fn repr_kind(c: &Client, ea: Ea, op: u32) -> Option<OpReprKind> {
    repr_row(c, ea, op).map(|r| r.kind)
}

fn set_repr(c: &Client, ea: Ea, op: Option<u32>, repr: OpRepr) -> Result<(), ClientError> {
    c.call_blocking(SetOpRepr { ea, op, repr })
}

/// `mov eax, 5A4Dh`: an instruction whose second operand is a plain hex number.
fn find_immediate(c: &Client, funcs: &[FuncRow]) -> Option<(Ea, u64)> {
    for f in funcs.iter().filter(|f| !f.library && !f.thunk).take(40) {
        let l = c
            .call_blocking(ListingDown {
                ea: f.start,
                count: 300,
            })
            .unwrap();
        for line in &l.lines {
            let (m, ops) = split_insn(&tags::strip(&line.tagged));
            if m != "mov" || ops.len() != 2 {
                continue;
            }
            let t = &ops[1];
            let Some(digits) = t.strip_suffix('h') else {
                continue;
            };
            let Ok(v) = u64::from_str_radix(digits, 16) else {
                continue;
            };
            if (0x20..=0xFFFF).contains(&v) && t.starts_with(|ch: char| ch.is_ascii_digit()) {
                return Some((line.ea, v));
            }
        }
    }
    None
}

/// An instruction with a stack variable operand (`[rsp+28h+var_10]`).
fn find_stack_operand(c: &Client, funcs: &[FuncRow]) -> Option<(Ea, u32)> {
    for f in funcs.iter().filter(|f| !f.library && !f.thunk).take(40) {
        let l = c
            .call_blocking(ListingDown {
                ea: f.start,
                count: 300,
            })
            .unwrap();
        for line in &l.lines {
            let t = tags::strip(&line.tagged);
            if !(t.contains("var_") || t.contains("arg_")) || !t.contains('[') {
                continue;
            }
            if let Some(r) = c
                .call_blocking(GetOpRepr { ea: line.ea })
                .unwrap()
                .into_iter()
                .find(|r| r.kind == OpReprKind::StackVar)
            {
                return Some((line.ea, r.op));
            }
        }
    }
    None
}

fn operand_representation(
    c: &Client,
    funcs: &[FuncRow],
    events: &mut Events,
    out: &mut String,
    check: &mut impl FnMut(bool, &str),
) {
    writeln!(out, "\n== operand representation").unwrap();
    let Some((ea, v)) = find_immediate(c, funcs) else {
        check(false, "an instruction with a hex immediate exists");
        return;
    };
    let before = item_text(c, ea);
    writeln!(out, "{ea:#x}: {before}   (immediate {v:#x})").unwrap();
    let show = |what: &str, out: &mut String| {
        let t = operand_text(c, ea, 1);
        writeln!(out, "  {what:<16} -> {t}").unwrap();
        t
    };
    drain(events);

    set_repr(c, ea, Some(1), OpRepr::Decimal).unwrap();
    let dec = show("decimal", out);
    check(
        dec == v.to_string(),
        "set_op_repr decimal shows the number in decimal",
    );
    check(
        repr_kind(c, ea, 1) == Some(OpReprKind::Decimal),
        "get_op_repr reports decimal after set decimal",
    );
    let ev = drain(events);
    check(
        changes(&ev)
            .iter()
            .any(|d| d.kinds.iter().any(|k| k == "type") && d.lo.is_some_and(|lo| lo <= ea)),
        "an operand change emits a db_changed event of kind type at the address",
    );
    set_repr(c, ea, Some(1), OpRepr::Hex).unwrap();
    let hex = show("hex", out);
    check(
        hex.ends_with('h') && u64::from_str_radix(hex.trim_end_matches('h'), 16) == Ok(v),
        "set_op_repr hex shows a hex number",
    );
    check(
        repr_kind(c, ea, 1) == Some(OpReprKind::Hex),
        "get_op_repr reports hex after set hex",
    );
    set_repr(c, ea, Some(1), OpRepr::Binary).unwrap();
    let bin = show("binary", out);
    check(
        bin.ends_with('b')
            && u64::from_str_radix(&bin.trim_end_matches('b').replace(' ', ""), 2) == Ok(v),
        "set_op_repr binary shows the number in binary",
    );
    set_repr(c, ea, Some(1), OpRepr::Octal).unwrap();
    let oct = show("octal", out);
    check(
        oct.ends_with('o') && u64::from_str_radix(oct.trim_end_matches('o'), 8) == Ok(v),
        "set_op_repr octal shows the number in octal",
    );
    set_repr(c, ea, Some(1), OpRepr::Char).unwrap();
    show("char", out);
    check(
        repr_kind(c, ea, 1) == Some(OpReprKind::Char),
        "get_op_repr reports char after set char",
    );
    set_repr(c, ea, Some(1), OpRepr::Number).unwrap();
    check(
        repr_kind(c, ea, 1) == Some(OpReprKind::Hex),
        "set_op_repr number uses the processor's default radix",
    );

    // Negation and inversion are set, not toggled: applying twice is a no-op.
    set_repr(c, ea, Some(1), OpRepr::Decimal).unwrap();
    for (name, on, off) in [
        (
            "negate",
            OpRepr::Negate { on: true },
            OpRepr::Negate { on: false },
        ),
        (
            "invert",
            OpRepr::Invert { on: true },
            OpRepr::Invert { on: false },
        ),
    ] {
        set_repr(c, ea, Some(1), on.clone()).unwrap();
        set_repr(c, ea, Some(1), on).unwrap();
        let t = show(name, out);
        let row = repr_row(c, ea, 1).unwrap();
        let flag = if name == "negate" {
            row.negated
        } else {
            row.inverted
        };
        check(
            t != dec && flag,
            &format!("set_op_repr {name} changes the operand text and get_op_repr reports it"),
        );
        set_repr(c, ea, Some(1), off).unwrap();
        check(
            operand_text(c, ea, 1) == dec,
            &format!("set_op_repr {name} off restores the text"),
        );
    }

    set_repr(c, ea, Some(1), OpRepr::Offset { base: None }).unwrap();
    show("offset (base 0)", out);
    check(
        repr_kind(c, ea, 1) == Some(OpReprKind::Offset),
        "set_op_repr offset makes get_op_repr report an offset",
    );
    // A base that makes the target a function start, which certainly exists.
    let base = funcs[0].start - v;
    set_repr(c, ea, Some(1), OpRepr::Offset { base: Some(base) }).unwrap();
    show("offset (based)", out);
    let row = repr_row(c, ea, 1).unwrap();
    check(
        row.kind == OpReprKind::Offset && row.detail == format!("{base:X}"),
        "set_op_repr offset from a base makes get_op_repr report the offset and its base",
    );
    set_repr(c, ea, Some(1), OpRepr::OffsetSegment).unwrap();
    show("offset (segment)", out);
    check(
        repr_kind(c, ea, 1) == Some(OpReprKind::Offset),
        "set_op_repr offset-from-segment sets an offset",
    );

    // Enum and struct offset by name: types whose member sits at the immediate.
    let applied = c
        .call_blocking(TypeApply {
            decl: format!(
                "enum E2eOpsEnum {{ E2E_OPS_MEMBER = {v:#x} }};\nstruct E2eOpsStruct {{ char pad[{v}]; char marker; }};"
            ),
        })
        .unwrap();
    check(
        applied.applied,
        "the operand-representation types are declared",
    );
    set_repr(
        c,
        ea,
        Some(1),
        OpRepr::Enum {
            name: "E2eOpsEnum".into(),
        },
    )
    .unwrap();
    let en = show("enum", out);
    check(
        en == "E2E_OPS_MEMBER" && repr_kind(c, ea, 1) == Some(OpReprKind::Enum),
        "set_op_repr enum shows the enum member's name",
    );
    check(
        repr_row(c, ea, 1).unwrap().detail == "E2eOpsEnum",
        "get_op_repr names the enum",
    );
    set_repr(
        c,
        ea,
        Some(1),
        OpRepr::StructOffset {
            path: vec!["E2eOpsStruct".into()],
            delta: 0,
        },
    )
    .unwrap();
    let so = show("struct offset", out);
    check(
        so.contains("marker") && repr_kind(c, ea, 1) == Some(OpReprKind::StructOffset),
        "set_op_repr struct offset shows the member at that offset",
    );
    check(
        repr_row(c, ea, 1).unwrap().detail == "E2eOpsStruct",
        "get_op_repr names the struct path",
    );
    check(
        set_repr(
            c,
            ea,
            Some(1),
            OpRepr::Enum {
                name: "NoSuchEnum".into(),
            },
        )
        .is_err()
            && set_repr(
                c,
                ea,
                Some(1),
                OpRepr::StructOffset {
                    path: vec!["NoSuchStruct".into()],
                    delta: 0,
                },
            )
            .is_err()
            && set_repr(c, ea, Some(9), OpRepr::Hex).is_err(),
        "set_op_repr rejects unknown types and operand numbers",
    );

    // All operands at once, then back to the default.
    set_repr(c, ea, None, OpRepr::Hex).unwrap();
    check(
        repr_kind(c, ea, 1) == Some(OpReprKind::Hex),
        "set_op_repr on all operands reaches operand 1",
    );
    set_repr(c, ea, None, OpRepr::Clear).unwrap();
    check(
        item_text(c, ea) == before && repr_kind(c, ea, 1) == Some(OpReprKind::Default),
        "set_op_repr clear restores the default text",
    );
    for t in ["E2eOpsEnum", "E2eOpsStruct"] {
        let _ = c.call_blocking(TypeDelete {
            name: t.into(),
            force: true,
        });
    }

    // Undo reverts an operand change.
    set_repr(c, ea, Some(1), OpRepr::Decimal).unwrap();
    check(
        operand_text(c, ea, 1) == v.to_string(),
        "decimal is shown before the undo",
    );
    let undone = c.call_blocking(Undo {}).unwrap();
    let after = item_text(c, ea);
    writeln!(out, "  undo -> {after}").unwrap();
    check(undone && after == before, "undo reverts an operand change");

    // Stack variable (K): off by hex, back on by stack_var.
    if let Some((sea, sop)) = find_stack_operand(c, funcs) {
        let s0 = item_text(c, sea);
        set_repr(c, sea, Some(sop), OpRepr::Hex).unwrap();
        let s1 = item_text(c, sea);
        let off = repr_kind(c, sea, sop) != Some(OpReprKind::StackVar) && s1 != s0;
        set_repr(c, sea, Some(sop), OpRepr::StackVar).unwrap();
        let s2 = item_text(c, sea);
        writeln!(
            out,
            "stack operand {sea:#x}: {s0} | hex: {s1} | stack_var: {s2}"
        )
        .unwrap();
        check(
            off && s2 == s0 && repr_kind(c, sea, sop) == Some(OpReprKind::StackVar),
            "set_op_repr stack_var turns a numeric operand back into the stack variable",
        );
    } else {
        check(false, "an instruction with a stack variable exists");
    }
}

fn arrays_and_strings(
    c: &Client,
    strings: &[StrRow],
    target: Ea,
    out: &mut String,
    check: &mut impl FnMut(bool, &str),
) {
    writeln!(out, "\n== arrays and string types").unwrap();
    let Some(s) = strings
        .iter()
        .find(|s| s.kind == "C" && s.len >= 16 && s.text.is_ascii())
    else {
        check(false, "a long C string exists to turn into an array");
        return;
    };
    let ea = s.ea;
    let original = item_block(c, ea);
    let original_bytes = c.call_blocking(Bytes { ea, len: 16 }).unwrap().hex;
    let next_of = |c: &Client| c.call_blocking(ListingDown { ea, count: 1 }).unwrap().next;
    let original_next = next_of(c);
    writeln!(out, "{ea:#x}: {original}").unwrap();

    let _ = c.call_blocking(Undefine { ea }); // fails when nothing is defined there
    let d = c.call_blocking(ArrayDefaults { ea }).unwrap();
    writeln!(out, "defaults on unexplored bytes: {d:?}").unwrap();
    check(
        d.elem == ArrayElem::Byte && !d.is_array && d.max_count >= 16 && d.count >= 1,
        "array_defaults offers bytes and the room that is left over unexplored data",
    );
    c.call_blocking(MakeArray {
        ea,
        elem: ArrayElem::Dword,
        count: 3,
    })
    .unwrap();
    let line = item_text(c, ea);
    writeln!(out, "dword[3]: {line}  next={:?}", next_of(c)).unwrap();
    check(
        next_of(c) == Some(ea + 12),
        "make_array dword x3 makes a 12-byte item",
    );
    check(
        line.contains("dd "),
        "make_array dword shows as dd in the listing",
    );
    let d = c.call_blocking(ArrayDefaults { ea }).unwrap();
    check(
        d.is_array && d.elem == ArrayElem::Dword && d.count == 3 && d.elem_size == 4,
        "array_defaults describes the array that is there",
    );
    c.call_blocking(MakeArray {
        ea,
        elem: ArrayElem::Byte,
        count: 5,
    })
    .unwrap();
    check(
        next_of(c) == Some(ea + 5),
        "make_array again replaces the array",
    );
    check(
        c.call_blocking(MakeArray {
            ea,
            elem: ArrayElem::Byte,
            count: 0,
        })
        .is_err()
            && c.call_blocking(MakeArray {
                ea,
                elem: ArrayElem::Struct {
                    name: "NoSuchStruct".into(),
                },
                count: 2,
            })
            .is_err(),
        "make_array rejects an empty array and an unknown struct",
    );

    // A pointer shown as an offset: `dq offset name`.
    let _ = c.call_blocking(Undefine { ea });
    c.call_blocking(PatchBytes {
        ea,
        hex: target
            .to_le_bytes()
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" "),
    })
    .unwrap();
    let _ = c.call_blocking(Undefine { ea });
    c.call_blocking(MakeArray {
        ea,
        elem: ArrayElem::Qword,
        count: 1,
    })
    .unwrap();
    set_repr(c, ea, Some(0), OpRepr::Hex).unwrap();
    let plain = item_text(c, ea);
    set_repr(c, ea, Some(0), OpRepr::Offset { base: None }).unwrap();
    let as_offset = item_text(c, ea);
    writeln!(out, "qword: {plain} | offset: {as_offset}").unwrap();
    check(
        !plain.contains("offset")
            && as_offset.contains("offset")
            && repr_kind(c, ea, 0) == Some(OpReprKind::Offset),
        "set_op_repr offset shows a pointer as `offset name`",
    );
    set_repr(c, ea, Some(0), OpRepr::Clear).unwrap();
    check(
        repr_kind(c, ea, 0) == Some(OpReprKind::Default),
        "set_op_repr clear takes the offset away again",
    );

    // String types: write each layout with patch_bytes, then define it.
    let cases: [(StringKind, &str, u64, &str); 6] = [
        (StringKind::C, "48 69 00", 3, "C"),
        (StringKind::C16, "48 00 69 00 00 00", 6, "C16"),
        (
            StringKind::C32,
            "48 00 00 00 69 00 00 00 00 00 00 00",
            12,
            "C32",
        ),
        (StringKind::Pascal, "02 48 69", 3, "Pascal"),
        (StringKind::Len2, "02 00 48 69", 4, "Len2"),
        (StringKind::Len4x16, "02 00 00 00 48 00 69 00", 8, "Len4x16"),
    ];
    for (kind, hex, size, name) in cases {
        let _ = c.call_blocking(Undefine { ea }); // fails when nothing is defined there
        c.call_blocking(PatchBytes {
            ea,
            hex: hex.into(),
        })
        .unwrap();
        let _ = c.call_blocking(Undefine { ea }); // fails when nothing is defined there
        c.call_blocking(MakeStringTyped { ea, kind, len: 0 })
            .unwrap();
        let t = item_text(c, ea);
        let kind_now = c.call_blocking(AddrInfo { ea }).unwrap().kind;
        writeln!(out, "{name}: {t}  next={:?} kind={kind_now}", next_of(c)).unwrap();
        check(
            kind_now == "string" && next_of(c) == Some(ea + size),
            &format!("make_string_typed {name} makes a {size}-byte string"),
        );
        check(
            item_block(c, ea).contains("'Hi'"),
            &format!("make_string_typed {name} shows the text in the listing"),
        );
    }
    // The old call keeps making a C string.
    let _ = c.call_blocking(Undefine { ea }); // fails when nothing is defined there
    c.call_blocking(PatchBytes {
        ea,
        hex: "48 69 00".into(),
    })
    .unwrap();
    let _ = c.call_blocking(Undefine { ea }); // fails when nothing is defined there
    c.call_blocking(MakeString { ea }).unwrap();
    check(
        next_of(c) == Some(ea + 3),
        "make_string still makes a C string",
    );
    check(
        c.call_blocking(MakeStringTyped {
            ea: 0,
            kind: StringKind::C,
            len: 0,
        })
        .is_err(),
        "make_string_typed fails where nothing can be defined",
    );

    // Put everything back.
    let reverted = c.call_blocking(RevertPatches { ea, len: 16 }).unwrap();
    let _ = c.call_blocking(Undefine { ea }); // fails when nothing is defined there
    c.call_blocking(MakeString { ea }).unwrap();
    let restored = item_block(c, ea);
    let restored_bytes = c.call_blocking(Bytes { ea, len: 16 }).unwrap().hex;
    writeln!(out, "restored ({reverted} bytes reverted): {restored}").unwrap();
    check(
        restored_bytes == original_bytes
            && restored.contains(&format!("'{}'", s.text))
            && next_of(c) == original_next,
        "the string and its bytes are back after the array and string-type checks",
    );
}

fn patching(
    c: &Client,
    funcs: &[FuncRow],
    binary: &std::path::Path,
    events: &mut Events,
    out: &mut String,
    check: &mut impl FnMut(bool, &str),
) {
    writeln!(out, "\n== patching").unwrap();
    // A multi-byte `mov` in the middle of a function.
    let mut pick = None;
    'f: for f in funcs.iter().filter(|f| !f.library && !f.thunk).take(40) {
        let l = c
            .call_blocking(ListingDown {
                ea: f.start,
                count: 120,
            })
            .unwrap();
        let eas: Vec<Ea> = l.lines.iter().map(|l| l.ea).collect();
        for w in eas.windows(2).skip(4) {
            let size = w[1].saturating_sub(w[0]);
            if (3..=7).contains(&size)
                && item_text(c, w[0]).starts_with("mov")
                && c.call_blocking(AddrInfo { ea: w[0] }).unwrap().kind == "code"
            {
                pick = Some((w[0], size));
                break 'f;
            }
        }
    }
    let Some((ea, size)) = pick else {
        check(false, "a multi-byte instruction exists to patch");
        return;
    };
    let before_text = item_text(c, ea);
    let before_hex = c
        .call_blocking(Bytes {
            ea,
            len: size as u32,
        })
        .unwrap()
        .hex;
    let input_before = std::fs::read(binary).unwrap();
    writeln!(out, "{ea:#x}: {before_text}  bytes {before_hex}").unwrap();
    let in_range = |rows: &[PatchRow]| {
        rows.iter()
            .filter(|r| r.ea >= ea && r.ea < ea + size)
            .count()
    };
    drain(events);

    // Raw bytes.
    check(
        c.call_blocking(PatchBytes {
            ea,
            hex: "CC".into(),
        })
        .unwrap()
            == 1,
        "patch_bytes reports the bytes written",
    );
    let ev = drain(events);
    check(
        changes(&ev)
            .iter()
            .any(|d| d.kinds.iter().any(|k| k == "patch")),
        "patching emits a db_changed event of kind patch",
    );
    let b = c.call_blocking(Bytes { ea, len: 1 }).unwrap();
    check(
        b.get(0) == Some(0xCC),
        "patch_bytes changes the bytes read back",
    );
    check(
        c.call_blocking(PatchBytes {
            ea,
            hex: "XYZ".into(),
        })
        .is_err()
            && c.call_blocking(PatchBytes {
                ea: 0,
                hex: "90".into(),
            })
            .is_err(),
        "patch_bytes rejects bad hex and unmapped addresses",
    );
    let rows = c.call_blocking(PatchList {}).unwrap();
    let orig0 = u8::from_str_radix(&before_hex[..2], 16).unwrap();
    check(
        rows.iter().any(|r| {
            r.ea == ea && r.original == orig0 && r.patched == 0xCC && r.file_offset.is_some()
        }),
        "patch_list shows the address, the original and the patched byte",
    );
    let reverted = c.call_blocking(RevertPatches { ea, len: size }).unwrap();
    let b = c.call_blocking(Bytes { ea, len: 1 }).unwrap();
    check(
        reverted == 1
            && b.get(0) == Some(orig0)
            && in_range(&c.call_blocking(PatchList {}).unwrap()) == 0,
        "revert_patches restores the byte and empties the list",
    );

    // Assemble.
    let plain = c
        .call_blocking(Assemble {
            ea,
            text: "nop".into(),
            pad: false,
        })
        .unwrap();
    writeln!(out, "assemble nop: {plain:?}").unwrap();
    let b = c.call_blocking(Bytes { ea, len: 1 }).unwrap();
    check(
        plain.hex == "90"
            && plain.len == 1
            && plain.old_len as u64 == size
            && b.get(0) == Some(0x90),
        "assemble `nop` yields 0x90 and writes it",
    );
    check(
        item_text(c, ea).starts_with("nop"),
        "the listing shows the assembled instruction",
    );
    check(
        c.call_blocking(RevertPatches { ea, len: size }).unwrap() == 1
            && item_text(c, ea) == before_text,
        "revert_patches after assemble brings the instruction back",
    );
    let padded = c
        .call_blocking(Assemble {
            ea,
            text: "nop".into(),
            pad: true,
        })
        .unwrap();
    let bytes = c
        .call_blocking(Bytes {
            ea,
            len: size as u32,
        })
        .unwrap();
    let nops = c
        .call_blocking(ListingDown {
            ea,
            count: size as u32,
        })
        .unwrap()
        .lines
        .iter()
        .filter(|l| {
            l.ea >= ea && l.ea < ea + size && tags::strip(&l.tagged).trim().starts_with("nop")
        })
        .count() as u64;
    writeln!(
        out,
        "assemble nop (padded): {padded:?}, nops in listing: {nops}"
    )
    .unwrap();
    check(
        padded.hex == "90".repeat(size as usize) && bytes.hex == padded.hex && nops == size,
        "assemble with pad fills the old instruction with nops",
    );
    check(
        c.call_blocking(Assemble {
            ea,
            text: "this is not an instruction".into(),
            pad: false,
        })
        .is_err(),
        "assemble rejects text that is not an instruction",
    );

    // Patch list and apply to a copy of the input.
    let rows = c.call_blocking(PatchList {}).unwrap();
    for r in rows.iter().filter(|r| r.ea >= ea && r.ea < ea + size) {
        writeln!(
            out,
            "patch {:#x}: {:02X} -> {:02X} file offset {:?}",
            r.ea, r.original, r.patched, r.file_offset
        )
        .unwrap();
    }
    let dir = binary.parent().unwrap().join("applied");
    std::fs::create_dir_all(&dir).unwrap();
    let output = dir.join("patched.bin");
    let applied = c
        .call_blocking(ApplyPatches {
            output: output.display().to_string(),
        })
        .unwrap();
    let input_after = std::fs::read(binary).unwrap();
    let patched = std::fs::read(&output).unwrap();
    let differing: Vec<usize> = (0..patched.len().min(input_after.len()))
        .filter(|&i| patched[i] != input_after[i])
        .collect();
    let mut expected: Vec<usize> = rows
        .iter()
        .filter(|r| r.original != r.patched)
        .filter_map(|r| r.file_offset.map(|o| o as usize))
        .collect();
    expected.sort_unstable();
    writeln!(
        out,
        "apply_patches: written={} skipped={} differing offsets {:x?}",
        applied.written, applied.skipped, differing
    )
    .unwrap();
    check(
        patched.len() == input_after.len() && !differing.is_empty() && differing == expected,
        "the applied copy differs from the input exactly at the patched file offsets",
    );
    check(
        rows.iter().all(|r| {
            r.file_offset
                .is_none_or(|o| patched[o as usize] == r.patched)
        }),
        "the applied copy holds the patched values",
    );
    check(
        input_before == input_after,
        "apply_patches leaves the input file untouched",
    );
    check(
        c.call_blocking(ApplyPatches {
            output: binary.display().to_string(),
        })
        .is_err()
            && std::fs::read(binary).unwrap() == input_before,
        "apply_patches refuses to overwrite the input file",
    );
    let _ = std::fs::remove_dir_all(&dir);

    // Revert the whole range; the restored bytes are code again.
    let reverted = c.call_blocking(RevertPatches { ea, len: size }).unwrap();
    let after_hex = c
        .call_blocking(Bytes {
            ea,
            len: size as u32,
        })
        .unwrap()
        .hex;
    let after_text = item_text(c, ea);
    writeln!(out, "reverted {reverted}: {after_text}  bytes {after_hex}").unwrap();
    check(
        after_hex == before_hex && in_range(&c.call_blocking(PatchList {}).unwrap()) == 0,
        "revert_patches restores the original bytes and leaves no patches",
    );
    check(
        after_text == before_text,
        "revert_patches disassembles the restored bytes again",
    );
}

fn marks(
    c: &Client,
    funcs: &[FuncRow],
    events: &mut Events,
    out: &mut String,
    check: &mut impl FnMut(bool, &str),
) -> (Ea, String) {
    writeln!(out, "\n== marked positions").unwrap();
    let a = funcs[0].start;
    let b = funcs[funcs.len() / 2].start;
    let keep = format!("e2e mark {a:#x}");
    drain(events);
    let slot_a = c
        .call_blocking(MarkSet {
            ea: a,
            desc: "first".into(),
        })
        .unwrap();
    let ev = drain(events);
    check(
        changes(&ev)
            .iter()
            .any(|d| d.kinds.iter().any(|k| k == "display") && d.lo.is_some_and(|lo| lo <= a)),
        "marking a position emits a display change at the address",
    );
    let list = c.call_blocking(MarkList {}).unwrap();
    check(
        list.iter()
            .any(|m| m.slot == slot_a && m.ea == a && m.desc == "first"),
        "mark_list shows the new mark",
    );
    let again = c
        .call_blocking(MarkSet {
            ea: a,
            desc: keep.clone(),
        })
        .unwrap();
    let list = c.call_blocking(MarkList {}).unwrap();
    check(
        again == slot_a
            && list.iter().filter(|m| m.ea == a).count() == 1
            && list.iter().any(|m| m.ea == a && m.desc == keep),
        "marking the same address again replaces the description in the same slot",
    );
    let slot_b = c
        .call_blocking(MarkSet {
            ea: b,
            desc: "second".into(),
        })
        .unwrap();
    check(slot_b != slot_a, "a second address gets its own slot");
    c.call_blocking(MarkDelete { slot: slot_b }).unwrap();
    let list = c.call_blocking(MarkList {}).unwrap();
    writeln!(out, "marks: {list:?}").unwrap();
    check(
        !list.iter().any(|m| m.ea == b) && list.iter().any(|m| m.ea == a),
        "mark_delete removes only that mark",
    );
    check(
        c.call_blocking(MarkDelete { slot: slot_b }).is_err(),
        "mark_delete of an empty slot fails",
    );
    (a, keep)
}

fn function_properties(
    c: &Client,
    funcs: &[FuncRow],
    avoid: Ea,
    events: &mut Events,
    out: &mut String,
    check: &mut impl FnMut(bool, &str),
) {
    writeln!(out, "\n== edit function").unwrap();
    // The smallest ordinary function with a few instructions.
    let f = funcs
        .iter()
        .filter(|f| !f.library && !f.thunk && f.start != avoid && f.end - f.start >= 12)
        .min_by_key(|f| f.end - f.start)
        .unwrap();
    let info = c.call_blocking(FuncEditInfo { ea: f.start }).unwrap();
    writeln!(out, "{info:?}").unwrap();
    check(
        info.start == f.start && info.end == f.end && info.name == f.name,
        "func_edit_info agrees with the function list",
    );
    let lines = c
        .call_blocking(ListingDown {
            ea: f.start,
            count: 200,
        })
        .unwrap();
    let new_end = lines
        .lines
        .iter()
        .map(|l| l.ea)
        .filter(|&e| e > f.start && e < f.end)
        .max()
        .unwrap();
    drain(events);
    c.call_blocking(EditFunc {
        ea: f.start,
        edit: FuncEdit {
            end: Some(new_end),
            ..Default::default()
        },
    })
    .unwrap();
    let ev = drain(events);
    let moved = c
        .call_blocking(Functions {})
        .unwrap()
        .into_iter()
        .find(|g| g.start == f.start)
        .unwrap();
    check(
        moved.end == new_end
            && c.call_blocking(FuncEditInfo { ea: f.start }).unwrap().end == new_end,
        "edit_func moves the end of the function",
    );
    check(
        changes(&ev)
            .iter()
            .any(|d| d.kinds.iter().any(|k| k == "func")),
        "edit_func emits a db_changed event of kind func",
    );
    c.call_blocking(EditFunc {
        ea: f.start,
        edit: FuncEdit {
            end: Some(f.end),
            ..Default::default()
        },
    })
    .unwrap();
    check(
        c.call_blocking(FuncEditInfo { ea: f.start }).unwrap().end == f.end,
        "edit_func puts the end back",
    );

    let flags = |c: &Client| {
        c.call_blocking(FuncSummary { ea: f.start })
            .unwrap()
            .unwrap()
            .flags
    };
    let base_flags = flags(c);
    c.call_blocking(EditFunc {
        ea: f.start,
        edit: FuncEdit {
            noret: Some(true),
            static_def: Some(true),
            library: Some(true),
            ..Default::default()
        },
    })
    .unwrap();
    let after = flags(c);
    let ai = c.call_blocking(FuncEditInfo { ea: f.start }).unwrap();
    writeln!(out, "flags {base_flags:?} -> {after:?}").unwrap();
    check(
        ai.noret && ai.static_def && ai.library,
        "edit_func sets noret, static and library",
    );
    check(
        ["noreturn", "static", "lib"]
            .iter()
            .all(|w| after.iter().any(|f| f == w)),
        "the function summary shows the flags edit_func set",
    );
    c.call_blocking(EditFunc {
        ea: f.start,
        edit: FuncEdit {
            noret: Some(info.noret),
            static_def: Some(info.static_def),
            library: Some(info.library),
            ..Default::default()
        },
    })
    .unwrap();
    check(flags(c) == base_flags, "edit_func clears the flags again");

    c.call_blocking(EditFunc {
        ea: f.start,
        edit: FuncEdit {
            locals: Some(info.locals + 0x20),
            saved_regs: Some(8),
            purged: Some(0x10),
            ..Default::default()
        },
    })
    .unwrap();
    let sized = c.call_blocking(FuncEditInfo { ea: f.start }).unwrap();
    writeln!(
        out,
        "sizes: locals={} saved={} purged={}",
        sized.locals, sized.saved_regs, sized.purged
    )
    .unwrap();
    check(
        sized.locals == info.locals + 0x20 && sized.saved_regs == 8 && sized.purged == 0x10,
        "edit_func sets the local, saved-register and purged sizes",
    );
    c.call_blocking(EditFunc {
        ea: f.start,
        edit: FuncEdit {
            locals: Some(info.locals),
            saved_regs: Some(info.saved_regs),
            purged: Some(info.purged),
            ..Default::default()
        },
    })
    .unwrap();
    let back = c.call_blocking(FuncEditInfo { ea: f.start }).unwrap();
    check(
        back.locals == info.locals
            && back.saved_regs == info.saved_regs
            && back.purged == info.purged,
        "edit_func restores the frame sizes",
    );
    check(
        c.call_blocking(EditFunc {
            ea: 0,
            edit: FuncEdit::default(),
        })
        .is_err()
            && c.call_blocking(EditFunc {
                ea: f.start,
                edit: FuncEdit {
                    end: Some(f.start),
                    ..Default::default()
                },
            })
            .is_err(),
        "edit_func rejects an address outside functions and an end below the start",
    );
}

/// Analysis progress on open: monotonic, ends done, counts the functions.
fn analysis_events(
    events: &[HostEvent],
    functions: usize,
    out: &mut String,
    check: &mut impl FnMut(bool, &str),
) {
    let analysis: Vec<&AnalysisEvent> = events
        .iter()
        .filter_map(|e| {
            if let HostEvent::Analysis(a) = e {
                Some(a)
            } else {
                None
            }
        })
        .collect();
    check(
        analysis.first().is_some_and(|a| a.start && !a.done),
        "the first analysis event starts the run",
    );
    check(
        analysis
            .windows(2)
            .all(|w| w[0].percent <= w[1].percent && w[0].elapsed_ms <= w[1].elapsed_ms),
        "analysis percent and time never go back",
    );
    check(
        analysis.iter().all(|a| a.percent < 100 || a.done),
        "100 percent is only reported with done",
    );
    let last = analysis.last();
    check(
        last.is_some_and(|a| a.done && a.percent == 100),
        "the last analysis event is done at 100 percent",
    );
    check(
        last.is_some_and(|a| a.functions as usize == functions),
        "done reports the number of functions found",
    );
    check(
        analysis.iter().filter(|a| a.done).count() == 1,
        "exactly one done event",
    );
    writeln!(
        out,
        "\n== analysis events\nstarts with a start event, ends with done, monotonic"
    )
    .unwrap();
}

/// Python, host events and interruption, against the live database.
fn python_and_events(
    c: &Client,
    events: &mut Events,
    info: &DbInfo,
    target: Ea,
    dir: &std::path::Path,
    out: &mut String,
    check: &mut impl FnMut(bool, &str),
) {
    writeln!(out, "\n== python and events").unwrap();
    drain(events);

    // Output, results and statement completeness.
    let r = py(c, "print('hello')\n1 + 2", target, false);
    check(
        !r.incomplete
            && r.error.is_none()
            && text_of(&r, "out") == "hello\n"
            && text_of(&r, "result") == "3\n",
        "py_eval returns stdout and the value of a trailing expression",
    );
    let r = py(
        c,
        "x = 5\nfor i in range(2):\n    print(i * x)\n",
        target,
        false,
    );
    check(
        r.error.is_none() && text_of(&r, "out") == "0\n5\n",
        "py_eval runs multi-line source",
    );
    let r = py(c, "x * 2", target, false);
    check(
        text_of(&r, "result") == "10\n",
        "names persist between evaluations",
    );
    let comp = c
        .call_blocking(PyComplete {
            word: "idaapi.get_func_q".into(),
        })
        .unwrap();
    check(
        comp.iter().any(|w| w == "idaapi.get_func_qty"),
        "py_complete offers idaapi.get_func_qty for idaapi.get_func_q",
    );
    let comp = c.call_blocking(PyComplete { word: "x".into() }).unwrap();
    check(
        comp.iter().any(|w| w == "x"),
        "py_complete sees names the console defined",
    );
    check(
        c.call_blocking(PyComplete {
            word: String::new(),
        })
        .unwrap()
        .is_empty(),
        "py_complete of nothing offers nothing",
    );
    let r = py(c, "for i in range(3):", target, false);
    check(
        r.incomplete && r.chunks.is_empty() && r.error.is_none(),
        "an open block is reported as incomplete and nothing runs",
    );
    let r = py(c, "for i in range(3):", target, true);
    check(
        !r.incomplete
            && r.error
                .as_deref()
                .is_some_and(|e| e.contains("IndentationError")),
        "force runs incomplete source and reports the syntax error",
    );
    let r = py(c, "if True:\n    y = 1\n", target, false);
    check(
        !r.incomplete && r.error.is_none(),
        "a block closed by a blank line is complete",
    );
    let r = py(c, "print(1)\nprint(2)", target, false);
    check(
        text_of(&r, "out") == "1\n2\n",
        "several statements run in one go",
    );

    // Errors are data, not request failures.
    let r = py(
        c,
        "def boom():\n    return 1 / 0\n\nboom()\n",
        target,
        false,
    );
    writeln!(out, "traceback: {:?}", r.error).unwrap();
    check(
        r.error.as_deref().is_some_and(|e| {
            e.contains("ZeroDivisionError")
                && e.contains("in boom")
                && e.contains("<console>")
                && e.contains("1 / 0")
        }),
        "an exception comes back as a traceback with the source line",
    );
    check(
        r.error.as_deref().is_some_and(|e| !e.contains("_izs_")),
        "the traceback hides the runner",
    );
    check(
        r.chunks.last().is_some_and(|c| c.0 == "tb"),
        "the traceback is a chunk of its own",
    );
    let r = py(
        c,
        "import sys\nsys.stderr.write('bad\\n')\nprint('good')",
        target,
        false,
    );
    check(
        text_of(&r, "err") == "bad\n" && text_of(&r, "out") == "good\n",
        "stdout and stderr are told apart",
    );
    let r = py(c, "import sys\nsys.exit(3)", target, false);
    check(
        r.error.as_deref().is_some_and(|e| e.contains("SystemExit"))
            && c.call_blocking(Info {}).is_ok(),
        "sys.exit() is an error, not the end of the host",
    );
    check(
        text_of(&py(c, "print('after')", target, false), "out") == "after\n",
        "output goes to the console again after an exception",
    );

    // The cursor address.
    let r = py(c, "idc.here()", target, false);
    check(
        text_of(&r, "result") == format!("{target}\n"),
        "idc.here() is the address the UI passed",
    );
    let r = py(
        c,
        "import ida_kernwin\nida_kernwin.get_screen_ea()",
        target + 4,
        false,
    );
    check(
        text_of(&r, "result") == format!("{}\n", target + 4),
        "ida_kernwin.get_screen_ea() follows too",
    );

    // IDA's messages arrive as log events.
    py(c, "ida_kernwin.msg('izs-marker-line\\n')", target, false);
    let ev = drain(events);
    let logged: Vec<&String> = ev
        .iter()
        .filter_map(|e| {
            if let HostEvent::Log { lines } = e {
                Some(lines)
            } else {
                None
            }
        })
        .flatten()
        .collect();
    writeln!(out, "log events seen: {}", !logged.is_empty()).unwrap();
    check(
        logged.iter().any(|l| l.contains("izs-marker-line")),
        "msg() output arrives as a log event",
    );
    check(changes(&ev).is_empty(), "printing is not a database change");

    // A rename from Python is a python-origin change; one over the protocol is not.
    let r = py(
        c,
        &format!("idc.set_name({target:#x}, 'renamed_by_python')"),
        target,
        false,
    );
    let ev = drain(events);
    let ch = changes(&ev);
    check(
        r.changed && r.error.is_none(),
        "py_eval reports that it changed the database",
    );
    check(
        ch.iter().any(|d| {
            d.origin == ChangeOrigin::Python
                && d.kinds.iter().any(|k| k == "name")
                && d.lo.is_some_and(|lo| lo <= target)
                && d.hi.is_some_and(|hi| hi > target)
        }),
        "a Python rename produces a python db_changed event covering the address",
    );
    let funcs = c.call_blocking(Functions {}).unwrap();
    check(
        funcs
            .iter()
            .any(|f| f.start == target && f.name == "renamed_by_python"),
        "the Python rename is visible through functions",
    );
    check(
        c.call_blocking(Names {})
            .unwrap()
            .iter()
            .any(|n| n.ea == target && n.name == "renamed_by_python"),
        "the Python rename is visible through names",
    );
    c.call_blocking(Rename {
        ea: target,
        name: "renamed_by_request".into(),
    })
    .unwrap();
    let ev = drain(events);
    check(
        !changes(&ev).is_empty()
            && changes(&ev)
                .iter()
                .all(|d| d.origin == ChangeOrigin::Request),
        "a protocol rename is a request-origin change",
    );

    // 10k edits from a script are a handful of events whose ranges cover them.
    let base = info.min_ea;
    let r = py(
        c,
        &format!("for i in range(10000):\n    idc.set_cmt({base:#x} + i, 'bulk', 0)\n"),
        target,
        false,
    );
    let ev = drain(events);
    let ch = changes(&ev);
    let total: u64 = ch.iter().map(|d| d.count).sum();
    let lo = ch.iter().filter_map(|d| d.lo).min();
    let hi = ch.iter().filter_map(|d| d.hi).max();
    writeln!(out, "bulk edit: events bounded = {}", ch.len() <= 40).unwrap();
    eprintln!(
        "bulk edit: {} events, {} notifications, {:?}..{:?}, {} ms",
        ch.len(),
        total,
        lo,
        hi,
        r.elapsed_ms
    );
    check(
        r.error.is_none() && !ch.is_empty() && ch.len() <= 40,
        "10000 comment edits produce a bounded number of db_changed events",
    );
    check(
        total >= 10000 && lo.is_some_and(|l| l <= base) && hi.is_some_and(|h| h >= base + 10000),
        "the coalesced events account for every edit and cover their range",
    );
    check(
        ch.iter()
            .all(|d| d.origin == ChangeOrigin::Python && d.kinds == ["cmt"]),
        "bulk comment events are python-origin cmt changes",
    );
    check(
        c.call_blocking(Undo {}).unwrap()
            && c.call_blocking(GetComment {
                ea: base + 5,
                repeatable: false,
            })
            .unwrap()
            .is_empty(),
        "one undo reverts everything a script did",
    );
    drain(events);

    // A script file, with its path in the traceback.
    let script = dir.join("e2e_script.py");
    std::fs::write(
        &script,
        "print('from file', __name__)\nraise ValueError('boom from file')\n",
    )
    .unwrap();
    let r = c
        .call_blocking(PyRunFile {
            path: script.display().to_string(),
            ea: target,
        })
        .unwrap();
    check(
        text_of(&r, "out") == "from file __main__\n"
            && r.error.as_deref().is_some_and(|e| {
                e.contains("ValueError: boom from file") && e.contains("e2e_script.py")
            }),
        "py_run_file runs a script and reports its traceback",
    );
    let r = c
        .call_blocking(PyRunFile {
            path: dir.join("missing.py").display().to_string(),
            ea: target,
        })
        .unwrap();
    check(
        r.error
            .as_deref()
            .is_some_and(|e| e.contains("FileNotFoundError")),
        "a missing script file is an error in the reply",
    );

    // Interrupting a runaway script.
    let killer = {
        let c = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(600));
            c.call_blocking(Interrupt {})
        })
    };
    let t0 = std::time::Instant::now();
    let r = py(c, "while True:\n    pass\n", target, true);
    let killed = killer.join().unwrap();
    check(killed.is_ok(), "interrupt is answered while a script runs");
    check(
        r.error
            .as_deref()
            .is_some_and(|e| e.contains("KeyboardInterrupt"))
            && t0.elapsed() < std::time::Duration::from_secs(10),
        "interrupt stops a runaway script with KeyboardInterrupt",
    );
    check(
        c.call_blocking(Info {}).is_ok()
            && text_of(&py(c, "1 + 1", target, false), "result") == "2\n",
        "the host serves requests after an interrupt",
    );
    c.call_blocking(Rename {
        ea: target,
        name: "e2e_function".into(),
    })
    .unwrap();
    drain(events);
}

/// Big samples (shell32.dll has 34k functions) would make the dump megabytes:
/// lists are written as their first `DUMP_CAP` entries plus the full count.
const DUMP_CAP: usize = 200;

fn capped_note(out: &mut impl std::fmt::Write, len: usize) {
    if len > DUMP_CAP {
        writeln!(out, "(first {DUMP_CAP} of {len})").unwrap();
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let binary = PathBuf::from(args.next().expect("usage: e2e <binary> [artifact-dir]"));
    let out_dir = PathBuf::from(args.next().unwrap_or_else(|| "artifacts/e2e".into()));
    std::fs::create_dir_all(&out_dir).unwrap();

    let (client, logs, mut events) =
        Client::spawn_with_events(&HostConfig::discover()).expect("spawn host");
    let log_lines =
        std::thread::spawn(move || futures::executor::block_on_stream(logs).collect::<Vec<_>>());

    let t0 = std::time::Instant::now();
    let mut out = String::new();
    let mut failures = Vec::new();
    let mut check = |cond: bool, what: &str| {
        if !cond {
            failures.push(what.to_owned());
        }
    };

    eprintln!("[{:6.1}s] open", t0.elapsed().as_secs_f32());
    let info = client
        .call_blocking(Open {
            path: binary.display().to_string(),
            auto_analyze: true,
            base: None,
        })
        .expect("open");
    writeln!(
        out,
        "== info\nprocessor={} bitness={} file_type={} hexrays={}",
        info.processor, info.bitness, info.file_type, info.hexrays
    )
    .unwrap();
    writeln!(
        out,
        "min={:#x} max={:#x} entry={:?}",
        info.min_ea,
        info.max_ea,
        info.entry.map(|e| format!("{e:#x}"))
    )
    .unwrap();
    check(info.entry.is_some(), "database has an entry point");
    let open_events = drain(&mut events);

    let funcs = client.call_blocking(Functions {}).unwrap();
    analysis_events(&open_events, funcs.len(), &mut out, &mut check);
    eprintln!("[{:6.1}s] functions", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== functions ({})", funcs.len()).unwrap();
    capped_note(&mut out, funcs.len());
    for f in funcs.iter().take(DUMP_CAP) {
        writeln!(
            out,
            "{:#x}-{:#x} lib={} thunk={} {}",
            f.start, f.end, f.library, f.thunk, f.name
        )
        .unwrap();
    }
    check(!funcs.is_empty(), "functions are listed");

    let segs = client.call_blocking(Segments {}).unwrap();
    eprintln!("[{:6.1}s] segments", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== segments ({})", segs.len()).unwrap();
    for s in &segs {
        writeln!(
            out,
            "{:#x}-{:#x} {} {} {} {}",
            s.start, s.end, s.name, s.class, s.perm, s.bitness
        )
        .unwrap();
    }
    check(!segs.is_empty(), "segments are listed");

    let imports = client.call_blocking(Imports {}).unwrap();
    eprintln!("[{:6.1}s] imports", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== imports ({})", imports.len()).unwrap();
    capped_note(&mut out, imports.len());
    for i in imports.iter().take(DUMP_CAP) {
        writeln!(out, "{:#x} {}!{}", i.ea, i.module, i.name).unwrap();
    }

    let exports = client.call_blocking(Exports {}).unwrap();
    eprintln!("[{:6.1}s] exports", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== exports ({})", exports.len()).unwrap();
    capped_note(&mut out, exports.len());
    for e in exports.iter().take(DUMP_CAP) {
        writeln!(out, "{:#x} #{} {}", e.ea, e.ordinal, e.name).unwrap();
    }

    let strings = client.call_blocking(Strings {}).unwrap();
    eprintln!("[{:6.1}s] strings", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== strings ({})", strings.len()).unwrap();
    for s in strings.iter().take(40) {
        writeln!(out, "{:#x} {} {:?}", s.ea, s.kind, s.text).unwrap();
    }
    check(!strings.is_empty(), "strings are listed");

    let names = client.call_blocking(Names {}).unwrap();
    eprintln!("[{:6.1}s] names", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== names ({})", names.len()).unwrap();

    let types = client.call_blocking(LocalTypes {}).unwrap();
    eprintln!("[{:6.1}s] local types", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== local types ({})", types.len()).unwrap();
    for t in types.iter().take(10) {
        writeln!(
            out,
            "#{} {} {} size={:?}\n{}",
            t.ordinal, t.kind, t.name, t.size, t.decl
        )
        .unwrap();
    }

    let entry = info.entry.unwrap_or(funcs[0].start);
    let down = client
        .call_blocking(ListingDown {
            ea: entry,
            count: 40,
        })
        .unwrap();
    eprintln!(
        "[{:6.1}s] listing down from entry",
        t0.elapsed().as_secs_f32()
    );
    writeln!(
        out,
        "\n== listing down from entry ({} lines, next={:?}, edge={})",
        down.lines.len(),
        down.next.map(|e| format!("{e:#x}")),
        down.at_edge
    )
    .unwrap();
    for l in &down.lines {
        writeln!(out, "{:<26} {}", l.prefix, tags::strip(&l.tagged)).unwrap();
    }
    check(
        down.lines.len() >= 40,
        "listing_down produces the requested lines",
    );
    check(
        down.lines.iter().all(|l| l.ea >= entry),
        "listing_down starts at the entry",
    );
    check(
        down.lines.windows(2).all(|w| w[0].ea <= w[1].ea),
        "listing_down is in address order",
    );

    check(
        down.lines
            .iter()
            .all(|l| !tags::strip(&l.tagged).starts_with(".text:")),
        "listing lines carry no IDA prefix (the UI draws its own)",
    );
    if let Some(collapsed) = down
        .lines
        .iter()
        .find(|l| tags::strip(&l.tagged).contains("COLLAPSED"))
    {
        client
            .call_blocking(ToggleCollapse { ea: collapsed.ea })
            .unwrap();
        let expanded = client
            .call_blocking(ListingDown {
                ea: collapsed.ea,
                count: 10,
            })
            .unwrap();
        let first = tags::strip(&expanded.lines[0].tagged);
        eprintln!(
            "[{:6.1}s] expanded collapsed item",
            t0.elapsed().as_secs_f32()
        );
        writeln!(
            out,
            "\n== expanded collapsed item at {:#x}: {first}",
            collapsed.ea
        )
        .unwrap();
        check(
            !first.contains("COLLAPSED"),
            "toggle_collapse expands a collapsed function",
        );
    }

    let up = client
        .call_blocking(ListingUp {
            ea: entry,
            count: 60,
        })
        .unwrap();
    eprintln!(
        "[{:6.1}s] listing up before entry",
        t0.elapsed().as_secs_f32()
    );
    writeln!(
        out,
        "\n== listing up before entry ({} lines)",
        up.lines.len()
    )
    .unwrap();
    for l in &up.lines {
        writeln!(out, "{:<26} {}", l.prefix, tags::strip(&l.tagged)).unwrap();
    }
    check(
        !up.lines.is_empty() || up.at_edge,
        "listing_up produces lines or reaches the start",
    );
    check(
        up.lines.iter().all(|l| l.ea < entry),
        "listing_up stays before the anchor",
    );

    // Pick the largest non-library function for the graph/decompiler checks.
    let big = funcs
        .iter()
        .filter(|f| !f.library && !f.thunk)
        .max_by_key(|f| f.end - f.start)
        .unwrap();
    let graph = client.call_blocking(Graph { ea: big.start }).unwrap();
    eprintln!("[{:6.1}s] graph", t0.elapsed().as_secs_f32());
    writeln!(
        out,
        "\n== graph of {} ({} blocks)",
        graph.name,
        graph.blocks.len()
    )
    .unwrap();
    for (i, b) in graph.blocks.iter().enumerate() {
        let succ: Vec<_> = b
            .succs
            .iter()
            .map(|s| format!("{}:{:?}", s.to, s.kind))
            .collect();
        writeln!(
            out,
            "#{i} {:#x}-{:#x} lines={} ret={} -> [{}]",
            b.start,
            b.end,
            b.lines.len(),
            b.is_ret,
            succ.join(", ")
        )
        .unwrap();
    }
    check(graph.blocks.len() > 1, "graph has several blocks");
    check(
        graph.blocks.iter().all(|b| !b.lines.is_empty()),
        "every block has lines",
    );
    check(
        graph
            .blocks
            .iter()
            .flat_map(|b| &b.succs)
            .all(|s| (s.to as usize) < graph.blocks.len()),
        "edges point at blocks",
    );

    let sum = client
        .call_blocking(FuncSummary { ea: big.start + 1 })
        .unwrap()
        .expect("a function summary");
    writeln!(
        out,
        "
== func_summary of {}
{sum:#?}",
        sum.name
    )
    .unwrap();
    let edges: usize = graph.blocks.iter().map(|b| b.succs.len()).sum();
    check(
        sum.start == big.start && sum.end == big.end && sum.name == big.name,
        "func_summary describes the function that contains the address",
    );
    check(
        sum.blocks as usize == graph.blocks.len() && sum.edges as usize == edges,
        "func_summary block and edge counts agree with the graph",
    );
    check(
        !sum.segment.is_empty() && sum.seg_perm.len() == 3 && !sum.convention.is_empty(),
        "func_summary names the segment, its permissions and the calling convention",
    );
    check(
        sum.callees.iter().all(|c| c.ea != 0 && !c.name.is_empty())
            && sum.callees.windows(2).all(|w| w[0].ea != w[1].ea)
            && !sum.callees.is_empty(),
        "func_summary lists distinct callees",
    );
    {
        // The prototype uses the demangled name, once, and not the sanitised one.
        let bare = sum
            .name
            .rfind('(')
            .filter(|_| sum.name.ends_with(')'))
            .map_or(sum.name.as_str(), |i| &sum.name[..i]);
        check(
            sum.prototype.is_empty() || sum.prototype.matches(bare).count() == 1,
            &format!(
                "func_summary prototype names the function once ({:?} in {:?})",
                bare, sum.prototype
            ),
        );
        check(
            !sum.name.contains("::") || !sum.prototype.contains(&sum.name.replace("::", "__")),
            "func_summary prototype does not use the sanitised name",
        );
    }
    check(
        client
            .call_blocking(FuncSummary { ea: info.min_ea })
            .unwrap()
            .is_none()
            || info.min_ea >= big.start,
        "func_summary is empty outside functions",
    );

    if info.hexrays {
        let pc = client.call_blocking(Decompile { ea: big.start }).unwrap();
        eprintln!("[{:6.1}s] pseudocode", t0.elapsed().as_secs_f32());
        writeln!(out, "\n== pseudocode of {} (error={:?})", pc.name, pc.error).unwrap();
        for l in &pc.lines {
            writeln!(out, "{}", tags::strip(l)).unwrap();
        }
        check(
            pc.error.is_none() && !pc.lines.is_empty(),
            "decompilation succeeds",
        );

        // Resolve every anchor on the first lines and record what they point at.
        eprintln!("[{:6.1}s] pseudocode anchors", t0.elapsed().as_secs_f32());
        writeln!(out, "\n== pseudocode anchors").unwrap();
        let mut saw_lvar = None;
        for l in pc.lines.iter().take(12) {
            let parsed = tags::parse(l);
            for a in parsed.anchors.iter().take(3) {
                let item = client
                    .call_blocking(PseudoItem {
                        func: pc.func,
                        item: a.value,
                    })
                    .unwrap();
                writeln!(
                    out,
                    "{:#x} op={} target={:?} lvar={:?}",
                    a.value, item.op, item.target, item.lvar
                )
                .unwrap();
                if saw_lvar.is_none() {
                    saw_lvar = item.lvar.clone();
                }
            }
        }
        check(saw_lvar.is_some(), "an anchor resolves to a local variable");

        if let Some(var) = saw_lvar {
            client
                .call_blocking(RenameLvar {
                    func: pc.func,
                    old: var.clone(),
                    new: "e2e_renamed".into(),
                })
                .unwrap();
            let after = client.call_blocking(Decompile { ea: big.start }).unwrap();
            check(
                after
                    .lines
                    .iter()
                    .any(|l| tags::strip(l).contains("e2e_renamed")),
                "lvar rename shows in pseudocode",
            );
            writeln!(out, "renamed lvar {var} -> e2e_renamed").unwrap();
            client
                .call_blocking(SetLvarType {
                    func: pc.func,
                    name: "e2e_renamed".into(),
                    decl: "int".into(),
                })
                .unwrap();
            let typed = client.call_blocking(Decompile { ea: big.start }).unwrap();
            check(
                typed
                    .lines
                    .iter()
                    .any(|l| tags::strip(l).trim_start().starts_with("int e2e_renamed;")),
                "lvar retype shows in pseudocode",
            );
        }

        // Clicking a global name in pseudocode must land on an item that
        // targets it: find name-colored runs and resolve the governing anchor.
        eprintln!(
            "[{:6.1}s] pseudocode color runs",
            t0.elapsed().as_secs_f32()
        );
        writeln!(out, "\n== pseudocode color runs").unwrap();
        let samples = [
            pc.lines.iter().find(|l| tags::strip(l).contains("; //")),
            pc.lines
                .iter()
                .find(|l| tags::strip(l).contains("::") && tags::strip(l).contains("(this")),
        ];
        for l in samples.into_iter().flatten() {
            let parsed = tags::parse(l);
            let runs: Vec<_> = parsed
                .runs
                .iter()
                .map(|r| format!("{:?}={:#04x}", &parsed.text[r.range.clone()], r.color))
                .collect();
            writeln!(out, "{}", runs.join(" ")).unwrap();
            let anchors: Vec<_> = parsed
                .anchors
                .iter()
                .map(|a| format!("@{}={:#x}", a.pos, a.value))
                .collect();
            writeln!(out, "  anchors {}", anchors.join(" ")).unwrap();
        }

        eprintln!(
            "[{:6.1}s] pseudocode name clicks",
            t0.elapsed().as_secs_f32()
        );
        writeln!(out, "\n== pseudocode name clicks").unwrap();
        let mut resolved_clicks = 0;
        'lines: for l in &pc.lines {
            let parsed = tags::parse(l);
            for run in &parsed.runs {
                // Hex-Rays reuses listing colors with other meanings, so pick
                // identifier-looking tokens and let the anchor decide.
                let text = &parsed.text[run.range.clone()];
                if text.starts_with("//")
                    || !text.starts_with(|c: char| c.is_alphabetic() || c == '_')
                {
                    continue;
                }
                let Some(anchor) = parsed.anchor_at(run.range.start) else {
                    continue;
                };
                let item = client
                    .call_blocking(PseudoItem {
                        func: pc.func,
                        item: anchor,
                    })
                    .unwrap();
                let word = &parsed.text[run.range.clone()];
                writeln!(
                    out,
                    "{word} -> op={} target={:?}",
                    item.op,
                    item.target.map(|t| format!("{t:#x}"))
                )
                .unwrap();
                if let Some(t) = item.target {
                    let named = client.call_blocking(AddrInfo { ea: t }).unwrap();
                    if named
                        .name
                        .as_deref()
                        .is_some_and(|n| n.contains(word.split('(').next().unwrap_or(word)))
                    {
                        resolved_clicks += 1;
                    }
                }
                if resolved_clicks >= 3 {
                    break 'lines;
                }
            }
        }
        check(
            resolved_clicks > 0,
            "clicking a global name in pseudocode resolves to its address",
        );
    }

    let xrefs = client.call_blocking(XrefsTo { ea: big.start }).unwrap();
    eprintln!("[{:6.1}s] xrefs to", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== xrefs to {} ({})", big.name, xrefs.len()).unwrap();
    for x in &xrefs {
        writeln!(out, "{:#x} {} {:?} {}", x.from, x.kind, x.func, x.text).unwrap();
    }

    let bytes = client.call_blocking(Bytes { ea: entry, len: 16 }).unwrap();
    eprintln!("[{:6.1}s] bytes", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== bytes at entry\n{}", bytes.hex).unwrap();
    check(
        bytes.len() == 16 && bytes.get(0).is_some(),
        "entry bytes are loaded",
    );

    let pattern: String = (0..6)
        .filter_map(|i| bytes.get(i))
        .map(|b| format!("{b:02X} "))
        .collect();
    let found = client
        .call_blocking(FindBytes {
            ea: info.min_ea.saturating_sub(1),
            pattern: pattern.clone(),
            forward: true,
        })
        .unwrap();
    writeln!(
        out,
        "find_bytes {pattern}-> {:?}",
        found.map(|e| format!("{e:#x}"))
    )
    .unwrap();
    check(
        found.is_some_and(|f| f <= entry),
        "byte search finds the entry bytes",
    );

    // Search > next code / data / unexplored / not function / immediate.
    for (kind, want) in [("code", "code"), ("data", "data")] {
        let hit = client
            .call_blocking(FindKind {
                ea: entry,
                kind: kind.into(),
                forward: true,
            })
            .unwrap();
        let got = hit.map(|ea| client.call_blocking(AddrInfo { ea }).unwrap().kind);
        writeln!(
            out,
            "find_kind {kind} from {entry:#x} -> {hit:x?} ({got:?})"
        )
        .unwrap();
        check(
            hit.is_some_and(|h| h > entry) && got.as_deref() == Some(want),
            &format!("find_kind {kind} finds a later {want} item"),
        );
    }
    let bad = client.call_blocking(FindKind {
        ea: entry,
        kind: "nonsense".into(),
        forward: true,
    });
    check(bad.is_err(), "find_kind rejects an unknown kind");
    let back = client
        .call_blocking(FindKind {
            ea: info.max_ea,
            kind: "code".into(),
            forward: false,
        })
        .unwrap();
    check(
        back.is_some_and(|b| b < info.max_ea),
        "find_kind searches backwards too",
    );
    let imm = [0x28u64, 0x20, 0x30, 0x38, 8, 1, 0x40]
        .into_iter()
        .find_map(|value| {
            client
                .call_blocking(FindImm {
                    ea: info.min_ea.saturating_sub(1),
                    value,
                    forward: true,
                })
                .unwrap()
                .map(|ea| (value, ea))
        });
    writeln!(out, "find_imm -> {imm:x?}").unwrap();
    check(
        imm.is_some_and(|(_, ea)| client.call_blocking(AddrInfo { ea }).unwrap().kind == "code"),
        "find_imm lands on an instruction",
    );
    let none = client
        .call_blocking(FindImm {
            ea: info.min_ea.saturating_sub(1),
            value: 0x1234_5678_9abc_def1,
            forward: true,
        })
        .unwrap();
    check(none.is_none(), "find_imm of an absent value finds nothing");

    let resolved = client
        .call_blocking(Resolve {
            expr: big.name.split('(').next().unwrap().to_owned(),
            from: entry,
        })
        .unwrap();
    writeln!(
        out,
        "resolve {} -> {:?}",
        big.name,
        resolved.map(|e| format!("{e:#x}"))
    )
    .unwrap();
    let hex = client
        .call_blocking(Resolve {
            expr: format!("{entry:X}"),
            from: entry,
        })
        .unwrap();
    check(hex == Some(entry), "hex address resolves");

    let ai = client.call_blocking(AddrInfo { ea: entry }).unwrap();
    writeln!(out, "addr_info {:?}", ai).unwrap();

    // Mutations: rename + comment, verify, then undo.
    let target = big.start;
    client
        .call_blocking(Rename {
            ea: target,
            name: "e2e_function".into(),
        })
        .unwrap();
    let renamed = client.call_blocking(Functions {}).unwrap();
    check(
        renamed
            .iter()
            .any(|f| f.start == target && f.name == "e2e_function"),
        "rename shows in functions",
    );
    client
        .call_blocking(SetComment {
            ea: target,
            text: "e2e comment".into(),
            repeatable: false,
        })
        .unwrap();
    let cmt = client
        .call_blocking(GetComment {
            ea: target,
            repeatable: false,
        })
        .unwrap();
    check(cmt == "e2e comment", "comment round-trips");
    let listing = client
        .call_blocking(ListingDown {
            ea: target,
            count: 30,
        })
        .unwrap();
    check(
        listing
            .lines
            .iter()
            .any(|l| tags::strip(&l.tagged).contains("e2e comment")),
        "comment shows in listing",
    );
    let undone = client.call_blocking(Undo {}).unwrap();
    let cmt_after = client
        .call_blocking(GetComment {
            ea: target,
            repeatable: false,
        })
        .unwrap();
    eprintln!("[{:6.1}s] undo", t0.elapsed().as_secs_f32());
    writeln!(
        out,
        "\n== undo: performed={undone} comment_after={cmt_after:?}"
    )
    .unwrap();
    check(undone && cmt_after.is_empty(), "undo reverts the comment");

    let redone = client.call_blocking(Redo {}).unwrap();
    let cmt_redo = client
        .call_blocking(GetComment {
            ea: target,
            repeatable: false,
        })
        .unwrap();
    check(
        redone && cmt_redo == "e2e comment",
        "redo restores the comment",
    );

    let ty = client.call_blocking(GetType { ea: target }).unwrap();
    writeln!(out, "type of target: {ty}").unwrap();
    client
        .call_blocking(SetType {
            ea: target,
            decl: "int __fastcall e2e_function(int e2e_arg, char *e2e_p);".into(),
        })
        .unwrap();
    let ty2 = client.call_blocking(GetType { ea: target }).unwrap();
    writeln!(out, "type after set_type: {ty2}").unwrap();
    check(
        ty2.contains("e2e_arg") && ty2.contains("char *"),
        "set_type applies a prototype",
    );

    // xrefs from a call instruction inside the big function.
    let body = client
        .call_blocking(ListingDown {
            ea: big.start,
            count: 400,
        })
        .unwrap();
    let call = body
        .lines
        .iter()
        .find(|l| tags::strip(&l.tagged).trim_start().starts_with("call "))
        .map(|l| l.ea);
    if let Some(call) = call {
        let xf = client.call_blocking(XrefsFrom { ea: call }).unwrap();
        eprintln!("[{:6.1}s] xrefs from call", t0.elapsed().as_secs_f32());
        writeln!(out, "\n== xrefs from call at {call:#x}").unwrap();
        for x in &xf {
            writeln!(out, "-> {:#x} {} {}", x.to, x.kind, x.text).unwrap();
        }
        check(!xf.is_empty(), "xrefs_from a call is non-empty");
    }

    // Code/data/function editing on a real instruction, restoring as we go.
    let insn = body
        .lines
        .iter()
        .find(|l| tags::strip(&l.tagged).trim_start().starts_with("mov "))
        .map(|l| l.ea);
    if let Some(insn) = insn {
        let kind = |c: &Client| c.call_blocking(AddrInfo { ea: insn }).unwrap().kind;
        client.call_blocking(Undefine { ea: insn }).unwrap();
        let k1 = kind(&client);
        client.call_blocking(MakeData { ea: insn }).unwrap();
        let k2 = kind(&client);
        client.call_blocking(Undefine { ea: insn }).unwrap();
        client.call_blocking(MakeCode { ea: insn }).unwrap();
        let k3 = kind(&client);
        eprintln!("[{:6.1}s] edit", t0.elapsed().as_secs_f32());
        writeln!(
            out,
            "\n== edit at {insn:#x}: undefine->{k1} data->{k2} code->{k3}"
        )
        .unwrap();
        check(
            k1 == "unknown" && k2 == "data" && k3 == "code",
            "undefine/make_data/make_code change item kinds",
        );
    }
    let small = funcs
        .iter()
        .filter(|f| !f.library && !f.thunk && f.start != big.start)
        .min_by_key(|f| f.end - f.start)
        .unwrap();
    client
        .call_blocking(DeleteFunction { ea: small.start })
        .unwrap();
    let gone = client
        .call_blocking(AddrInfo { ea: small.start })
        .unwrap()
        .function
        .is_none();
    client
        .call_blocking(MakeFunction { ea: small.start })
        .unwrap();
    let back = client
        .call_blocking(AddrInfo { ea: small.start })
        .unwrap()
        .func_start
        == Some(small.start);
    check(
        gone && back,
        "delete_function then make_function round-trips",
    );

    let probe = strings
        .first()
        .map(|s| s.text.chars().take(8).collect::<String>())
        .unwrap_or_default();
    let ft = client.call_blocking(FindText {
        ea: info.min_ea,
        text: probe.clone(),
        forward: true,
    });
    eprintln!("[{:6.1}s] find_text", t0.elapsed().as_secs_f32());
    writeln!(
        out,
        "\n== find_text {probe:?} -> {:?}",
        ft.as_ref().map(|o| o.map(|e| format!("{e:#x}")))
    )
    .unwrap();

    let err = client.call_blocking(Graph { ea: 0 });
    check(
        matches!(err, Err(ClientError::Ida(_))),
        "errors come back as Ida errors",
    );

    type_editing(&client, &mut out, &mut check);
    frame_editing(
        &client,
        &funcs,
        big.start,
        info.hexrays,
        &mut out,
        &mut check,
    );
    eprintln!("[{:6.1}s] types and frames", t0.elapsed().as_secs_f32());

    // Listing options: the instruction column of the generated lines follows
    // set_listing_options; the original value is restored afterwards.
    let original_columns: String;
    {
        let indent_of = |client: &Client| -> usize {
            let l = client
                .call_blocking(ListingDown {
                    ea: entry,
                    count: 40,
                })
                .unwrap();
            l.lines
                .iter()
                .map(|l| tags::strip(&l.tagged))
                .find(|t| {
                    t.starts_with(' ')
                        && !t.trim_start().starts_with(';')
                        && t.trim_start().len() > 2
                })
                .map_or(usize::MAX, |t| t.len() - t.trim_start().len())
        };
        let before = indent_of(&client);
        original_columns = inf_columns(&client);
        client
            .call_blocking(SetListingOptions {
                indent: 4,
                cmt_indent: 24,
            })
            .unwrap();
        let narrow = indent_of(&client);
        writeln!(
            out,
            "
== listing options: instruction indent {before} -> {narrow}"
        )
        .unwrap();
        check(
            narrow == 4 && before > narrow,
            "set_listing_options changes the instruction indent of listing lines",
        );
        check(
            client
                .call_blocking(SetListingOptions {
                    indent: 500,
                    cmt_indent: 0,
                })
                .is_err(),
            "set_listing_options rejects nonsense",
        );
        client
            .call_blocking(SetListingOptions {
                indent: before as u32,
                cmt_indent: 40,
            })
            .unwrap();
        check(
            indent_of(&client) == before,
            "set_listing_options can restore the original indent",
        );
    }

    operand_representation(&client, &funcs, &mut events, &mut out, &mut check);
    arrays_and_strings(&client, &strings, funcs[0].start, &mut out, &mut check);
    patching(&client, &funcs, &binary, &mut events, &mut out, &mut check);
    function_properties(
        &client,
        &funcs,
        big.start,
        &mut events,
        &mut out,
        &mut check,
    );
    let (mark_ea, mark_desc) = marks(&client, &funcs, &mut events, &mut out, &mut check);
    eprintln!("[{:6.1}s] edit menu", t0.elapsed().as_secs_f32());

    signatures_lumina_proximity(&client, &binary, &mut out, &mut check);
    python_and_events(
        &client,
        &mut events,
        &info,
        target,
        binary.parent().unwrap(),
        &mut out,
        &mut check,
    );

    // Persistence: save, close, reopen the database in a fresh host.
    client
        .call_blocking(Rename {
            ea: target,
            name: "e2e_saved".into(),
        })
        .unwrap();
    // The UI's listing columns must not reach the file (GAPS S4): save with
    // them set, check they are still applied afterwards, and below that a
    // fresh host finds the original values in the database.
    client
        .call_blocking(SetListingOptions {
            indent: 4,
            cmt_indent: 24,
        })
        .unwrap();
    client.call_blocking(Save {}).unwrap();
    let applied = inf_columns(&client);
    writeln!(
        out,
        "
== listing columns and save: original {original_columns}, after save {applied}"
    )
    .unwrap();
    check(
        applied == "4,24",
        "save re-applies the UI's listing columns to the open database",
    );
    client.call_blocking(Close { save: true }).unwrap();
    drop(client);
    let logs = log_lines.join().unwrap();

    let idb = PathBuf::from(&info.idb_path);
    eprintln!("[{:6.1}s] reopen", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== reopen (idb exists: {})", idb.exists()).unwrap();
    check(idb.exists(), "saving writes the database next to the input");
    let left = unpacked_files(&binary);
    writeln!(out, "unpacked files after close(save=true): {left:?}").unwrap();
    check(
        left.is_empty(),
        "close(save=true) leaves no unpacked database files",
    );
    for (label, path) in [("idb", idb.clone()), ("input", binary.clone())] {
        let before = std::fs::read(&idb).unwrap();
        let (c, _logs) = Client::spawn(&HostConfig::discover()).expect("respawn host");
        match c.call_blocking(Open {
            path: path.display().to_string(),
            auto_analyze: true,
            base: None,
        }) {
            Ok(_) => {
                let stored = inf_columns(&c);
                writeln!(out, "reopen via {label}: stored columns {stored}").unwrap();
                check(
                    stored == original_columns,
                    &format!("listing columns in the saved database are IDA's own ({label})"),
                );
                let f = c.call_blocking(Functions {}).unwrap();
                let kept = f.iter().any(|f| f.start == target && f.name == "e2e_saved");
                writeln!(out, "reopen via {label}: rename kept = {kept}").unwrap();
                check(kept, &format!("rename survives save + reopen via {label}"));
                let kept_mark = c
                    .call_blocking(MarkList {})
                    .unwrap()
                    .iter()
                    .any(|m| m.ea == mark_ea && m.desc == mark_desc);
                writeln!(out, "reopen via {label}: mark kept = {kept_mark}").unwrap();
                check(
                    kept_mark,
                    &format!("a marked position survives save + reopen via {label}"),
                );
                let types = c.call_blocking(TypeList {}).unwrap();
                check(
                    types.iter().any(|t| t.name == "E2eHolder")
                        && types.iter().any(|t| t.name == "E2eE"),
                    &format!("local types survive save + reopen via {label}"),
                );
            }
            Err(e) => {
                writeln!(out, "reopen via {label}: error {e}").unwrap();
                check(false, &format!("reopen via {label} succeeds"));
            }
        }
        // Edit, then discard: the database on disk must stay byte-identical.
        let _ = c.call_blocking(Rename {
            ea: target,
            name: "e2e_discarded".into(),
        });
        let _ = c.call_blocking(Close { save: false });
        let after = std::fs::read(&idb).unwrap();
        let left = unpacked_files(&binary);
        writeln!(
            out,
            "close(save=false) via {label}: db unchanged = {}, unpacked = {left:?}",
            before == after
        )
        .unwrap();
        check(
            before == after,
            &format!("close(save=false) via {label} leaves the .i64 unchanged"),
        );
        check(
            left.is_empty(),
            &format!("close(save=false) via {label} leaves no unpacked files"),
        );
    }
    raw_blob_as_arm(&binary, &mut out, &mut check);
    raw_blobs_more(&binary, &mut out, &mut check);
    plugins(&binary, &mut out, &mut check);
    eprintln!("[{:6.1}s] host log", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== host log ({} lines)", logs.len()).unwrap();

    eprintln!("[{:6.1}s] checks", t0.elapsed().as_secs_f32());
    writeln!(out, "\n== checks").unwrap();
    if failures.is_empty() {
        writeln!(out, "all passed").unwrap();
    }
    for f in &failures {
        writeln!(out, "FAILED: {f}").unwrap();
    }
    let name = binary.file_name().unwrap().to_string_lossy();
    let path = out_dir.join(format!("{name}.txt"));
    std::fs::write(&path, &out).unwrap();
    std::fs::write(out_dir.join(format!("{name}.log")), logs.join("\n")).unwrap();
    println!("wrote {}", path.display());
    if !failures.is_empty() {
        eprintln!(
            "{} check(s) failed:\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
        std::process::exit(1);
    }
}
