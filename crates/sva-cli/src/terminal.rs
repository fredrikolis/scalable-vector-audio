// Concern: renders diagnostics already answered as terminal lines | Non-concern: finding them (lint.rs), the JSON rendering of the same objects (output.rs) | IO: (&[Diagnostic], color) -> text

use sva_core::{Diagnostic, Severity};

pub fn colored() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// The objects the envelope carries, one line each.
pub fn diagnostics_text(diagnostics: &[Diagnostic], color: bool) -> String {
    let mut out = String::new();
    for d in diagnostics {
        let (word, shade) = match d.severity {
            Severity::Error => ("error", "\x1b[31m"),
            Severity::Warning => ("warning", "\x1b[33m"),
            Severity::Advice => ("advice", "\x1b[36m"),
        };
        let (open, close) = match color {
            true => (shade, "\x1b[0m"),
            false => ("", ""),
        };
        let at = d.file.as_deref().map_or(String::new(), |f| format!(" {f}"));
        out.push_str(&format!(
            "{open}{word}{close}[{}]{at}: {}\n",
            d.code, d.message
        ));
        if let Some(help) = d.help.as_deref() {
            out.push_str(&format!("  help: {help}\n"));
        }
    }
    out.push_str(&format!("{} diagnostic(s)", diagnostics.len()));
    out
}
