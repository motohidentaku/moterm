//! config.keys のパースと既定キーバインド（README「キー割当のカスタマイズ」準拠）。

use std::collections::HashMap;

use crate::model::KeyBinding;

/// キーに割り当て可能なアクション
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Launcher,
    /// 左サイドバーのキーボード選択モードに入る（↑↓移動 / Enter 接続 / Esc 解除）。
    SidebarFocus,
    NewTab,
    NextTab,
    PrevTab,
    CloseTab,
    Reconnect,
    Copy,
    Paste,
    Search,
    Download,
    Reload,
    PfPanel,
    /// 右の情報パネル（ホスト情報 + システムメトリクス）の表示切替。
    InfoPanel,
    ScrollPageUp,
    ScrollPageDown,
}

/// バインド対象のキー名
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyName {
    /// 1文字キー（小文字に正規化して保持）
    Char(char),
    /// ファンクションキー F1〜F12
    F(u8),
    PageUp,
    PageDown,
    Tab,
    Enter,
    Esc,
    Up,
    Down,
    Left,
    Right,
}

/// 修飾キーの組み合わせ
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ModMask {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl ModMask {
    pub const NONE: ModMask = ModMask {
        ctrl: false,
        shift: false,
        alt: false,
    };
    pub const CTRL: ModMask = ModMask {
        ctrl: true,
        shift: false,
        alt: false,
    };
    pub const SHIFT: ModMask = ModMask {
        ctrl: false,
        shift: true,
        alt: false,
    };
    pub const CTRL_SHIFT: ModMask = ModMask {
        ctrl: true,
        shift: true,
        alt: false,
    };
}

/// key 文字列のパース。1文字（小文字化）または名前
/// （PageUp/PageDown/Tab/Enter/Esc/F1〜F12/Up/Down/Left/Right、大文字小文字無視）。
pub fn parse_key(s: &str) -> Option<KeyName> {
    let s = s.trim();
    let mut chars = s.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(KeyName::Char(c.to_lowercase().next().unwrap_or(c)));
    }
    let lower = s.to_ascii_lowercase();
    if let Some(num) = lower.strip_prefix('f') {
        let n: u8 = num.parse().ok()?;
        return (1..=12).contains(&n).then_some(KeyName::F(n));
    }
    match lower.as_str() {
        "pageup" => Some(KeyName::PageUp),
        "pagedown" => Some(KeyName::PageDown),
        "tab" => Some(KeyName::Tab),
        "enter" | "return" => Some(KeyName::Enter),
        "esc" | "escape" => Some(KeyName::Esc),
        "up" => Some(KeyName::Up),
        "down" => Some(KeyName::Down),
        "left" => Some(KeyName::Left),
        "right" => Some(KeyName::Right),
        _ => None,
    }
}

/// mods 文字列のパース。CTRL/SHIFT/ALT を `|` または `+` で連結（大文字小文字無視）。
/// 空文字は修飾なし。未知トークンは None。
pub fn parse_mods(s: &str) -> Option<ModMask> {
    let mut mask = ModMask::NONE;
    for token in s.split(['|', '+']) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        match token.to_ascii_uppercase().as_str() {
            "CTRL" | "CONTROL" => mask.ctrl = true,
            "SHIFT" => mask.shift = true,
            "ALT" => mask.alt = true,
            _ => return None,
        }
    }
    Some(mask)
}

/// action 文字列のパース（moterm.lua.example のアクション名）
pub fn parse_action(s: &str) -> Option<Action> {
    match s.trim().to_ascii_lowercase().as_str() {
        "launcher" => Some(Action::Launcher),
        "sidebar_focus" => Some(Action::SidebarFocus),
        "new_tab" => Some(Action::NewTab),
        "next_tab" => Some(Action::NextTab),
        "prev_tab" => Some(Action::PrevTab),
        "close_tab" => Some(Action::CloseTab),
        "reconnect" => Some(Action::Reconnect),
        "copy" => Some(Action::Copy),
        "paste" => Some(Action::Paste),
        "search" => Some(Action::Search),
        "download" => Some(Action::Download),
        "reload" => Some(Action::Reload),
        "pf_panel" => Some(Action::PfPanel),
        "info_panel" => Some(Action::InfoPanel),
        "scroll_page_up" => Some(Action::ScrollPageUp),
        "scroll_page_down" => Some(Action::ScrollPageDown),
        _ => None,
    }
}

