//! The overlay's shortcuts: one table that the key dispatch, the shortcut guide and the loupe's
//! hint row all read (`docs/design/overlay-ui-demo.html`, "快捷键：注册表驱动").
//!
//! Every action is declared once ([`Action::spec`]): its group, its row in the guide, its label,
//! the context it works in and its default chords. Nothing else in the crate names a key — the
//! overlay asks [`Keymap::action_for`] what a key press means, and the painted texts come from
//! [`Keymap::key_text`] and [`Keymap::guide`], so a rebound key is rebound everywhere at once.
//!
//! The user's settings carry only the difference from the defaults ([`KeymapOverrides`]):
//! `chords[id]` replaces an action's chords, `visible[row]` shows or hides a guide row. A
//! [`Keymap`] is only ever built through validation, so a conflicting or malformed override is an
//! error when the settings are applied, never a key that silently does two things.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// Where a shortcut applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Context {
    /// The capture session: picking, dragging and editing a selection.
    Pick,
    /// A running scroll capture.
    Scroll,
}

/// Everything the overlay can be asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    LevelWheel,
    LevelUp,
    LevelDown,
    PickClick,
    PickDrag,
    LoupeZoom,
    ColorFormat,
    ColorCopy,
    CoordMode,
    Annotate,
    Confirm,
    Cancel,
    ScrollAuto,
    ScrollFinish,
    ScrollCancel,
}

/// The guide's section an action is listed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Snap,
    Loupe,
    Output,
    Scroll,
}

impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Self::Snap => "吸附",
            Self::Loupe => "放大镜",
            Self::Output => "输出",
            Self::Scroll => "长截图",
        }
    }
}

/// How an action is triggered, which decides the chords it may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// A key press, with or without modifiers.
    Key,
    /// A key held down while the wheel turns (`Z+Wheel`).
    HoldWheel,
    /// A mouse gesture: shown in the guide, never rebound.
    Mouse,
}

/// One action's entry in the table.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    /// The stable name settings refer to.
    pub id: &'static str,
    pub group: Group,
    /// Actions sharing a row are one line of the guide (`滚轮 / ↑ / ↓`); the row is also what
    /// `visible` is keyed by.
    pub row: &'static str,
    pub label: &'static str,
    pub context: Context,
    pub gesture: Gesture,
    /// Default chords in their settings form; any one of them triggers the action.
    pub defaults: &'static [&'static str],
}

/// [`Action::Confirm`]'s label when F7 armed the session: Enter starts the scroll capture.
const CONFIRM_SCROLL_LABEL: &str = "开始长截图";

impl Action {
    /// Every action, in guide order.
    pub const ALL: [Action; 15] = [
        Self::LevelWheel,
        Self::LevelUp,
        Self::LevelDown,
        Self::PickClick,
        Self::PickDrag,
        Self::LoupeZoom,
        Self::ColorFormat,
        Self::ColorCopy,
        Self::CoordMode,
        Self::Annotate,
        Self::Confirm,
        Self::Cancel,
        Self::ScrollAuto,
        Self::ScrollFinish,
        Self::ScrollCancel,
    ];

