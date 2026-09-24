// Concern: builds this CLI's subcommand JSON objects | Non-concern: the envelope they are answered in, and the query data (sva-core) | IO: (Output, fields) -> a JSON string

use sva_core::json::{NONE, escape, list, num, pair_list, strings};

/// `target: null` is a whole-directory lint, the one mode running the entry-point check.
pub fn lint_data(dir: &str, target: Option<&str>, nodes: usize) -> String {
    let target = target.map_or_else(|| NONE.to_string(), |t| format!("\"{}\"", escape(t)));
    format!(
        "{{\n  \"dir\": \"{}\",\n  \"target\": {target},\n  \"nodes\": {nodes}\n  }}",
        escape(dir)
    )
}

/// Every key a trace declares is written, `sva_core::NONE` where the node has none.
pub fn trace_data(t: &crate::Traceable) -> String {
    let d = &t.traced;
    let text = |held: Option<&String>| {
        held.map_or_else(|| NONE.to_string(), |held| format!("\"{}\"", escape(held)))
    };
    format!(
        "{{\n  \"node\": \"{}\",\n  \"expr\": \"{}\",\n  \"ty\": \"{}\",\n  \
         \"discrete\": {},\n  \"bpm\": {},\n  \"meter\": {},\n  \"entry\": {},\n  \
         \"file\": {},\n  \"loop\": {},\n  \"down\": {},\n  \"up\": {}\n}}",
        escape(&d.node),
        escape(&d.expr),
        escape(&d.ty),
        text(d.discrete.as_ref()),
        t.bpm.map_or_else(|| NONE.to_string(), num),
        text(t.meter.as_ref()),
        strings(&d.entry),
        text(d.file.as_ref()),
        d.cycle.as_ref().map_or_else(
            || NONE.to_string(),
            |members| format!("{{ \"order\": {} }}", strings(members))
        ),
        strings(&d.down),
        list(&d.up, |u| {
            format!(
                "\n    {{ \"node\": \"{}\", \"expr\": \"{}\", \"ty\": \"{}\" }}",
                escape(&u.node),
                escape(&u.expr),
                escape(&u.ty)
            )
        })
    )
}

pub fn new_data(s: &crate::Scaffolded) -> String {
    format!(
        "{{ \"root\": \"{}\", \"files\": {}, \"next\": {} }}",
        escape(&s.root.display().to_string()),
        strings(&s.files),
        pair_list(&crate::NEXT, "command", "why")
    )
}

pub fn help_data(page: &str) -> String {
    format!("{{ \"help\": \"{}\" }}", escape(page))
}

pub fn version_data(name: &str, version: &str) -> String {
    format!(
        "{{ \"name\": \"{}\", \"version\": \"{}\" }}",
        escape(name),
        escape(version)
    )
}
