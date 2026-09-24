// Concern: the grammar of a node's `Models | Neglects | IO | Tags` line, and tag house style | Non-concern: finding that line, how loudly to object (sva-cli) | IO: (&str) -> DocComment or why not

use std::fmt;

/// Above real compound tags (`sustained-pad`=13, `transient-response`=18).
pub const MAX_TAG_CHARS: usize = 24;
/// Few tags keep `Tags:` a quick triage aid, not a second `Neglects:` field.
pub const MAX_TAGS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocComment {
    pub models: String,
    pub neglects: String,
    pub io_in: String,
    pub io_out: String,
    pub tags: Vec<String>,
}

/// A leading `;` is tolerated, so a caller may pass the node's source line as written.
pub fn parse(line: &str) -> Result<DocComment, String> {
    let content = line.trim_start().trim_start_matches(';').trim();
    let fields: Vec<&str> = content.split(" | ").collect();
    let [models, neglects, io, tags] = fields.as_slice() else {
        return Err("expected exactly four ` | `-delimited fields".to_string());
    };

    let models = models
        .strip_prefix("Models:")
        .ok_or("first field must start with `Models:`")?
        .trim();
    if models.is_empty() {
        return Err("`Models:` field is empty".to_string());
    }

    let neglects = neglects
        .strip_prefix("Neglects:")
        .ok_or("second field must start with `Neglects:`")?
        .trim();
    if neglects.is_empty() {
        return Err("`Neglects:` field is empty".to_string());
    }

    let io = io
        .strip_prefix("IO:")
        .ok_or("third field must start with `IO:`")?
        .trim();
    let Some((input, output)) = io.split_once("->") else {
        return Err("`IO:` field must be `<input> -> <output>`".to_string());
    };
    let (input, output) = (input.trim(), output.trim());
    if input.is_empty() || output.is_empty() {
        return Err("`IO:` field's input and output must both be non-empty".to_string());
    }

    let tags = tags
        .strip_prefix("Tags:")
        .ok_or("fourth field must start with `Tags:`")?
        .trim();
    if tags.is_empty() {
        return Err("`Tags:` field is empty".to_string());
    }
    let tags: Vec<&str> = tags.split(',').map(str::trim).collect();
    if tags.iter().any(|tag| tag.is_empty()) {
        return Err("`Tags:` field has an empty tag between commas".to_string());
    }

    Ok(DocComment {
        models: models.to_string(),
        neglects: neglects.to_string(),
        io_in: input.to_string(),
        io_out: output.to_string(),
        tags: tags.into_iter().map(String::from).collect(),
    })
}

/// House style, not grammar: whether an odd tag is advised or refused is the caller's call.
pub fn is_plain_tag(tag: &str) -> bool {
    !tag.starts_with('-')
        && !tag.ends_with('-')
        && !tag.contains("--")
        && tag.chars().count() <= MAX_TAG_CHARS
        && tag
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl fmt::Display for DocComment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Models: {} | Neglects: {} | IO: {} -> {} | Tags: {}",
            self.models,
            self.neglects,
            self.io_in,
            self.io_out,
            self.tags.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_well_formed_line_parses_into_its_four_fields() {
        let d = parse("; Models: a plucked string | Neglects: body coupling | IO: note -> mix | Tags: pluck, string").unwrap();
        assert_eq!(d.models, "a plucked string");
        assert_eq!(d.neglects, "body coupling");
        assert_eq!((d.io_in.as_str(), d.io_out.as_str()), ("note", "mix"));
        assert_eq!(d.tags, ["pluck", "string"]);
    }

    #[test]
    fn a_parsed_line_prints_back_to_what_parse_reads() {
        let line = "Models: a | Neglects: b | IO: t -> mix | Tags: x, y";
        assert_eq!(parse(line).unwrap().to_string(), line);
    }

    #[test]
    fn each_missing_piece_names_itself() {
        for (line, why) in [
            ("free text", "four"),
            ("Model: a | Neglects: b | IO: t -> m | Tags: x", "`Models:`"),
            (
                "Models: a | Neglects:  | IO: t -> m | Tags: x",
                "`Neglects:` field is empty",
            ),
            (
                "Models: a | Neglects: b | IO: t | Tags: x",
                "`<input> -> <output>`",
            ),
            (
                "Models: a | Neglects: b | IO: t -> m | Tags: x,,y",
                "empty tag",
            ),
        ] {
            let err = parse(line).unwrap_err();
            assert!(err.contains(why), "{line:?} answered {err:?}");
        }
    }

    #[test]
    fn a_tag_count_or_shape_is_style_the_grammar_accepts() {
        let d = parse("Models: a | Neglects: b | IO: t -> m | Tags: w, x, y, Free Text").unwrap();
        assert_eq!(d.tags.len(), MAX_TAGS + 1);
        assert!(!is_plain_tag("Free Text"));
        assert!(is_plain_tag("sustained-pad"));
        assert!(!is_plain_tag("a--b"));
    }
}