    pub fn spec(self) -> Spec {
        use Context::{Pick, Scroll};
        use Gesture::{HoldWheel, Key, Mouse};
        let (id, group, row, label, context, gesture, defaults): (_, _, _, _, _, _, &[&str]) =
            match self {
                Self::LevelWheel => (
                    "level.wheel",
                    Group::Snap,
                    "level",
                    "选择上层或下层 UI 元素",
                    Pick,
                    Mouse,
                    &["Wheel"],
                ),
                Self::LevelUp => (
                    "level.up",
                    Group::Snap,
                    "level",
                    "上一层（走向窗口）",
                    Pick,
                    Key,
                    &["ArrowUp"],
                ),
                Self::LevelDown => (
                    "level.down",
                    Group::Snap,
                    "level",
                    "下一层",
                    Pick,
                    Key,
                    &["ArrowDown"],
                ),
                Self::PickClick => (
                    "pick.click",
                    Group::Snap,
                    "pick.click",
                    "选定当前元素",
                    Pick,
                    Mouse,
                    &["Click"],
                ),
                Self::PickDrag => (
                    "pick.drag",
                    Group::Snap,
                    "pick.drag",
                    "自由框选",
                    Pick,
                    Mouse,
                    &["Drag"],
                ),
                Self::LoupeZoom => (
                    "loupe.zoom",
                    Group::Loupe,
                    "loupe.zoom",
                    "放大镜缩放",
                    Pick,
                    HoldWheel,
                    &["Z+Wheel"],
                ),
                Self::ColorFormat => (
                    "color.format",
                    Group::Loupe,
                    "color.format",
                    "切换色值格式",
                    Pick,
                    Key,
                    &["S"],
                ),
                Self::ColorCopy => (
                    "color.copy",
                    Group::Loupe,
                    "color.copy",
                    "复制色值",
                    Pick,
                    Key,
                    &["C"],
                ),
                Self::CoordMode => (
                    "coord.mode",
                    Group::Loupe,
                    "coord.mode",
                    "切换坐标模式",
                    Pick,
                    Key,
                    &["P"],
                ),
                Self::Annotate => (
                    "annotate",
                    Group::Output,
                    "annotate",
                    "标注选区",
                    Pick,
                    Key,
                    &["A"],
                ),
                Self::Confirm => (
                    "confirm",
                    Group::Output,
                    "confirm",
                    "确认截图",
                    Pick,
                    Key,
                    &["Enter"],
                ),
                Self::Cancel => (
                    "cancel",
                    Group::Output,
                    "cancel",
                    "退出截图",
                    Pick,
                    Key,
                    &["Escape"],
                ),
                Self::ScrollAuto => (
                    "scroll.auto",
                    Group::Scroll,
                    "scroll.auto",
                    "自动滚动",
                    Scroll,
                    Key,
                    &["A"],
                ),
                Self::ScrollFinish => (
                    "scroll.finish",
                    Group::Scroll,
                    "scroll.finish",
                    "完成长截图",
                    Scroll,
                    Key,
                    // F9 first: it is also the global hotkey, so it is the key that works after a
                    // click on the page took the focus away — the one the panel's button names.
                    &["F9", "Enter"],
                ),
                Self::ScrollCancel => (
                    "scroll.cancel",
                    Group::Scroll,
                    "scroll.cancel",
                    "取消长截图",
                    Scroll,
                    Key,
                    &["Escape"],
                ),
            };
        Spec {
            id,
            group,
            row,
            label,
            context,
            gesture,
            defaults,
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.spec().id == id)
    }

    /// The guide's text for this action. Enter's meaning depends on how the session started.
    pub fn label(self, scroll_entry: bool) -> &'static str {
        match self {
            Self::Confirm if scroll_entry => CONFIRM_SCROLL_LABEL,
            _ => self.spec().label,
        }
    }
}

/// Guide rows shown when the user has not said otherwise.
const DEFAULT_VISIBLE: [&str; 12] = [
    "level",
    "pick.click",
    "pick.drag",
    "loupe.zoom",
    "color.format",
    "color.copy",
    "coord.mode",
    "confirm",
    "cancel",
    "scroll.auto",
    "scroll.finish",
    "scroll.cancel",
];

/// A keyboard key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    /// `A`–`Z`, `0`–`9`, and the punctuation keys in [`PUNCTUATION`].
    Char(char),
    Enter,
    Escape,
    Tab,
    Space,
    Backspace,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    /// `F1`–`F24`.
    F(u8),
}

/// The punctuation keys a chord may use, by their unshifted character (the physical key).
const PUNCTUATION: &str = "`,./;'[]-=";

const NAMED_KEYS: [(Key, &str); 14] = [
    (Key::Enter, "Enter"),
    (Key::Escape, "Escape"),
    (Key::Tab, "Tab"),
    (Key::Space, "Space"),
    (Key::Backspace, "Backspace"),
    (Key::Delete, "Delete"),
    (Key::Home, "Home"),
    (Key::End, "End"),
    (Key::PageUp, "PageUp"),
    (Key::PageDown, "PageDown"),
    (Key::ArrowUp, "ArrowUp"),
    (Key::ArrowDown, "ArrowDown"),
    (Key::ArrowLeft, "ArrowLeft"),
    (Key::ArrowRight, "ArrowRight"),
];

