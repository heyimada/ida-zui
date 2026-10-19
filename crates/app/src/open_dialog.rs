// File > Open with options: loader, processor, base address and whether to
// run auto-analysis. idalib cannot take these as open_database arguments, so
// the host is started with the Binary loader forced and the processor is
// picked through a private IDAUSR config keyed by a private file extension
// (see `Session::open_with`). The dialog says where everything ends up.
use crate::actions::{Cancel, Confirm};
use crate::dialog::{
    ButtonKind, DialogEvent, body, button, caption, checkbox, footer, frame, header,
};
use crate::input::{InputEvent, TextInput};
use crate::scrollbar::{self, Axes, Source};
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui::{self, icon};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, PathPromptOptions, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use std::path::{Path, PathBuf};

/// Processor modules offered for raw binaries: IDA's processor name and a label.
/// Each name was checked by opening a blob with it and reading the processor back
/// (tricore, msp430 and xtensa did not round-trip headless, so they are not offered).
pub const PROCESSORS: &[(&str, &str)] = &[
    ("metapc", "x86 / x86-64"),
    ("arm", "ARM / AArch64"),
    ("mipsl", "MIPS little-endian"),
    ("mipsb", "MIPS big-endian"),
    ("ppc", "PowerPC"),
    ("68k", "Motorola 68000"),
    ("8051", "Intel 8051"),
    ("avr", "Atmel AVR"),
    ("riscv", "RISC-V"),
    ("sh4", "Hitachi SH"),
    ("z80", "Zilog Z80"),
    ("wasm", "WebAssembly"),
];

use ida_proto::client::processor_extension;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Loader {
    Auto,
    /// IDA's "Binary file" loader: the whole file as one segment.
    Binary,
}

#[derive(Clone, Debug)]
pub struct OpenRequest {
    pub path: PathBuf,
    pub loader: Loader,
    /// IDA processor module name; only used with the binary loader.
    pub processor: String,
    /// Address to rebase the program to after loading.
    pub base: Option<u64>,
    pub run_auto: bool,
}

impl OpenRequest {
    /// A plain open: the loader is chosen by IDA and auto-analysis runs.
    pub fn auto(path: PathBuf) -> Self {
        Self {
            path,
            loader: Loader::Auto,
            processor: String::new(),
            base: None,
            run_auto: true,
        }
    }

    pub fn is_plain(&self) -> bool {
        self.loader == Loader::Auto && self.base.is_none() && self.run_auto
    }

    /// metapc is what the Binary loader picks without a hint (and an `izp_metapc`
    /// map entry breaks the lookup for every other processor), so it needs no copy.
    fn needs_copy(&self) -> bool {
        !self.processor.is_empty() && self.processor != "metapc"
    }

    /// The file IDA is actually given: a copy named with the private
    /// extension when a processor is forced, else the file itself.
    pub fn input_path(&self) -> PathBuf {
        if self.loader == Loader::Binary && self.needs_copy() {
            let mut name = self.path.file_name().unwrap_or_default().to_os_string();
            name.push(".");
            name.push(processor_extension(&self.processor));
            self.path.with_file_name(name)
        } else {
            self.path.clone()
        }
    }

    /// Where IDA writes the database.
    pub fn database_path(&self) -> PathBuf {
        let is_db = self
            .path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("i64") || e.eq_ignore_ascii_case("idb"));
        if is_db {
            return self.path.clone();
        }
        let mut p = self.input_path().into_os_string();
        p.push(".i64");
        PathBuf::from(p)
    }
}

pub fn parse_hex(text: &str) -> Option<u64> {
    let t = text
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .replace('_', "");
    u64::from_str_radix(&t, 16).ok()
}

