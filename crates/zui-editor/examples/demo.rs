// Two editors side by side: a C declaration editor with error markers and a
// Python console. Flags preset state so a screenshot needs no input:
//   --select L:C-L:C   anchor-head of the C editor selection (0-based, byte columns)
//   --cursor L:C       C editor caret (scrolls it into view)
//   --error L[-L]:MSG  error marker on the C editor (repeatable)
//   --big              10,000 generated lines in the C editor
//   --scroll N         first visible line of the C editor
//   --py-select L:C-L:C, --py-cursor L:C   same for the Python editor
//   --read-only        C editor read-only
//   --no-focus         open without taking the foreground (carets stay visible)
use gpui::{
    App, AppContext, Bounds, Context, Entity, Focusable, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use zui_editor::{
    CHighlighter, Editor, EditorEvent, EditorTheme, ErrorMarker, Pos, PythonHighlighter,
};

const C_SAMPLE: &str = r#"#pragma pack(push, 1)
#include <stdint.h>

/* A packet header as recovered from sub_140012A30.
   The checksum covers everything after `magic`. */
struct PacketHeader
{
	uint32_t magic;         // 'PKT1'
	uint16_t version;
	unsigned __int8 flags;
	_BYTE pad[3];
	char name[16];          // "日本語" is fine too
	struct PacketHeader *next;
};

enum PacketKind : int
{
	KIND_DATA = 0x10,
	KIND_ACK  = 0x20,
	KIND_BYE  = 3,
};

typedef int (__fastcall *handler_t)(struct PacketHeader *hdr, void *ctx);

static const char *kMessage = "hello \"world\"";
int __cdecl dispatch(handler_t cb, DWORD kind); /* unterminated
   comment continues on the next line
   and ends here */ int tail;
"#;

const PY_SAMPLE: &str = r#"import idaapi, idc

@staticmethod
def rename_all(prefix="sub_"):
    """Rename every function that
    still has an automatic name."""
    count = 0
    for ea in Functions():
        name = idc.get_func_name(ea)
        if name.startswith(prefix):   # automatic
            idc.set_name(ea, f"fn_{ea:#x}")
            count += 1
    return count

print(rename_all(), 0x1F + 3.5e2)
"#;

struct Args {
    select: Option<(Pos, Pos)>,
    cursor: Option<Pos>,
    errors: Vec<ErrorMarker>,
    big: bool,
    scroll: Option<usize>,
    py_select: Option<(Pos, Pos)>,
    py_cursor: Option<Pos>,
    read_only: bool,
    no_focus: bool,
}

fn pos(s: &str) -> Pos {
    let (l, c) = s.split_once(':').expect("L:C");
    Pos {
        line: l.parse().unwrap(),
        col: c.parse().unwrap(),
    }
}

fn range(s: &str) -> (Pos, Pos) {
    let (a, b) = s.split_once('-').expect("L:C-L:C");
    (pos(a), pos(b))
}

fn parse_args() -> Args {
    let mut out = Args {
        select: None,
        cursor: None,
        errors: Vec::new(),
        big: false,
        scroll: None,
        py_select: None,
        py_cursor: None,
        read_only: false,
        no_focus: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--select" => out.select = args.next().map(|s| range(&s)),
            "--cursor" => out.cursor = args.next().map(|s| pos(&s)),
            "--error" => {
                let s = args.next().unwrap();
                let (lines, msg) = s.split_once(':').unwrap();
                let (a, b) = lines.split_once('-').unwrap_or((lines, lines));
                out.errors.push(ErrorMarker::new(
                    a.parse().unwrap(),
                    b.parse().unwrap(),
                    msg.to_owned(),
                ));
            }
            "--big" => out.big = true,
            "--scroll" => out.scroll = args.next().and_then(|s| s.parse().ok()),
            "--py-select" => out.py_select = args.next().map(|s| range(&s)),
            "--py-cursor" => out.py_cursor = args.next().map(|s| pos(&s)),
            "--read-only" => out.read_only = true,
            "--no-focus" => out.no_focus = true,
            other => eprintln!("unknown flag {other}"),
        }
    }
    out
}

fn big_text() -> String {
    let mut s = String::new();
    for i in 0..10_000 {
        match i % 5 {
            0 => s.push_str(&format!("struct S{i} {{ int a{i}; char *p; }};\n")),
            1 => s.push_str(&format!("\tunsigned __int64 v{i} = 0x{i:X}; // line {i}\n")),
            2 => s.push_str(&format!(
                "int __fastcall f{i}(DWORD x, const char *s) /* c{i} */;\n"
            )),
            3 => s.push_str("#define MASK(x) ((x) & 0xFF)\n"),
            _ => s.push_str(&format!("static const char *k{i} = \"string {i}\";\n")),
        }
    }
    s
}

struct Demo {
    c: Entity<Editor>,
    py: Entity<Editor>,
    status: SharedString,
}

impl Demo {
    fn new(args: Args, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let theme = EditorTheme::dark();
        let text = if args.big {
            big_text()
        } else {
            C_SAMPLE.to_owned()
        };
        let c = cx.new(|cx| {
            Editor::new(cx)
                .with_theme(theme.clone())
                .with_text(&text)
                .with_highlighter(CHighlighter)
                .with_read_only(args.read_only)
        });
        let py = cx.new(|cx| {
            let mut e = Editor::new(cx)
                .with_theme(theme.clone())
                .with_text(PY_SAMPLE)
                .with_highlighter(PythonHighlighter);
            e.set_indent_openers(":");
            e
        });
        c.update(cx, |e, cx| {
            e.set_always_show_caret(args.no_focus, cx);
            if !args.errors.is_empty() {
                e.set_errors(args.errors.clone(), cx);
            }
            if let Some((a, h)) = args.select {
                e.set_selection(a, h, cx);
            } else if let Some(p) = args.cursor {
                e.set_cursor(p, cx);
            }
            if let Some(n) = args.scroll {
                e.scroll_to_line(n, cx);
            }
        });
        py.update(cx, |e, cx| {
            e.set_always_show_caret(args.no_focus, cx);
            if let Some((a, h)) = args.py_select {
                e.set_selection(a, h, cx);
            } else if let Some(p) = args.py_cursor {
                e.set_cursor(p, cx);
            }
        });
        cx.subscribe(&py, |this, py, event: &EditorEvent, cx| {
            if *event == EditorEvent::Submit {
                let n = py.read(cx).text().len();
                this.status = format!("submitted {n} bytes of Python (ctrl+enter)").into();
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&c, |this, c, event: &EditorEvent, cx| {
            if *event == EditorEvent::CursorMoved {
                let p = c.read(cx).cursor();
                this.status = format!("C editor  Ln {}, Col {}", p.line + 1, p.col + 1).into();
                cx.notify();
            }
        })
        .detach();
        if !args.no_focus {
            let handle = c.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        Self {
            c,
            py,
            status: "ready".into(),
        }
    }
}

impl Render for Demo {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = EditorTheme::dark();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.gutter_background)
            .text_color(theme.text)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap_2()
                    .p_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(self.c.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(self.py.clone()),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(12.))
                    .child(self.status.clone()),
            )
    }
}

fn main() {
    let args = parse_args();
    gpui_platform::application().run(move |cx: &mut App| {
        zui_editor::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1200.), px(760.)), cx);
        let focus = !args.no_focus;
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("zui-editor demo".into()),
                    ..Default::default()
                }),
                focus,
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| Demo::new(args, window, cx)),
        );
        if let Err(e) = result {
            eprintln!("Could not open the window: {e}");
            cx.quit();
        }
    });
}