impl Key {
    fn parse(token: &str) -> Option<Self> {
        let mut chars = token.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            let c = c.to_ascii_uppercase();
            return (c.is_ascii_uppercase() || c.is_ascii_digit() || PUNCTUATION.contains(c))
                .then_some(Self::Char(c));
        }
        if token.eq_ignore_ascii_case("Esc") {
            return Some(Self::Escape);
        }
        if let Some(n) = token
            .strip_prefix(['F', 'f'])
            .and_then(|n| n.parse::<u8>().ok())
        {
            return (1..=24).contains(&n).then_some(Self::F(n));
        }
        NAMED_KEYS
            .iter()
            .find(|(_, name)| name.eq_ignore_ascii_case(token))
            .map(|(key, _)| *key)
    }

    /// The settings form: `S`, `Enter`, `F9`.
    fn name(self) -> String {
        match self {
            Self::Char(c) => c.to_string(),
            Self::F(n) => format!("F{n}"),
            named => NAMED_KEYS
                .iter()
                .find(|(key, _)| *key == named)
                .map(|(_, name)| (*name).to_string())
                .unwrap_or_default(),
        }
    }

    /// What a key cap shows.
    pub fn display(self) -> String {
        match self {
            Self::Escape => "Esc".into(),
            Self::ArrowUp => "↑".into(),
            Self::ArrowDown => "↓".into(),
            Self::ArrowLeft => "←".into(),
            Self::ArrowRight => "→".into(),
            other => other.name(),
        }
    }

    /// The key a Windows virtual-key code names, when it is one a chord can use.
    pub fn from_virtual_key(vk: u32) -> Option<Self> {
        Some(match vk {
            0x41..=0x5A | 0x30..=0x39 => Self::Char(char::from(vk as u8)),
            0x0D => Self::Enter,
            0x1B => Self::Escape,
            0x09 => Self::Tab,
            0x20 => Self::Space,
            0x08 => Self::Backspace,
            0x2E => Self::Delete,
            0x24 => Self::Home,
            0x23 => Self::End,
            0x21 => Self::PageUp,
            0x22 => Self::PageDown,
            0x26 => Self::ArrowUp,
            0x28 => Self::ArrowDown,
            0x25 => Self::ArrowLeft,
            0x27 => Self::ArrowRight,
            0x70..=0x87 => Self::F((vk - 0x6F) as u8),
            0xC0 => Self::Char('`'),
            0xBC => Self::Char(','),
            0xBE => Self::Char('.'),
            0xBF => Self::Char('/'),
            0xBA => Self::Char(';'),
            0xDE => Self::Char('\''),
            0xDB => Self::Char('['),
            0xDD => Self::Char(']'),
            0xBD => Self::Char('-'),
            0xBB => Self::Char('='),
            _ => return None,
        })
    }
}

/// What completes a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    Key(Key),
    Wheel,
    Click,
    Drag,
}

impl Trigger {
    fn display(self) -> String {
        match self {
            Self::Key(key) => key.display(),
            Self::Wheel => "滚轮".into(),
            Self::Click => "单击".into(),
            Self::Drag => "拖拽".into(),
        }
    }

    fn name(self) -> String {
        match self {
            Self::Key(key) => key.name(),
            Self::Wheel => "Wheel".into(),
            Self::Click => "Click".into(),
            Self::Drag => "Drag".into(),
        }
    }
}

/// One way to trigger an action: modifiers, an optional held key (only before the wheel) and the
/// trigger. Its settings form is the tokens joined by `+`, modifiers first: `Ctrl+Shift+C`,
/// `Z+Wheel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub hold: Option<Key>,
    pub trigger: Trigger,
}

impl Chord {
    /// A key pressed with the given modifiers — what a key-down event is.
    pub fn key(key: Key, ctrl: bool, shift: bool, alt: bool) -> Self {
        Self {
            ctrl,
            shift,
            alt,
            hold: None,
            trigger: Trigger::Key(key),
        }
    }

    /// The wheel turned with the given modifiers, while `hold` is down — what a wheel event is.
    pub fn wheel(hold: Option<Key>, ctrl: bool, shift: bool, alt: bool) -> Self {
        Self {
            ctrl,
            shift,
            alt,
            hold,
            trigger: Trigger::Wheel,
        }
    }

    pub fn parse(text: &str) -> Result<Self, KeymapError> {
        let invalid = || KeymapError::InvalidChord(text.to_string());
        let (mut ctrl, mut shift, mut alt, mut hold) = (false, false, false, None);
        let tokens: Vec<&str> = text.split('+').map(str::trim).collect();
        let (last, rest) = tokens.split_last().ok_or_else(invalid)?;
        for token in rest {
            let flag = match token.to_ascii_lowercase().as_str() {
                "ctrl" => &mut ctrl,
                "shift" => &mut shift,
                "alt" => &mut alt,
                _ => {
                    // A plain key before the trigger is a held key; one is all a hand can hold.
                    if hold.is_some() {
                        return Err(invalid());
                    }
                    hold = Some(Key::parse(token).ok_or_else(invalid)?);
                    continue;
                }
            };
            if *flag {
                return Err(invalid());
            }
            *flag = true;
        }
        let trigger = match last.to_ascii_lowercase().as_str() {
            "wheel" => Trigger::Wheel,
            "click" => Trigger::Click,
            "drag" => Trigger::Drag,
            _ => Trigger::Key(Key::parse(last).ok_or_else(invalid)?),
        };
        // Only the wheel is turned while a key is held; `Z+S` would be two key presses.
        if hold.is_some() && trigger != Trigger::Wheel {
            return Err(invalid());
        }
        Ok(Self {
            ctrl,
            shift,
            alt,
            hold,
            trigger,
        })
    }

