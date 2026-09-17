// Concern: splits a bare filename into its label and optional span suffix | Non-concern: what the span means to a TSV grid (tsv.rs) | IO: (&str) -> (label, Option<FileSpan>)

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpanUnit {
    Bars,
    Seconds,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileSpan {
    pub amount: f64,
    pub unit: SpanUnit,
}

/// Splits on the LAST `-`; the trailing segment is a span only if it is `<number><b|s>`
/// exactly. Anything else (including no `-` at all) leaves the whole name plain.
pub fn parse_filename(name: &str) -> (String, Option<FileSpan>) {
    let Some((base, suffix)) = name.rsplit_once('-') else {
        return (name.to_string(), None);
    };
    let Some(unit_char) = suffix.chars().last() else {
        return (name.to_string(), None);
    };
    let unit = match unit_char {
        'b' => SpanUnit::Bars,
        's' => SpanUnit::Seconds,
        _ => return (name.to_string(), None),
    };
    let number_part = &suffix[..suffix.len() - 1];
    match number_part.parse::<f64>() {
        Ok(amount) if amount.is_finite() && !number_part.is_empty() => {
            (base.to_string(), Some(FileSpan { amount, unit }))
        }
        _ => (name.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_suffix_parses() {
        assert_eq!(
            parse_filename("kick-4b"),
            (
                "kick".to_string(),
                Some(FileSpan {
                    amount: 4.0,
                    unit: SpanUnit::Bars
                })
            )
        );
        assert_eq!(
            parse_filename("pattern-1b"),
            (
                "pattern".to_string(),
                Some(FileSpan {
                    amount: 1.0,
                    unit: SpanUnit::Bars
                })
            )
        );
    }

    #[test]
    fn seconds_suffix_parses_including_fractional() {
        assert_eq!(
            parse_filename("pluck-1.5s"),
            (
                "pluck".to_string(),
                Some(FileSpan {
                    amount: 1.5,
                    unit: SpanUnit::Seconds
                })
            )
        );
    }

    #[test]
    fn a_non_span_trailing_segment_leaves_the_name_plain() {
        assert_eq!(parse_filename("kick-808"), ("kick-808".to_string(), None));
        assert_eq!(parse_filename("lead-dry"), ("lead-dry".to_string(), None));
        assert_eq!(parse_filename("kick"), ("kick".to_string(), None));
        assert_eq!(parse_filename("song-"), ("song-".to_string(), None));
        assert_eq!(parse_filename("song-b"), ("song-b".to_string(), None));
    }
}
