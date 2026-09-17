// Concern: builds the JSON envelope every answer is written in, success or failure | Non-concern: the data objects it carries (sva-cli's output.rs) | IO: (data or a code, diagnostics) -> JSON

use crate::json::{NONE, escape, list, meta};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Advice,
}

impl Severity {
    fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Advice => "advice",
        }
    }
}

/// One located finding. No `fix` field: an edit needs a span, and no written literal carries one.
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub span: Option<(usize, usize)>,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            code: code.into(),
            severity: Severity::Error,
            message: message.into(),
            file: None,
            line: None,
            span: None,
            help: None,
        }
    }

    pub fn at(mut self, file: Option<String>, span: Option<(usize, usize)>) -> Diagnostic {
        self.file = file;
        self.span = span;
        self
    }

    pub fn helped(mut self, help: impl Into<String>) -> Diagnostic {
        self.help = Some(help.into());
        self
    }

    pub fn on_line(mut self, line: Option<usize>) -> Diagnostic {
        self.line = line;
        self
    }

    pub fn with_severity(mut self, severity: Severity) -> Diagnostic {
        self.severity = severity;
        self
    }
}

pub fn diagnostics_json(diagnostics: &[Diagnostic]) -> String {
    list(diagnostics, diagnostic_json)
}

fn diagnostic_json(d: &Diagnostic) -> String {
    let span = d.span.map_or_else(
        || NONE.to_string(),
        |(o, l)| format!("{{ \"offset\": {o}, \"length\": {l} }}"),
    );
    // A check knowing only the line points at its first column, no guess.
    let start = d.line.map_or_else(
        || NONE.to_string(),
        |l| format!("{{ \"line\": {l}, \"column\": 1 }}"),
    );
    let location = d.file.as_ref().map_or_else(
        || NONE.to_string(),
        |f| {
            format!(
                "{{ \"file\": \"{}\", \"span\": {span}, \"start\": {start}, \"end\": {NONE} }}",
                escape(f)
            )
        },
    );
    let help = d
        .help
        .as_ref()
        .map_or_else(|| NONE.to_string(), |h| format!("\"{}\"", escape(h)));
    format!(
        "{{ \"code\": \"{}\", \"severity\": \"{}\", \"message\": \"{}\", \"location\": {location}, \"help\": {help} }}",
        escape(&d.code),
        d.severity.as_str(),
        escape(&d.message)
    )
}

/// `details` is free-form in the standard: the count and codes an agent branches on, so the
/// located array is read once, at `data.diagnostics`.
fn details_json(diagnostics: &[Diagnostic]) -> String {
    let mut codes: Vec<&str> = Vec::new();
    for d in diagnostics {
        if !codes.contains(&d.code.as_str()) {
            codes.push(&d.code);
        }
    }
    format!(
        "{{ \"count\": {}, \"codes\": {} }}",
        diagnostics.len(),
        crate::json::strings(&codes)
    )
}

pub fn success_envelope(data: &str, diagnostics: &[Diagnostic]) -> String {
    let fields = data
        .strip_suffix('}')
        .expect("every data builder answers one JSON object");
    format!(
        "{{\n  \"status\": \"success\",\n  \"data\": {fields},\n  \"diagnostics\": {}\n  }},\n  \"meta\": {}\n}}",
        diagnostics_json(diagnostics),
        meta()
    )
}

/// `data.diagnostics` is always present, empty where the failure has no subject to locate.
pub fn error_envelope(code: &str, message: &str, diagnostics: &[Diagnostic]) -> String {
    format!(
        "{{\n  \"status\": \"error\",\n  \"error\": {{ \"code\": \"{}\", \"message\": \"{}\", \"details\": {} }},\n  \"data\": {{ \"diagnostics\": {} }},\n  \"meta\": {}\n}}",
        escape(code),
        escape(message),
        details_json(diagnostics),
        diagnostics_json(diagnostics),
        meta()
    )
}