    /// The key-cap texts, in order: `["Ctrl", "Shift", "C"]`, `["Z", "滚轮"]`.
    pub fn display_tokens(&self) -> Vec<String> {
        let mut tokens = Vec::new();
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.shift, "Shift"),
            (self.alt, "Alt"),
        ] {
            if on {
                tokens.push(name.to_string());
            }
        }
        tokens.extend(self.hold.map(Key::display));
        tokens.push(self.trigger.display());
        tokens
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut tokens = Vec::new();
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.shift, "Shift"),
            (self.alt, "Alt"),
        ] {
            if on {
                tokens.push(name.to_string());
            }
        }
        tokens.extend(self.hold.map(Key::name));
        tokens.push(self.trigger.name());
        f.write_str(&tokens.join("+"))
    }
}

/// Why a set of overrides cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeymapError {
    UnknownAction(String),
    UnknownRow(String),
    InvalidChord(String),
    /// A mouse gesture: it can be hidden from the guide, not rebound.
    FixedAction(&'static str),
    /// The chord does not fit how the action is triggered (a key action given `Z+Wheel`, the
    /// zoom given a plain key).
    WrongGesture {
        action: &'static str,
        chord: String,
    },
    /// An action left with no chord at all could never be triggered.
    NoChord(&'static str),
    /// Two actions in one context answer the same chord.
    Conflict {
        chord: String,
        first: &'static str,
        second: &'static str,
    },
}

impl fmt::Display for KeymapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAction(id) => write!(f, "unknown action `{id}`"),
            Self::UnknownRow(row) => write!(f, "unknown guide row `{row}`"),
            Self::InvalidChord(chord) => write!(f, "`{chord}` is not a chord"),
            Self::FixedAction(id) => write!(f, "`{id}` is a mouse gesture and cannot be rebound"),
            Self::WrongGesture { action, chord } => {
                write!(f, "`{chord}` cannot trigger `{action}`")
            }
            Self::NoChord(id) => write!(f, "`{id}` has no chord left"),
            Self::Conflict {
                chord,
                first,
                second,
            } => write!(f, "`{chord}` is bound to both `{first}` and `{second}`"),
        }
    }
}

impl std::error::Error for KeymapError {}

/// What the user changed: chords per action id, visibility per guide row. Everything absent keeps
/// its default, which is what lets a new default reach users who never touched that action.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct KeymapOverrides {
    chords: BTreeMap<String, Vec<String>>,
    visible: BTreeMap<String, bool>,
}

impl KeymapOverrides {
    pub fn is_empty(&self) -> bool {
        self.chords.is_empty() && self.visible.is_empty()
    }

    /// Bind `action` to `chords`; the defaults again when they are the defaults.
    pub fn set_chords(&mut self, action: Action, chords: &[Chord]) {
        let spec = action.spec();
        let texts: Vec<String> = chords.iter().map(Chord::to_string).collect();
        let defaults: Vec<String> = spec
            .defaults
            .iter()
            .filter_map(|text| Chord::parse(text).ok())
            .map(|chord| chord.to_string())
            .collect();
        if texts == defaults {
            self.chords.remove(spec.id);
        } else {
            self.chords.insert(spec.id.to_string(), texts);
        }
    }

    pub fn set_visible(&mut self, row: &str, visible: bool) {
        if DEFAULT_VISIBLE.contains(&row) == visible {
            self.visible.remove(row);
        } else {
            self.visible.insert(row.to_string(), visible);
        }
    }

    /// Back to every default.
    pub fn reset(&mut self) {
        self.chords.clear();
        self.visible.clear();
    }
}

/// One line of the guide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideRow {
    pub label: &'static str,
    /// The alternatives (drawn with `/` between them), each a list of key caps (drawn with `+`).
    pub chords: Vec<Vec<String>>,
}

/// One section of the guide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideGroup {
    pub name: &'static str,
    pub rows: Vec<GuideRow>,
}

/// One item of the loupe's hint row: the key cap and a two-character name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoupeHint {
    pub key: String,
    pub label: &'static str,
}

/// The loupe's hint row: what the keys do to the loupe, in the order they are drawn.
const LOUPE_HINTS: [(Action, &str); 4] = [
    (Action::ColorFormat, "格式"),
    (Action::ColorCopy, "复制"),
    (Action::CoordMode, "坐标"),
    (Action::LoupeZoom, "缩放"),
];

