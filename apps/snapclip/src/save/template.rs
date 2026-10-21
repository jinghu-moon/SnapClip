//! Expanding the naming template (spec §5, "命名变量").
//!
//! The template is stored as the user typed it (`SnapClip_[日期]`) and resolved only when saving,
//! so changing the name never forces a re-encode (`docs/42` D9). Expansion is pure string work:
//! no filesystem, no clock — the caller passes the values it already has, which is what makes the
//! result testable without waiting for a particular minute.
//!
//! The five variables are the spec's: `[日期]` `[时间]` `[序号]` `[分辨率]` `[软件名]`.

/// The values a template may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateValues<'a> {
    /// `YYYY-MM-DD`.
    pub date: &'a str,
    /// `HHmm`, minutes only: the spec leaves seconds undecided and the name is disambiguated by
    /// the conflict policy instead.
    pub time: &'a str,
    /// The sequence value **before** this save; it advances only after a successful save.
    pub sequence: u32,
    pub width: u64,
    pub height: u64,
    /// The application name for `[软件名]`.
    pub app: &'a str,
}

/// Why a template cannot be expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    /// Nothing to make a name out of.
    Empty,
    /// `[` without a closing `]`.
    Unclosed,
    /// A bracketed word that is not one of the five variables.
    UnknownVariable(String),
    /// The template was fine but produced a name with nothing in it.
    EmptyResult,
}

/// Expands `template` into a file name stem.
///
/// Variables are the five from the spec; `[` and `]` are not otherwise special, so a user who
/// types a literal bracket gets told it is unclosed rather than silently getting one.
pub fn expand(template: &str, values: &TemplateValues<'_>) -> Result<String, TemplateError> {
    if template.is_empty() {
        return Err(TemplateError::Empty);
    }
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            return Err(TemplateError::Unclosed);
        };
        let name = &after[..close];
        out.push_str(&substitute(name, values)?);
        rest = &after[close + 1..];
    }
    out.push_str(rest);

    if out.trim().is_empty() {
        return Err(TemplateError::EmptyResult);
    }
    Ok(out)
}

fn substitute(name: &str, values: &TemplateValues<'_>) -> Result<String, TemplateError> {
    match name {
        "日期" => Ok(values.date.to_owned()),
        "时间" => Ok(values.time.to_owned()),
        // Not zero padded: the spec's example is `7`, and a name is read by a person.
        "序号" => Ok(values.sequence.to_string()),
        "分辨率" => Ok(format!("{}x{}", values.width, values.height)),
        "软件名" => Ok(values.app.to_owned()),
        other => Err(TemplateError::UnknownVariable(other.to_owned())),
    }
}

/// Whether the template asks for the sequence, which is what makes a save advance it.
pub fn uses_sequence(template: &str) -> bool {
    template.contains("[序号]")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> TemplateValues<'static> {
        TemplateValues {
            date: "2026-10-18",
            time: "1432",
            sequence: 7,
            width: 1250,
            height: 809,
            app: "SnapClip",
        }
    }

    /// Every variable produces exactly the shape the spec's table shows.
    #[test]
    fn each_variable_expands_to_the_shape_the_spec_shows() {
        let expanded = expand(
            "[日期]_[时间]_[序号]_[分辨率]_[软件名]",
            &values(),
        )
        .expect("a template of known variables");
        assert_eq!(
            expanded, "2026-10-18_1432_7_1250x809_SnapClip",
            "the spec's examples: 2026-10-18, 1432, 7, 1440x900, SnapClip"
        );
    }

    /// The sequence is not padded: the spec's example is `7`, not `007`.
    #[test]
    fn the_sequence_keeps_its_own_digits() {
        let mut v = values();
        v.sequence = 42;
        assert_eq!(expand("shot_[序号]", &v).expect("expands"), "shot_42");
        v.sequence = 3;
        assert_eq!(expand("shot_[序号]", &v).expect("expands"), "shot_3");
    }

    /// A typo must be refused before a file exists, not silently kept as `[日期x]`.
    #[test]
    fn an_unknown_variable_is_refused() {
        let error = expand("shot_[日期x]", &values()).expect_err("a typo is not a variable");
        assert_eq!(error, TemplateError::UnknownVariable("日期x".to_owned()));
    }

    /// A half-typed bracket is not a name either.
    #[test]
    fn an_unclosed_bracket_is_refused() {
        assert_eq!(expand("shot_[日期", &values()).expect_err("unclosed"), TemplateError::Unclosed);
    }

    /// An empty template, and one that expands to nothing, both leave no name.
    #[test]
    fn an_empty_template_or_an_empty_result_is_refused() {
        assert_eq!(expand("", &values()).expect_err("empty"), TemplateError::Empty);
        let mut v = values();
        v.date = "";
        assert_eq!(
            expand("[日期]", &v).expect_err("expands to nothing"),
            TemplateError::EmptyResult
        );
    }

    /// Only a template that asks for the sequence makes a save advance it.
    #[test]
    fn the_sequence_is_used_only_when_asked_for() {
        assert!(uses_sequence("shot_[序号]"));
        assert!(!uses_sequence("shot_[日期]"));
        assert!(!uses_sequence("shot"));
    }
}