/// KeyBinding 1件のパース。key/mods/action のいずれかが不正なら None。
pub fn parse_binding(binding: &KeyBinding) -> Option<(KeyName, ModMask, Action)> {
    Some((
        parse_key(&binding.key)?,
        parse_mods(&binding.mods)?,
        parse_action(&binding.action)?,
    ))
}

/// 解決済みキーマップ（既定バインド + config.keys の上書き）
#[derive(Debug, Clone)]
pub struct Keymap {
    map: HashMap<(KeyName, ModMask), Action>,
}

impl Default for Keymap {
    /// README のタブ操作表どおりの既定バインド
    fn default() -> Self {
        use Action::*;
        use KeyName::*;
        let map = HashMap::from([
            ((F(1), ModMask::NONE), Launcher),
            ((Char('t'), ModMask::CTRL), SidebarFocus),
            ((F(4), ModMask::NONE), CloseTab),
            ((F(5), ModMask::NONE), Reconnect),
            ((F(2), ModMask::NONE), PfPanel),
            // F3 は SFTP に固定割当のため情報パネルは F6。
            ((F(6), ModMask::NONE), InfoPanel),
            ((PageDown, ModMask::CTRL), NextTab),
            ((PageUp, ModMask::CTRL), PrevTab),
            ((Char('c'), ModMask::CTRL_SHIFT), Copy),
            ((Char('v'), ModMask::CTRL_SHIFT), Paste),
            ((Char('f'), ModMask::CTRL_SHIFT), Search),
            ((Char('d'), ModMask::CTRL_SHIFT), Download),
            ((Char('r'), ModMask::CTRL_SHIFT), Reload),
            ((PageUp, ModMask::SHIFT), ScrollPageUp),
            ((PageDown, ModMask::SHIFT), ScrollPageDown),
        ]);
        Keymap { map }
    }
}

impl Keymap {
    /// 既定バインドに config.keys を上書きマージする。不正な行は無視（警告ログ）。
    pub fn from_config(keys: &[KeyBinding]) -> Keymap {
        let mut keymap = Keymap::default();
        for binding in keys {
            match parse_binding(binding) {
                Some((key, mods, action)) => {
                    keymap.map.insert((key, mods), action);
                }
                None => log::warn!(
                    "不正なキーバインドを無視: key={:?} mods={:?} action={:?}",
                    binding.key,
                    binding.mods,
                    binding.action
                ),
            }
        }
        keymap
    }