pub struct OpenDialog {
    path: Entity<TextInput>,
    base: Entity<TextInput>,
    loader: Loader,
    processor: usize,
    run_auto: bool,
    error: Option<SharedString>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for OpenDialog {}

impl OpenDialog {
    pub fn new(path: Option<&Path>, cx: &mut Context<Self>) -> Self {
        let initial = path.map(|p| p.display().to_string()).unwrap_or_default();
        let path = cx.new(|cx| {
            TextInput::new(&initial, "Path of the file to open", cx)
                .tall()
                .mono()
        });
        let base = cx.new(|cx| TextInput::new("", "0x08000000", cx).tall().mono());
        let subs = vec![
            cx.subscribe(&path, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&base, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        let run_auto = crate::settings::get(cx).analysis_on_open;
        Self {
            path,
            base,
            loader: Loader::Auto,
            processor: 0,
            run_auto,
            error: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn request(&self, cx: &App) -> Result<OpenRequest, String> {
        let path = PathBuf::from(self.path.read(cx).value().trim().trim_matches('"'));
        if !path.is_file() {
            return Err(format!("'{}' is not a file", path.display()));
        }
        let base_text = self.base.read(cx).value().trim().to_owned();
        let base = if self.loader == Loader::Binary && !base_text.is_empty() {
            Some(
                parse_hex(&base_text)
                    .ok_or_else(|| format!("'{base_text}' is not a hexadecimal address"))?,
            )
        } else {
            None
        };
        Ok(OpenRequest {
            path,
            loader: self.loader,
            processor: if self.loader == Loader::Binary {
                PROCESSORS[self.processor].0.to_owned()
            } else {
                String::new()
            },
            base,
            run_auto: self.run_auto,
        })
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        match self.request(cx) {
            Ok(req) => cx.emit(DialogEvent::Open(req)),
            Err(e) => {
                self.error = Some(e.into());
                cx.notify();
            }
        }
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose the file to open".into()),
        });
        let input = self.path.clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(p) = paths.into_iter().next()
            {
                input.update(cx, |i, cx| i.set_value(&p.display().to_string(), cx));
            }
        })
        .detach();
    }

    fn summary(&self, cx: &App) -> Vec<String> {
        let Ok(req) = self.request(cx) else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        if req.input_path() != req.path {
            lines.push(format!(
                "A copy named {} is created next to the file.",
                req.input_path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ));
        }
        lines.push(format!(
            "The database will be written to {}.",
            req.database_path().display()
        ));
        lines
    }
}

impl Focusable for OpenDialog {
    /// The path field, so typing can start at once.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.path.read(cx).focus_handle(cx)
    }
}

