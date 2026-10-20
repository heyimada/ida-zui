//! What the loading screen says about IDA's log: plugin-load failures
//! (usually plugins that need IDA's Qt UI) as one line instead of a traceback.
//! The full text stays in the Output panel.

use gpui::SharedString;

/// The plugin files IDA reported as failing to load, once each, in order.
/// IDA prints `<ida dir>\plugins\<file>: <reason>` before the traceback.
fn failed_plugins(log: &[SharedString]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in log {
        let Some((path, reason)) = line.split_once(": ") else {
            continue;
        };
        let reason = reason.to_lowercase();
        if !(reason.contains("fail") || reason.contains("error")) {
            continue;
        }
        let path = path.replace('\\', "/");
        let Some((dir, file)) = path.rsplit_once('/') else {
            continue;
        };
        let is_plugin_file = [".py", ".dll", ".idc"]
            .iter()
            .any(|ext| file.to_lowercase().ends_with(ext));
        if is_plugin_file
            && dir.to_lowercase().ends_with("/plugins")
            && !found.contains(&file.to_owned())
        {
            found.push(file.to_owned());
        }
    }
    found
}

/// One line about the plugins that did not load, if any did not.
pub fn plugin_summary(log: &[SharedString]) -> Option<String> {
    let n = failed_plugins(log).len();
    if n == 0 {
        return None;
    }
    let needs_qt = log
        .iter()
        .any(|l| l.contains("PyQt") || l.contains("QtGui") || l.contains("QtWidgets"));
    let (noun, verb) = if n == 1 {
        ("plugin", "needs")
    } else {
        ("plugins", "need")
    };
    Some(if needs_qt {
        format!(
            "{n} {noun} {verb} IDA's Qt UI and {} skipped",
            if n == 1 { "was" } else { "were" }
        )
    } else {
        format!(
            "{n} {noun} failed to load and {} skipped",
            if n == 1 { "was" } else { "were" }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(lines: &[&str]) -> Vec<SharedString> {
        lines
            .iter()
            .map(|l| SharedString::from(l.to_string()))
            .collect()
    }

    #[test]
    fn a_clean_log_has_no_summary() {
        let l = log(&[
            "IDA is analysing the input file...",
            "Hex-rays version detected: ready to use",
        ]);
        assert_eq!(plugin_summary(&l), None);
    }

    #[test]
    fn a_qt_plugin_failure_is_one_line_however_long_the_traceback() {
        let l = log(&[
            r"C:\Program Files\IDA\plugins\cvutils-getoffset.py: DLL load failed while importing QtGui:",
            "Traceback (most recent call last):",
            r#"  File "C:/Program Files/IDA/plugins/cvutils-getoffset.py", line 50, in <module>"#,
            "    import PyQt5.QtGui as QtGui",
            "ImportError: DLL load failed while importing QtGui: The specified module could not be found.",
        ]);
        assert_eq!(
            plugin_summary(&l).as_deref(),
            Some("1 plugin needs IDA's Qt UI and was skipped")
        );
    }

    #[test]
    fn the_same_plugin_reported_twice_counts_once_and_others_count_separately() {
        let l = log(&[
            "C:/ida/plugins/a.py: Error loading",
            "C:/ida/plugins/a.py: Error loading",
            "C:/ida/plugins/b.dll: failed to initialise",
        ]);
        assert_eq!(
            plugin_summary(&l).as_deref(),
            Some("2 plugins failed to load and were skipped")
        );
    }

    #[test]
    fn colons_in_other_messages_are_not_plugin_failures() {
        let l = log(&[
            "Loading: failed to find a file",
            r"C:\work\scripts\tool.py: error in my own script",
            "plugins: error",
        ]);
        assert_eq!(plugin_summary(&l), None);
    }
}
