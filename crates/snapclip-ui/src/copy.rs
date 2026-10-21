//! The eleven validation messages and the two conflict links, **verbatim from the spec**
//!
//! This lives in the shared UI crate rather than in the app so that the font subset's codepoint
//! list can be built from **this module** (overlay_drawn_strings() in snapclip-capture calls
//! these producers). A second copy in the app would be a third source of truth, and the subset
//! gate would then be checking a list that is not the one the panel renders.
//!
//! Source: `docs/design/SnapClip 保存面板 · 功能与 UI 规格.md` §8.2, §8.3, §7.2.
//!
//! One source of truth, on purpose. The panel does not spell these strings itself: it asks for a
//! [`ValidationCopy`] value and renders whatever this module says, so "the same mistake described
//! two ways in two places" is not a bug someone has to notice — it cannot be written down.
//!
//! The placeholders are kept as placeholders here (`{盘}`, `{去重后的字符列表}`, `{名}`) and
//! filled by the small formatters below, so the literal wording stays in exactly one place.

/// Severity: an error blocks saving, a warning does not (`docs/41` §0.3, spec §8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
}

/// The eleven messages, in the order the spec lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationCopy {
    /// §8.2 · error · 为空
    PathEmpty,
    /// §8.2 · error · 不是完整路径
    PathNotAbsolute,
    /// §8.2 · error · 盘符之后含非法字符
    PathIllegalChars,
    /// §8.2 · error · 无写入权限
    NoWritePermission,
    /// §8.2 · error · 磁盘空间不足
    NotEnoughSpace,
    /// §8.2 · warn · 目录不存在
    DirectoryMissing,
    /// §8.3 · error · 模板为空
    StemEmpty,
    /// §8.3 · error · 含非法字符
    StemIllegalChars,
    /// §8.3 · error · 解析后是保留名
    StemReservedName,
    /// §8.3 · error · 完整路径超过 259 字符
    PathTooLong,
    /// §8.3 · warn · 末尾是点或空格
    StemTrailingDotOrSpace,
}

impl ValidationCopy {
    /// Every message, so a test can assert the count and the wording without a second list.
    pub const ALL: [ValidationCopy; 11] = [
        ValidationCopy::PathEmpty,
        ValidationCopy::PathNotAbsolute,
        ValidationCopy::PathIllegalChars,
        ValidationCopy::NoWritePermission,
        ValidationCopy::NotEnoughSpace,
        ValidationCopy::DirectoryMissing,
        ValidationCopy::StemEmpty,
        ValidationCopy::StemIllegalChars,
        ValidationCopy::StemReservedName,
        ValidationCopy::PathTooLong,
        ValidationCopy::StemTrailingDotOrSpace,
    ];

    pub fn level(self) -> Level {
        match self {
            ValidationCopy::DirectoryMissing | ValidationCopy::StemTrailingDotOrSpace => {
                Level::Warn
            }
            _ => Level::Error,
        }
    }

    /// The message as written in the spec. Messages with a placeholder keep it: use the formatters
    /// below to fill it in.
    pub fn message(self) -> &'static str {
        match self {
            ValidationCopy::PathEmpty => "路径为空",
            ValidationCopy::PathNotAbsolute => "请输入完整路径，如 C:\\Users\\…",
            ValidationCopy::PathIllegalChars => "路径含非法字符",
            ValidationCopy::NoWritePermission => "没有写入权限，请换一个目录",
            ValidationCopy::NotEnoughSpace => "{盘}: 仅剩 x，保存需约 y",
            ValidationCopy::DirectoryMissing => "目录不存在，保存时创建",
            ValidationCopy::StemEmpty => "文件名不能为空",
            ValidationCopy::StemIllegalChars => "含非法字符 {去重后的字符列表}",
            ValidationCopy::StemReservedName => "“{名}”是系统保留名",
            ValidationCopy::PathTooLong => "完整路径超过 260 字符",
            ValidationCopy::StemTrailingDotOrSpace => "末尾的点或空格会被系统去掉",
        }
    }

    /// The repair a warning offers, if any (`立即创建`, `替换为 _`, `去掉`).
    pub fn repair(self) -> Option<&'static str> {
        match self {
            ValidationCopy::DirectoryMissing => Some("立即创建"),
            ValidationCopy::StemIllegalChars => Some("替换为 _"),
            ValidationCopy::StemTrailingDotOrSpace => Some("去掉"),
            _ => None,
        }
    }
}

/// `{盘}: 仅剩 x，保存需约 y` — the volume letter, then the two sizes already formatted by the
/// caller, because "1.2 GB" is the panel's business and the sentence is ours.
pub fn not_enough_space(volume: char, free: &str, needed: &str) -> String {
    format!("{volume}: 仅剩 {free}，保存需约 {needed}")
}