impl Render for OpenDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let t = theme;
        let ph = scrollbar::handle("proc-h", window, cx);
        let pbar =
            scrollbar::scrollbar("proc-scroll", Source::handle(&ph), Axes::V, &t, window, cx);
        let binary = self.loader == Loader::Binary;
        let summary = self.summary(cx);
        // A form row: the label right-aligned in a 160 px column, the field beside it.
        let row = |label: &'static str, field: gpui::AnyElement| {
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(160.))
                        .flex_none()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .justify_end()
                        .text_color(hsla(t.muted))
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(field))
        };
        let loader = ui::segmented_track(&t)
            .child(
                ui::segment("loader-auto", "Automatic", !binary, &t).on_click(cx.listener(
                    |d, _, _, cx| {
                        d.loader = Loader::Auto;
                        cx.notify();
                    },
                )),
            )
            .child(
                ui::segment("loader-binary", "Binary file", binary, &t).on_click(cx.listener(
                    |d, _, _, cx| {
                        d.loader = Loader::Binary;
                        cx.notify();
                    },
                )),
            );
        let processor_list = div()
            .id("proc-list")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&ph)
            .rounded(px(6.))
            .border_1()
            .border_color(hsla(t.border_strong))
            .bg(hsla(t.bg))
            .p(px(4.))
            .children(PROCESSORS.iter().enumerate().map(|(i, (name, label))| {
                let on = i == self.processor;
                div()
                    .id(("proc", i))
                    .h(px(24.))
                    .flex_none()
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(on, |d| d.bg(hsla(t.accent_soft)))
                    .when(!on, |d| d.hover(move |d| d.bg(hsla(t.hover))))
                    .on_click(cx.listener(move |d, _, _, cx| {
                        d.processor = i;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(56.))
                            .flex_none()
                            .font_family(code_font_name())
                            .text_size(px(11.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(*name),
                    )
                    .child(div().flex_1().child(*label))
                    .when(on, |d| d.child(icon("check", 16., hsla(t.accent))))
            }));
        let processors = div()
            .relative()
            .h(px(184.))
            .child(processor_list)
            .child(pbar);
        let info = (!summary.is_empty()).then(|| {
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .p(px(12.))
                .rounded(px(6.))
                .border_1()
                .border_color(hsla(t.border))
                .bg(hsla(t.surface2))
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(hsla(t.muted))
                .child(div().pt(px(1.)).child(icon("info", 16., hsla(t.muted))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .children(summary.into_iter().map(|l| div().child(l))),
                )
        });
        frame(&theme, 640.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &Confirm, _, cx| d.submit(cx)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Close)))
            .child(header(
                &theme,
                "Open with options",
                cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
            ))
            .child(
                body()
                    .child(row(
                        "File",
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(div().flex_1().child(self.path.clone()))
                            .child(
                                button("browse", "Browse…", ButtonKind::Secondary, &t)
                                    .on_click(cx.listener(|d, _, window, cx| d.browse(window, cx))),
                            )
                            .into_any_element(),
                    ))
                    .child(row("Loader", loader.into_any_element()))
                    .when(binary, |d| {
                        d.child(row("Processor", processors.into_any_element()))
                            .child(row(
                                "Base address",
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(div().w(px(200.)).child(self.base.clone()))
                                    .child(caption(&t, "Address of the first byte"))
                                    .into_any_element(),
                            ))
                    })
                    .child(row(
                        "",
                        div()
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .child(
                                checkbox("run-auto", "Run auto-analysis", self.run_auto, &t)
                                    .on_click(cx.listener(|d, _, _, cx| {
                                        d.run_auto = !d.run_auto;
                                        cx.notify();
                                    })),
                            )
                            .into_any_element(),
                    ))
                    .when_some(info, |d, i| d.child(row("", i.into_any_element())))
                    .when_some(self.error.clone(), |d, e| {
                        d.child(row(
                            "",
                            div().text_color(hsla(t.danger)).child(e).into_any_element(),
                        ))
                    }),
            )
            .child(
                footer(&theme)
                    .child(
                        button("cancel", "Cancel", ButtonKind::Secondary, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                    )
                    .child(
                        button("open", "Open", ButtonKind::Primary, &t)
                            .on_click(cx.listener(|d, _, _, cx| d.submit(cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_processor_opens_a_private_copy() {
        let req = OpenRequest {
            path: PathBuf::from(r"C:\fw\blob.bin"),
            loader: Loader::Binary,
            processor: "arm".into(),
            base: None,
            run_auto: true,
        };
        assert_eq!(req.input_path(), PathBuf::from(r"C:\fw\blob.bin.izp_arm"));
        assert_eq!(
            req.database_path(),
            PathBuf::from(r"C:\fw\blob.bin.izp_arm.i64")
        );
    }

    #[test]
    fn plain_open_uses_the_file_itself() {
        let req = OpenRequest::auto(PathBuf::from(r"C:\x\find.exe"));
        assert!(req.is_plain());
        assert_eq!(req.input_path(), PathBuf::from(r"C:\x\find.exe"));
        assert_eq!(req.database_path(), PathBuf::from(r"C:\x\find.exe.i64"));
    }

    #[test]
    fn existing_database_is_its_own_path() {
        let req = OpenRequest::auto(PathBuf::from(r"C:\x\find.exe.i64"));
        assert_eq!(req.database_path(), PathBuf::from(r"C:\x\find.exe.i64"));
    }

    #[test]
    fn hex_addresses_accept_prefixes_and_reject_garbage() {
        assert_eq!(parse_hex("0x08000000"), Some(0x0800_0000));
        assert_eq!(parse_hex(" 1000 "), Some(0x1000));
        assert_eq!(parse_hex("0x"), None);
        assert_eq!(parse_hex("xyz"), None);
    }
}