/// The validated table: every action's chords, the input lookup per context, and which guide rows
/// are shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Keymap {
    chords: BTreeMap<Action, Vec<Chord>>,
    /// Key presses and wheel turns, per context. Clicks and drags are the pointer's own gestures and
    /// are listed for the guide only.
    lookup: HashMap<(Context, Chord), Action>,
    visible: BTreeMap<&'static str, bool>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::new(&KeymapOverrides::default()).expect("the default keymap is valid")
    }
}

impl Keymap {
    /// The defaults with `overrides` applied, or the first reason they cannot be.
    pub fn new(overrides: &KeymapOverrides) -> Result<Self, KeymapError> {
        for id in overrides.chords.keys() {
            let action =
                Action::from_id(id).ok_or_else(|| KeymapError::UnknownAction(id.clone()))?;
            if action.spec().gesture == Gesture::Mouse {
                return Err(KeymapError::FixedAction(action.spec().id));
            }
        }
        let mut chords = BTreeMap::new();
        for action in Action::ALL {
            let spec = action.spec();
            let texts: Vec<&str> = match overrides.chords.get(spec.id) {
                Some(texts) => texts.iter().map(String::as_str).collect(),
                None => spec.defaults.to_vec(),
            };
            let parsed = texts
                .into_iter()
                .map(Chord::parse)
                .collect::<Result<Vec<_>, _>>()?;
            if parsed.is_empty() {
                return Err(KeymapError::NoChord(spec.id));
            }
            for chord in &parsed {
                let fits = match spec.gesture {
                    Gesture::Key => matches!(chord.trigger, Trigger::Key(_)),
                    Gesture::HoldWheel => chord.trigger == Trigger::Wheel && chord.hold.is_some(),
                    Gesture::Mouse => true,
                };
                if !fits {
                    return Err(KeymapError::WrongGesture {
                        action: spec.id,
                        chord: chord.to_string(),
                    });
                }
            }
            chords.insert(action, parsed);
        }

        // Conflicts: every key a context reacts to, including the key a wheel gesture holds — a
        // held `Z` is also a `Z` press, and binding `Z` to something else would fire both.
        let mut lookup = HashMap::new();
        let mut taken: HashMap<(Context, Chord), Action> = HashMap::new();
        for (&action, list) in &chords {
            let spec = action.spec();
            for chord in list {
                let mut claims = vec![*chord];
                if let Some(held) = chord.hold {
                    claims.push(Chord::key(held, false, false, false));
                }
                for claim in claims {
                    if let Some(&other) = taken.get(&(spec.context, claim))
                        && other != action
                    {
                        return Err(KeymapError::Conflict {
                            chord: claim.to_string(),
                            first: other.spec().id,
                            second: spec.id,
                        });
                    }
                    taken.insert((spec.context, claim), action);
                }
                if matches!(chord.trigger, Trigger::Key(_) | Trigger::Wheel) {
                    lookup.insert((spec.context, *chord), action);
                }
            }
        }

        let rows: Vec<&'static str> = Action::ALL.iter().map(|a| a.spec().row).collect();
        let mut visible = BTreeMap::new();
        for row in &rows {
            visible.insert(*row, DEFAULT_VISIBLE.contains(row));
        }
        for (row, shown) in &overrides.visible {
            let Some(known) = rows.iter().find(|known| **known == row.as_str()) else {
                return Err(KeymapError::UnknownRow(row.clone()));
            };
            visible.insert(*known, *shown);
        }
        Ok(Self {
            chords,
            lookup,
            visible,
        })
    }

    /// What a key press or a wheel turn means in `context`.
    pub fn action_for(&self, context: Context, chord: &Chord) -> Option<Action> {
        self.lookup.get(&(context, *chord)).copied()
    }

    /// Whether `key` is held for a wheel gesture in `context`: pressing it arms the gesture
    /// instead of triggering anything.
    pub fn holds(&self, context: Context, key: Key) -> bool {
        self.lookup
            .keys()
            .any(|(ctx, chord)| *ctx == context && chord.hold == Some(key))
    }