    /// キー＋修飾に割り当てられたアクションを引く（Char は小文字に正規化して照合）
    pub fn lookup(&self, key: KeyName, mods: ModMask) -> Option<Action> {
        let key = match key {
            KeyName::Char(c) => KeyName::Char(c.to_lowercase().next().unwrap_or(c)),
            other => other,
        };
        self.map.get(&(key, mods)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(key: &str, mods: &str, action: &str) -> KeyBinding {
        KeyBinding {
            key: key.into(),
            mods: mods.into(),
            action: action.into(),
        }
    }

    #[test]
    fn parse_key_variants() {
        assert_eq!(parse_key("t"), Some(KeyName::Char('t')));
        assert_eq!(parse_key("T"), Some(KeyName::Char('t'))); // 小文字正規化
        assert_eq!(parse_key("あ"), Some(KeyName::Char('あ')));
        assert_eq!(parse_key("F1"), Some(KeyName::F(1)));
        assert_eq!(parse_key("f12"), Some(KeyName::F(12)));
        assert_eq!(parse_key("F13"), None);
        assert_eq!(parse_key("F0"), None);
        assert_eq!(parse_key("PageUp"), Some(KeyName::PageUp));
        assert_eq!(parse_key("PAGEDOWN"), Some(KeyName::PageDown));
        assert_eq!(parse_key("Tab"), Some(KeyName::Tab));
        assert_eq!(parse_key("Enter"), Some(KeyName::Enter));
        assert_eq!(parse_key("Esc"), Some(KeyName::Esc));
        assert_eq!(parse_key("Up"), Some(KeyName::Up));
        assert_eq!(parse_key("right"), Some(KeyName::Right));
        assert_eq!(parse_key("nosuchkey"), None);
        assert_eq!(parse_key(""), None);
    }

    #[test]
    fn parse_mods_variants() {
        assert_eq!(parse_mods(""), Some(ModMask::NONE));
        assert_eq!(parse_mods("CTRL"), Some(ModMask::CTRL));
        assert_eq!(parse_mods("ctrl|shift"), Some(ModMask::CTRL_SHIFT)); // 大文字小文字無視
        assert_eq!(parse_mods("CTRL+SHIFT"), Some(ModMask::CTRL_SHIFT)); // + 連結
        assert_eq!(
            parse_mods("CTRL|SHIFT|ALT"),
            Some(ModMask {
                ctrl: true,
                shift: true,
                alt: true
            })
        );
        assert_eq!(parse_mods("SUPER"), None);
    }

    #[test]
    fn parse_binding_full() {
        assert_eq!(
            parse_binding(&binding("j", "CTRL", "next_tab")),
            Some((KeyName::Char('j'), ModMask::CTRL, Action::NextTab))
        );
        assert_eq!(parse_binding(&binding("j", "CTRL", "no_such_action")), None);
        assert_eq!(parse_binding(&binding("", "CTRL", "next_tab")), None);
        assert_eq!(parse_binding(&binding("j", "META", "next_tab")), None);
    }

    #[test]
    fn default_bindings_match_readme() {
        let km = Keymap::default();
        assert_eq!(
            km.lookup(KeyName::F(1), ModMask::NONE),
            Some(Action::Launcher)
        );
        assert_eq!(
            km.lookup(KeyName::Char('t'), ModMask::CTRL),
            Some(Action::SidebarFocus)
        );
        assert_eq!(
            km.lookup(KeyName::F(4), ModMask::NONE),
            Some(Action::CloseTab)
        );
        assert_eq!(
            km.lookup(KeyName::F(5), ModMask::NONE),
            Some(Action::Reconnect)
        );
        assert_eq!(
            km.lookup(KeyName::F(2), ModMask::NONE),
            Some(Action::PfPanel)
        );
        assert_eq!(
            km.lookup(KeyName::F(6), ModMask::NONE),
            Some(Action::InfoPanel)
        );
        assert_eq!(
            km.lookup(KeyName::PageDown, ModMask::CTRL),
            Some(Action::NextTab)
        );
        assert_eq!(
            km.lookup(KeyName::PageUp, ModMask::CTRL),
            Some(Action::PrevTab)
        );
        assert_eq!(
            km.lookup(KeyName::Char('c'), ModMask::CTRL_SHIFT),
            Some(Action::Copy)
        );
        assert_eq!(
            km.lookup(KeyName::Char('v'), ModMask::CTRL_SHIFT),
            Some(Action::Paste)
        );
        assert_eq!(
            km.lookup(KeyName::Char('f'), ModMask::CTRL_SHIFT),
            Some(Action::Search)
        );
        assert_eq!(
            km.lookup(KeyName::Char('d'), ModMask::CTRL_SHIFT),
            Some(Action::Download)
        );
        assert_eq!(
            km.lookup(KeyName::Char('r'), ModMask::CTRL_SHIFT),
            Some(Action::Reload)
        );
        assert_eq!(
            km.lookup(KeyName::PageUp, ModMask::SHIFT),
            Some(Action::ScrollPageUp)
        );
        assert_eq!(
            km.lookup(KeyName::PageDown, ModMask::SHIFT),
            Some(Action::ScrollPageDown)
        );
        // 未割当は None
        assert_eq!(km.lookup(KeyName::Char('c'), ModMask::CTRL), None);
        assert_eq!(km.lookup(KeyName::Enter, ModMask::NONE), None);
    }

    #[test]
    fn config_overrides_and_adds() {
        let km = Keymap::from_config(&[
            binding("j", "CTRL", "next_tab"),       // 追加
            binding("f", "CTRL|SHIFT", "download"), // 既定 search を上書き
            binding("zzz", "CTRL", "next_tab"),     // 不正 → 無視
            binding("k", "CTRL", "no_such"),        // 不正 → 無視
        ]);
        assert_eq!(
            km.lookup(KeyName::Char('j'), ModMask::CTRL),
            Some(Action::NextTab)
        );
        assert_eq!(
            km.lookup(KeyName::Char('f'), ModMask::CTRL_SHIFT),
            Some(Action::Download)
        );
        // 既定は残っている
        assert_eq!(
            km.lookup(KeyName::F(1), ModMask::NONE),
            Some(Action::Launcher)
        );
        assert_eq!(km.lookup(KeyName::Char('k'), ModMask::CTRL), None);
    }

    #[test]
    fn lookup_normalizes_char_case() {
        let km = Keymap::default();
        assert_eq!(
            km.lookup(KeyName::Char('C'), ModMask::CTRL_SHIFT),
            Some(Action::Copy)
        );
    }
}