/// `含非法字符 {去重后的字符列表}` — the list is de-duplicated by the caller.
pub fn illegal_chars(list: &str) -> String {
    format!("含非法字符 {list}")
}

/// `“{名}”是系统保留名` — note the full-width quotes, which are part of the wording.
pub fn reserved_name(name: &str) -> String {
    format!("“{name}”是系统保留名")
}

/// The two links the footer offers when the name is taken (spec §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictLink {
    Overwrite,
    Rename,
}

impl ConflictLink {
    pub fn text(self) -> &'static str {
        match self {
            ConflictLink::Overwrite => "改为覆盖",
            ConflictLink::Rename => "改为重命名",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plan says eleven, and a plan that says eleven while the code has ten is a promise nobody
    /// keeps. Changing the spec means changing this number **and** the code, which is the point.
    #[test]
    fn the_plan_and_the_code_agree_on_the_count() {
        assert_eq!(ValidationCopy::ALL.len(), 11, "docs/41 §0.3 lists eleven messages");
    }

    /// Two rules that read the same would be indistinguishable to a user and to a test.
    #[test]
    fn the_eleven_messages_are_distinct_and_clean() {
        let mut seen = std::collections::BTreeSet::new();
        for copy in ValidationCopy::ALL {
            let message = copy.message();
            assert!(!message.is_empty(), "{copy:?} has no wording");
            assert_eq!(message.trim(), message, "{copy:?} has stray whitespace");
            assert!(seen.insert(message), "{copy:?} repeats another message: {message}");
        }
        assert_eq!(seen.len(), 11);
    }

    /// The formatters fill a placeholder without rewriting the rest of the sentence.
    #[test]
    fn a_formatted_message_keeps_its_literal_wording() {
        assert_eq!(
            not_enough_space('E', "1 MB", "3 MB"),
            "E: 仅剩 1 MB，保存需约 3 MB"
        );
        assert_eq!(illegal_chars("<>"), "含非法字符 <>");
        assert_eq!(
            reserved_name("CON"),
            "“CON”是系统保留名",
            "the quotes are full width and part of the message"
        );
    }

    /// Warnings offer a repair; errors do not pretend to.
    #[test]
    fn warnings_carry_a_repair() {
        assert_eq!(ValidationCopy::DirectoryMissing.repair(), Some("立即创建"));
        assert_eq!(ValidationCopy::StemIllegalChars.repair(), Some("替换为 _"));
        assert_eq!(ValidationCopy::StemTrailingDotOrSpace.repair(), Some("去掉"));
        for copy in [
            ValidationCopy::PathEmpty,
            ValidationCopy::PathNotAbsolute,
            ValidationCopy::PathIllegalChars,
            ValidationCopy::NoWritePermission,
            ValidationCopy::NotEnoughSpace,
            ValidationCopy::StemEmpty,
            ValidationCopy::StemReservedName,
            ValidationCopy::PathTooLong,
        ] {
            assert_eq!(copy.level(), Level::Error, "{copy:?} is an error");
            assert_eq!(copy.repair(), None, "{copy:?} offers no repair");
        }
    }

    /// The footer's two links say what pressing them does.
    #[test]
    fn conflict_links_read_as_actions() {
        assert_eq!(ConflictLink::Overwrite.text(), "改为覆盖");
        assert_eq!(ConflictLink::Rename.text(), "改为重命名");
    }

    /// The wording lives in the spec, so this test reads the spec. A message that drifts from it —
    /// or a spec edit nobody carried into the code — fails here rather than in front of a user.
    ///
    /// Markdown escapes the backslash in `C:\Users\…` and the underscore in `替换为 _`, so the
    /// comparison undoes those two escapes: the displayed sentence is the plain one, and that is
    /// what the panel must show.
    #[test]
    fn every_message_appears_verbatim_in_the_spec() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/design/SnapClip 保存面板 · 功能与 UI 规格.md"
        );
        let spec = std::fs::read_to_string(path)
            .expect("the specification is part of the repository")
            .replace("\\\\", "\\")
            .replace("\\_", "_");

        for copy in ValidationCopy::ALL {
            assert!(
                spec.contains(copy.message()),
                "{copy:?} is not in the spec word for word: {}",
                copy.message()
            );
            if let Some(repair) = copy.repair() {
                assert!(spec.contains(repair), "{copy:?} offers a repair the spec does not");
            }
        }
        for link in [ConflictLink::Overwrite, ConflictLink::Rename] {
            assert!(spec.contains(link.text()), "the spec does not offer {:?}", link);
        }
    }
}
