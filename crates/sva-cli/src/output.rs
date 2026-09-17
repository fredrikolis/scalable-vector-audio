// Concern: builds this CLI's subcommand JSON objects | Non-concern: the envelope they are answered in, and the query data (sva-core) | IO: (Output, fields) -> a JSON string

use sva_core::json::{NONE, escape, list, num, strings};

/// `target: null` is a whole-directory lint — the only mode that runs the entry-point check —
/// so a caller can tell an empty `diagnostics` there from an empty one under a target, where
/// that check never runs at all.
pub fn lint_data(dir: &str, target: Option<&str>, nodes: usize) -> String {
    let target = target.map_or_else(|| NONE.to_string(), |t| format!("\"{}\"", escape(t)));
    format!(
        "{{\n  \"dir\": \"{}\",\n  \"target\": {target},\n  \"nodes\": {nodes}\n  }}",
        escape(dir)
    )
}

/// Every key a trace declares is written, `null` where the node has no such thing — the one
/// null convention `sva_core::NONE` states, so a caller reads a field rather than probing for it.
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

fn pair_list(items: &[(&str, &str)], key: &str, value: &str) -> String {
    list(items, |(a, b)| {
        format!(
            "{{ \"{key}\": \"{}\", \"{value}\": \"{}\" }}",
            escape(a),
            escape(b)
        )
    })
}

pub fn builtins_data(b: &crate::Builtins) -> String {
    let callables = list(&b.callables, |c| {
        format!(
            "\n    {{ \"name\": \"{}\", \"required\": {}, \"max_positional\": {}, \"named\": {}, \
             \"required_named\": {}, \"takes_gain\": {} }}",
            escape(c.name),
            c.required,
            c.max_positional,
            strings(c.named),
            strings(c.required_named),
            c.takes_gain
                .map_or_else(|| NONE.to_string(), |g| g.to_string())
        )
    });
    let casts = list(&b.casts, |c| {
        format!(
            "\n    {{ \"name\": \"{}\", \"crossings\": {} }}",
            escape(c.name),
            list(&c.rows, |(from, to)| format!(
                "{{ \"from\": \"{from}\", \"to\": \"{to}\" }}"
            ))
        )
    });
    format!(
        "{{\n  \"callables\": {callables},\n  \"casts\": {casts},\n  \
         \"rule_table\": {{ \"version\": {}, \"families\": {} }},\n  \
         \"refusals\": {},\n  \"unit_suffixes\": {},\n  \"note_names\": \"{}\",\n  \
         \"reserved\": {},\n  \"special_forms\": {},\n  \"not_supported\": {}\n}}",
        b.table_version,
        pair_list(b.families, "name", "duals"),
        pair_list(b.refusals, "code", "when"),
        pair_list(b.unit_suffixes, "suffix", "meaning"),
        escape(b.note_names),
        pair_list(b.reserved, "name", "note"),
        pair_list(b.special_forms, "name", "shape"),
        strings(b.not_supported)
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