    pub fn chords(&self, action: Action) -> &[Chord] {
        self.chords.get(&action).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The key a wheel gesture is held with (`Z` for the zoom).
    pub fn hold_key(&self, action: Action) -> Option<Key> {
        self.chords(action).iter().find_map(|chord| chord.hold)
    }

    /// The action's keys as one cap text: alternatives joined by ` / `, tokens by `+`. A wheel
    /// gesture shows the key it holds, since the wheel is implied by the label beside it.
    pub fn key_text(&self, action: Action) -> String {
        if action.spec().gesture == Gesture::HoldWheel
            && let Some(held) = self.hold_key(action)
        {
            return held.display();
        }
        self.chords(action)
            .iter()
            .map(|chord| chord.display_tokens().join("+"))
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// The action's first chord as one cap text, for a place with room for one: `F9`, `Ctrl+C`.
    /// Empty when the action is unbound.
    pub fn primary_key_text(&self, action: Action) -> String {
        self.chords(action)
            .first()
            .map(|chord| chord.display_tokens().join("+"))
            .unwrap_or_default()
    }

    /// The loupe's hint row with the keys as currently bound.
    pub fn loupe_hints(&self) -> Vec<LoupeHint> {
        LOUPE_HINTS
            .iter()
            .map(|&(action, label)| LoupeHint {
                key: self.key_text(action),
                label,
            })
            .collect()
    }

    pub fn row_visible(&self, row: &str) -> bool {
        self.visible.get(row).copied().unwrap_or(false)
    }

    /// The guide for `context`: visible rows only, actions sharing a row merged, rows grouped in
    /// table order. `scroll_entry` is whether F7 armed the session (Enter's label).
    pub fn guide(&self, context: Context, scroll_entry: bool) -> Vec<GuideGroup> {
        let mut groups: Vec<(Group, Vec<(&'static str, GuideRow)>)> = Vec::new();
        for action in Action::ALL {
            let spec = action.spec();
            if spec.context != context || !self.row_visible(spec.row) {
                continue;
            }
            let chords = self.chords(action).iter().map(Chord::display_tokens);
            let index = match groups.iter().position(|(group, _)| *group == spec.group) {
                Some(index) => index,
                None => {
                    groups.push((spec.group, Vec::new()));
                    groups.len() - 1
                }
            };
            let rows = &mut groups[index].1;
            match rows.iter_mut().find(|(row, _)| *row == spec.row) {
                Some((_, row)) => row.chords.extend(chords),
                None => rows.push((
                    spec.row,
                    GuideRow {
                        label: action.label(scroll_entry),
                        chords: chords.collect(),
                    },
                )),
            }
        }
        groups
            .into_iter()
            .map(|(group, rows)| GuideGroup {
                name: group.name(),
                rows: rows.into_iter().map(|(_, row)| row).collect(),
            })
            .collect()
    }
}

/// Every text the guide and the key caps can draw, whatever the user binds: every label, group
/// name and key-cap name. The embedded font subset is built from (and gated against) this list, so
/// a rebound key never renders in a fallback font.
pub fn drawn_strings() -> Vec<String> {
    let mut strings: Vec<String> = Vec::new();
    for action in Action::ALL {
        strings.push(action.label(false).into());
        strings.push(action.spec().group.name().into());
    }
    strings.push(CONFIRM_SCROLL_LABEL.into());
    strings.extend(LOUPE_HINTS.iter().map(|(_, label)| (*label).to_string()));
    for trigger in [Trigger::Wheel, Trigger::Click, Trigger::Drag] {
        strings.push(trigger.display());
    }
    let mut keys: Vec<Key> = NAMED_KEYS.iter().map(|(key, _)| *key).collect();
    keys.extend((1..=24).map(Key::F));
    keys.extend(
        ('A'..='Z')
            .chain('0'..='9')
            .chain(PUNCTUATION.chars())
            .map(Key::Char),
    );
    strings.extend(keys.into_iter().map(Key::display));
    strings.extend(["Ctrl", "Shift", "Alt", "+", "/"].map(String::from));
    strings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides(chords: &[(&str, &[&str])], visible: &[(&str, bool)]) -> KeymapOverrides {
        KeymapOverrides {
            chords: chords
                .iter()
                .map(|(id, list)| (id.to_string(), list.iter().map(|c| c.to_string()).collect()))
                .collect(),
            visible: visible
                .iter()
                .map(|(row, shown)| (row.to_string(), *shown))
                .collect(),
        }
    }

    fn press(key: Key) -> Chord {
        Chord::key(key, false, false, false)
    }

    /// The defaults are the keys the overlay answered before the table existed.
    #[test]
    fn the_defaults_answer_the_keys_the_overlay_always_had() {
        let map = Keymap::default();
        let pick = |key| map.action_for(Context::Pick, &press(key));
        assert_eq!(pick(Key::Escape), Some(Action::Cancel));
        assert_eq!(pick(Key::Enter), Some(Action::Confirm));
        assert_eq!(pick(Key::ArrowUp), Some(Action::LevelUp));
        assert_eq!(pick(Key::ArrowDown), Some(Action::LevelDown));
        assert_eq!(pick(Key::Char('S')), Some(Action::ColorFormat));
        assert_eq!(pick(Key::Char('C')), Some(Action::ColorCopy));
        assert_eq!(pick(Key::Char('P')), Some(Action::CoordMode));
        assert_eq!(pick(Key::Char('A')), Some(Action::Annotate));
        // The zoom is a hold, not a press: `Z` itself means nothing on its own.
        assert_eq!(pick(Key::Char('Z')), None);
        assert_eq!(map.hold_key(Action::LoupeZoom), Some(Key::Char('Z')));
        let scroll = |key| map.action_for(Context::Scroll, &press(key));
        assert_eq!(scroll(Key::Escape), Some(Action::ScrollCancel));
        assert_eq!(scroll(Key::Enter), Some(Action::ScrollFinish));
        assert_eq!(scroll(Key::F(9)), Some(Action::ScrollFinish));
        assert_eq!(scroll(Key::Char('A')), Some(Action::ScrollAuto));
        // A modifier makes it a different chord.
        assert_eq!(
            map.action_for(
                Context::Pick,
                &Chord::key(Key::Char('C'), false, true, false)
            ),
            None
        );
    }

    #[test]
    fn chords_round_trip_through_their_settings_form() {
        for text in [
            "Ctrl+Shift+C",
            "Z+Wheel",
            "Escape",
            "F9",
            "`",
            "Alt+,",
            "ArrowUp",
        ] {
            assert_eq!(Chord::parse(text).unwrap().to_string(), text);
        }
        assert_eq!(Chord::parse("esc").unwrap().to_string(), "Escape");
        assert_eq!(
            Chord::parse("shift+ctrl+c").unwrap().to_string(),
            "Ctrl+Shift+C"
        );
        for bad in [
            "",
            "Ctrl+",
            "Ctrl+Ctrl+C",
            "Z+S",
            "X+Y+Wheel",
            "F25",
            "Hyper+C",
            "£",
        ] {
            assert!(Chord::parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn caps_show_the_names_the_guide_uses() {
        let map = Keymap::default();
        assert_eq!(map.key_text(Action::Cancel), "Esc");
        assert_eq!(map.key_text(Action::LoupeZoom), "Z");
        assert_eq!(map.key_text(Action::ScrollFinish), "F9 / Enter");
        assert_eq!(map.primary_key_text(Action::ScrollFinish), "F9");
        assert_eq!(map.primary_key_text(Action::ScrollCancel), "Esc");
        assert_eq!(
            Chord::parse("Ctrl+Shift+ArrowUp").unwrap().display_tokens(),
            ["Ctrl", "Shift", "↑"]
        );
    }

    /// A rebinding moves the key everywhere: the old key stops, the new one works, and the cap
    /// text follows.
    #[test]
    fn a_rebound_action_answers_its_new_key_only() {
        let map = Keymap::new(&overrides(
            &[("color.copy", &["Ctrl+C"]), ("loupe.zoom", &["X+Wheel"])],
            &[],
        ))
        .unwrap();
        assert_eq!(map.action_for(Context::Pick, &press(Key::Char('C'))), None);
        assert_eq!(
            map.action_for(
                Context::Pick,
                &Chord::key(Key::Char('C'), true, false, false)
            ),
            Some(Action::ColorCopy)
        );
        assert_eq!(map.key_text(Action::ColorCopy), "Ctrl+C");
        let hints = map.loupe_hints();
        assert_eq!(
            hints
                .iter()
                .map(|h| (h.key.as_str(), h.label))
                .collect::<Vec<_>>(),
            [
                ("S", "格式"),
                ("Ctrl+C", "复制"),
                ("P", "坐标"),
                ("X", "缩放")
            ]
        );
        assert!(map.holds(Context::Pick, Key::Char('X')));
        assert!(!map.holds(Context::Pick, Key::Char('Z')));
        let wheel = |hold| Chord::wheel(hold, false, false, false);
        assert_eq!(
            map.action_for(Context::Pick, &wheel(Some(Key::Char('X')))),
            Some(Action::LoupeZoom)
        );
        assert_eq!(
            map.action_for(Context::Pick, &wheel(Some(Key::Char('Z')))),
            None
        );
        assert_eq!(
            map.action_for(Context::Pick, &wheel(None)),
            Some(Action::LevelWheel),
            "the plain wheel still walks the levels"
        );
    }

    #[test]
    fn overrides_that_cannot_work_are_refused() {
        let error = |chords: &[(&str, &[&str])]| Keymap::new(&overrides(chords, &[])).unwrap_err();
        assert_eq!(
            error(&[("color.copy", &["S"])]),
            KeymapError::Conflict {
                chord: "S".into(),
                first: "color.format",
                second: "color.copy"
            }
        );
        // Holding `Z` is also pressing it.
        assert!(matches!(
            error(&[("color.copy", &["Z"])]),
            KeymapError::Conflict { .. }
        ));
        assert_eq!(
            error(&[("pick.click", &["Enter"])]),
            KeymapError::FixedAction("pick.click")
        );
        assert!(matches!(
            error(&[("loupe.zoom", &["Z"])]),
            KeymapError::WrongGesture { .. }
        ));
        assert!(matches!(
            error(&[("cancel", &["Q+Wheel"])]),
            KeymapError::WrongGesture { .. }
        ));
        assert_eq!(error(&[("cancel", &[])]), KeymapError::NoChord("cancel"));
        assert_eq!(
            error(&[("nope", &["Q"])]),
            KeymapError::UnknownAction("nope".into())
        );
        // The same key in two contexts is not a conflict: they are never live together.
        assert!(Keymap::new(&overrides(&[("scroll.auto", &["S"])], &[])).is_ok());
        assert_eq!(
            Keymap::new(&overrides(&[], &[("nope", true)])).unwrap_err(),
            KeymapError::UnknownRow("nope".into())
        );
    }

    /// The demo's default guide: three groups, the level row merged, Annotate hidden.
    #[test]
    fn the_guide_lists_the_visible_rows_grouped_and_merged() {
        let map = Keymap::default();
        let guide = map.guide(Context::Pick, false);
        let names: Vec<_> = guide.iter().map(|g| g.name).collect();
        assert_eq!(names, ["吸附", "放大镜", "输出"]);
        let level = &guide[0].rows[0];
        assert_eq!(level.label, "选择上层或下层 UI 元素");
        assert_eq!(level.chords, [vec!["滚轮"], vec!["↑"], vec!["↓"]]);
        let labels: Vec<_> = guide
            .iter()
            .flat_map(|g| &g.rows)
            .map(|r| r.label)
            .collect();
        assert_eq!(
            labels,
            [
                "选择上层或下层 UI 元素",
                "选定当前元素",
                "自由框选",
                "放大镜缩放",
                "切换色值格式",
                "复制色值",
                "切换坐标模式",
                "确认截图",
                "退出截图",
            ]
        );
        assert_eq!(guide[1].rows[0].chords, [vec!["Z", "滚轮"]]);
        // F7 changes what Enter does, and so its label.
        let scroll_entry = map.guide(Context::Pick, true);
        assert!(
            scroll_entry
                .iter()
                .flat_map(|g| &g.rows)
                .any(|r| r.label == "开始长截图")
        );
        // Hiding rows removes them, and an emptied group with them.
        let hidden = Keymap::new(&overrides(
            &[],
            &[
                ("loupe.zoom", false),
                ("color.format", false),
                ("color.copy", false),
                ("coord.mode", false),
                ("annotate", true),
            ],
        ))
        .unwrap();
        let guide = hidden.guide(Context::Pick, false);
        assert_eq!(
            guide.iter().map(|g| g.name).collect::<Vec<_>>(),
            ["吸附", "输出"]
        );
        assert_eq!(guide[1].rows[0].label, "标注选区");
    }

    /// Settings keep only the difference; setting a default back removes the entry.
    #[test]
    fn overrides_store_only_what_differs_from_the_defaults() {
        let mut o = KeymapOverrides::default();
        o.set_chords(Action::ColorCopy, &[Chord::parse("Ctrl+C").unwrap()]);
        o.set_visible("annotate", true);
        o.set_visible("level", true);
        assert_eq!(
            serde_json::to_string(&o).unwrap(),
            r#"{"chords":{"color.copy":["Ctrl+C"]},"visible":{"annotate":true}}"#
        );
        o.set_chords(Action::ColorCopy, &[Chord::parse("C").unwrap()]);
        o.set_visible("annotate", false);
        assert!(o.is_empty());
        o.set_visible("level", false);
        o.reset();
        assert!(o.is_empty());
        assert_eq!(Keymap::new(&o).unwrap(), Keymap::default());
    }

    #[test]
    fn virtual_keys_name_the_physical_key() {
        assert_eq!(Key::from_virtual_key(0x53), Some(Key::Char('S')));
        assert_eq!(Key::from_virtual_key(0x31), Some(Key::Char('1')));
        assert_eq!(Key::from_virtual_key(0x78), Some(Key::F(9)));
        assert_eq!(Key::from_virtual_key(0xC0), Some(Key::Char('`')));
        assert_eq!(Key::from_virtual_key(0x1B), Some(Key::Escape));
        assert_eq!(
            Key::from_virtual_key(0x10),
            None,
            "Shift alone is not a key"
        );
    }
}
