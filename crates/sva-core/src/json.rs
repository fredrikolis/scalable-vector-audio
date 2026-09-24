// Concern: the JSON scalar, array and envelope-meta spellings every emitter agrees on | Non-concern: what any one object holds (output.rs, sva-analysis's json.rs) | IO: (a value) -> a fragment

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

/// One spelling, every emitter, wherever a declared key holds nothing.
pub const NONE: &str = "null";

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// JSON has no NaN or infinity, so a measurement at one reads as `null`.
pub fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        NONE.to_string()
    }
}

/// The `cli` standard's collection: `count` is the whole of it, `has_more` that `items` is
/// not, and `next_cursor` a `--from` value the caller passes back for the rest.
pub fn collection(
    items: impl IntoIterator<Item = String>,
    held: usize,
    next: Option<String>,
) -> String {
    let written: Vec<String> = items.into_iter().collect();
    let cursor = next
        .as_ref()
        .map_or_else(|| NONE.to_string(), |at| format!("\"{}\"", escape(at)));
    format!(
        "{{ \"items\": [{}], \"pagination\": {{ \"count\": {held}, \"has_more\": {}, \"next_cursor\": {cursor} }} }}",
        written.join(", "),
        next.is_some()
    )
}

pub fn list<T>(items: &[T], f: impl Fn(&T) -> String) -> String {
    collection(items.iter().map(f), items.len(), None)
}

/// `items` holds the first `shown`; `resumes_at` names the second the next one begins at.
pub fn capped<T>(
    items: &[T],
    shown: usize,
    resumes_at: impl Fn(usize) -> f64,
    f: impl Fn(&T) -> String,
) -> String {
    let next = (shown < items.len()).then(|| format!("{}s", resumes_at(shown)));
    collection(items[..shown].iter().map(f), items.len(), next)
}

pub fn pair_list(items: &[(&str, &str)], key: &str, value: &str) -> String {
    list(items, |(a, b)| {
        format!(
            "{{ \"{key}\": \"{}\", \"{value}\": \"{}\" }}",
            escape(a),
            escape(b)
        )
    })
}

pub fn strings(items: &[impl AsRef<str>]) -> String {
    list(items, |s| format!("\"{}\"", escape(s.as_ref())))
}

#[cfg(not(target_arch = "wasm32"))]
fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(target_arch = "wasm32")]
fn timestamp() -> u64 {
    0
}

/// Nanoseconds and the process id part two runs inside one second; a wasm module has
/// neither, and answers one page's sequence off the counter alone.
fn origin() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        nanos ^ ((std::process::id() as u64) << 40)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// So an agent can quote the response it is asking about.
fn request_id() -> String {
    static ORIGIN: OnceLock<u64> = OnceLock::new();
    static ANSWERED: AtomicU64 = AtomicU64::new(0);
    let n = ANSWERED.fetch_add(1, Ordering::Relaxed);
    format!("req_{:016x}{n:06x}", ORIGIN.get_or_init(origin))
}

pub fn meta() -> String {
    format!(
        "{{ \"request_id\": \"{}\", \"timestamp\": {} }}",
        request_id(),
        timestamp()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_finite_measurement_reads_as_null_in_every_emitter() {
        assert_eq!(num(0.5), "0.5");
        assert_eq!(num(f64::NAN), "null");
        assert_eq!(num(f64::INFINITY), "null");
        assert_eq!(num(f64::NEG_INFINITY), "null");
    }

    #[test]
    fn a_control_character_is_escaped_rather_than_emitted_raw() {
        assert_eq!(escape("a\"b\\c\nd\u{1}"), "a\\\"b\\\\c\\nd\\u0001");
    }

    #[test]
    fn every_response_carries_its_own_request_id_beside_the_timestamp() {
        let (first, second) = (meta(), meta());
        assert!(first.contains("\"request_id\": \"req_"), "{first}");
        assert!(first.contains("\"timestamp\": "), "{first}");
        assert_ne!(first, second, "two responses are never the same request");
    }

    #[test]
    fn the_origin_carries_more_than_the_whole_second_two_runs_would_share() {
        let (a, b) = (origin(), origin());
        assert_ne!(a, b, "the origin carries more than whole seconds");
    }

    #[test]
    fn an_empty_collection_is_an_empty_items_list_with_a_zero_count() {
        assert!(list(&[] as &[f64], |v| num(*v)).contains("\"items\": []"));
        assert!(strings(&[] as &[&str]).contains("\"count\": 0"));
        assert!(strings(&["a", "b\"c"]).contains("[\"a\", \"b\\\"c\"]"));
        let cut = capped(&[1.0, 2.0, 3.0], 2, |n| n as f64 / 4.0, |v| num(*v));
        assert!(
            cut.contains("\"count\": 3") && cut.contains("\"has_more\": true"),
            "{cut}"
        );
        assert!(
            cut.contains("\"items\": [1, 2]"),
            "the page, not the whole: {cut}"
        );
        assert!(
            cut.contains("\"next_cursor\": \"0.5s\""),
            "a `--from` value: {cut}"
        );
    }
}
