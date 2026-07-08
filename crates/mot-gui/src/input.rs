//! winit のキーイベントを mot_term の Key へ変換し、GUI ショートカット判定を行う純ロジック。

use mot_core::keymap::{Action, KeyName, ModMask};
use mot_term::{Key as TermKey, Mods};
use winit::keyboard::{Key as WKey, NamedKey};

/// winit の論理キー＋修飾から、端末へ送るための TermKey を得る。
/// 文字は logical_key の Character を使う（レイアウト反映済み）。
pub fn to_term_key(key: &WKey, text: Option<&str>) -> Option<TermKey> {
    match key {
        WKey::Named(n) => Some(match n {
            NamedKey::Enter => TermKey::Enter,
            NamedKey::Tab => TermKey::Tab,
            NamedKey::Backspace => TermKey::Backspace,
            NamedKey::Escape => TermKey::Escape,
            NamedKey::Delete => TermKey::Delete,
            NamedKey::Insert => TermKey::Insert,
            NamedKey::Home => TermKey::Home,
            NamedKey::End => TermKey::End,
            NamedKey::PageUp => TermKey::PageUp,
            NamedKey::PageDown => TermKey::PageDown,
            NamedKey::ArrowUp => TermKey::Up,
            NamedKey::ArrowDown => TermKey::Down,
            NamedKey::ArrowLeft => TermKey::Left,
            NamedKey::ArrowRight => TermKey::Right,
            NamedKey::Space => TermKey::Char(' '),
            NamedKey::F1 => TermKey::F(1),
            NamedKey::F2 => TermKey::F(2),
            NamedKey::F3 => TermKey::F(3),
            NamedKey::F4 => TermKey::F(4),
            NamedKey::F5 => TermKey::F(5),
            NamedKey::F6 => TermKey::F(6),
            NamedKey::F7 => TermKey::F(7),
            NamedKey::F8 => TermKey::F(8),
            NamedKey::F9 => TermKey::F(9),
            NamedKey::F10 => TermKey::F(10),
            NamedKey::F11 => TermKey::F(11),
            NamedKey::F12 => TermKey::F(12),
            _ => return None,
        }),
        WKey::Character(s) => {
            let ch = s.chars().next()?;
            Some(TermKey::Char(ch))
        }
        _ => {
            // Character が無くても text があれば1文字目を使う
            let ch = text.and_then(|t| t.chars().next())?;
            Some(TermKey::Char(ch))
        }
    }
}

/// GUI 側で消費するアクション（キーバインド解決結果）。keymap.rs の Action と対応。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiAction {
    Launcher,
    /// 左サイドバーのキーボード選択モードに入る（Ctrl+T）。
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
    ScrollPageUp,
    ScrollPageDown,
    Sftp,
    SplitH,
    SplitV,
    ClosePane,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    Broadcast,
    /// Ctrl+1..=9
    TabIndex(u8),
    /// OSC 133 の前のプロンプトへスクロール（Ctrl+Shift+P）
    PromptPrev,
    /// OSC 133 の次のプロンプトへスクロール（Ctrl+Shift+N）
    PromptNext,
}

/// mot_term::Key + Mods を keymap の (KeyName, ModMask) へ変換する（config.keys 照合用）。
/// keymap が扱わないキー（Char 以外の一部）は None。
pub fn to_keymap_key(term_key: &TermKey, mods: Mods) -> Option<(KeyName, ModMask)> {
    let key = match term_key {
        TermKey::Char(c) => KeyName::Char(c.to_lowercase().next().unwrap_or(*c)),
        TermKey::F(n) => KeyName::F(*n),
        TermKey::PageUp => KeyName::PageUp,
        TermKey::PageDown => KeyName::PageDown,
        TermKey::Tab => KeyName::Tab,
        TermKey::Enter => KeyName::Enter,
        TermKey::Escape => KeyName::Esc,
        TermKey::Up => KeyName::Up,
        TermKey::Down => KeyName::Down,
        TermKey::Left => KeyName::Left,
        TermKey::Right => KeyName::Right,
        _ => return None,
    };
    let mask = ModMask {
        ctrl: mods.ctrl,
        shift: mods.shift,
        alt: mods.alt,
    };
    Some((key, mask))
}

/// keymap::Action を GUI アクションへ対応づける。
pub fn action_to_gui(action: Action) -> GuiAction {
    match action {
        Action::Launcher => GuiAction::Launcher,
        Action::SidebarFocus => GuiAction::SidebarFocus,
        Action::NewTab => GuiAction::NewTab,
        Action::NextTab => GuiAction::NextTab,
        Action::PrevTab => GuiAction::PrevTab,
        Action::CloseTab => GuiAction::CloseTab,
        Action::Reconnect => GuiAction::Reconnect,
        Action::Copy => GuiAction::Copy,
        Action::Paste => GuiAction::Paste,
        Action::Search => GuiAction::Search,
        Action::Download => GuiAction::Download,
        Action::Reload => GuiAction::Reload,
        Action::PfPanel => GuiAction::PfPanel,
        Action::ScrollPageUp => GuiAction::ScrollPageUp,
        Action::ScrollPageDown => GuiAction::ScrollPageDown,
    }
}

/// 端末モードでの固定ショートカット（config.keys で上書きしない分割・フォーカス等）。
/// README のキー表に対応。戻り値 Some なら GUI が消費し PTY へは送らない。
/// config.keys で扱える14アクションは keymap 側で先に解決されるため、ここには**含めない**
/// （分割/フォーカス/ブロードキャスト/Ctrl+Tab/Ctrl+数字/F3 SFTP のみ）。
pub fn fixed_shortcut(term_key: &TermKey, mods: Mods) -> Option<GuiAction> {
    let ctrl = mods.ctrl;
    let shift = mods.shift;
    match term_key {
        TermKey::Char(c) if ctrl && shift => match c.to_ascii_lowercase() {
            'e' => Some(GuiAction::SplitH),
            'o' => Some(GuiAction::SplitV),
            'x' => Some(GuiAction::ClosePane),
            'b' => Some(GuiAction::Broadcast),
            // OSC 133 プロンプトジャンプ（Ctrl+Shift+矢印はペインフォーカスで使用済みのため P/N）
            'p' => Some(GuiAction::PromptPrev),
            'n' => Some(GuiAction::PromptNext),
            _ => None,
        },
        TermKey::Left if ctrl && shift => Some(GuiAction::FocusLeft),
        TermKey::Right if ctrl && shift => Some(GuiAction::FocusRight),
        TermKey::Up if ctrl && shift => Some(GuiAction::FocusUp),
        TermKey::Down if ctrl && shift => Some(GuiAction::FocusDown),
        TermKey::Tab if ctrl && !shift => Some(GuiAction::NextTab),
        TermKey::Tab if ctrl && shift => Some(GuiAction::PrevTab),
        TermKey::F(3) => Some(GuiAction::Sftp),
        TermKey::Char('t') if ctrl && !shift => Some(GuiAction::SidebarFocus),
        TermKey::Char(c) if ctrl && !shift && c.is_ascii_digit() && *c != '0' => {
            Some(GuiAction::TabIndex(*c as u8 - b'0'))
        }
        _ => None,
    }
}

/// 端末モードでの固定ショートカット（keymap 非対応分 + 既定14アクション）。
/// テスト・後方互換のために既定割当を1関数で返す。GUI 本体は keymap→fixed の順で解決する。
pub fn terminal_shortcut(term_key: &TermKey, mods: Mods) -> Option<GuiAction> {
    let ctrl = mods.ctrl;
    let shift = mods.shift;
    match term_key {
        // ペイン分割・フォーカス（Ctrl+Shift+…）
        TermKey::Char(c) if ctrl && shift => match c.to_ascii_lowercase() {
            'e' => Some(GuiAction::SplitH),
            'o' => Some(GuiAction::SplitV),
            'x' => Some(GuiAction::ClosePane),
            'b' => Some(GuiAction::Broadcast),
            'c' => Some(GuiAction::Copy),
            'v' => Some(GuiAction::Paste),
            'f' => Some(GuiAction::Search),
            'd' => Some(GuiAction::Download),
            'r' => Some(GuiAction::Reload),
            _ => None,
        },
        TermKey::Left if ctrl && shift => Some(GuiAction::FocusLeft),
        TermKey::Right if ctrl && shift => Some(GuiAction::FocusRight),
        TermKey::Up if ctrl && shift => Some(GuiAction::FocusUp),
        TermKey::Down if ctrl && shift => Some(GuiAction::FocusDown),
        // タブ操作
        TermKey::PageDown if ctrl => Some(GuiAction::NextTab),
        TermKey::PageUp if ctrl && !shift => Some(GuiAction::PrevTab),
        TermKey::Tab if ctrl && !shift => Some(GuiAction::NextTab),
        TermKey::Tab if ctrl && shift => Some(GuiAction::PrevTab),
        TermKey::PageUp if shift && !ctrl => Some(GuiAction::ScrollPageUp),
        TermKey::PageDown if shift && !ctrl => Some(GuiAction::ScrollPageDown),
        // ファンクション
        TermKey::F(1) => Some(GuiAction::Launcher),
        TermKey::F(2) => Some(GuiAction::PfPanel),
        TermKey::F(3) => Some(GuiAction::Sftp),
        TermKey::F(4) => Some(GuiAction::CloseTab),
        TermKey::F(5) => Some(GuiAction::Reconnect),
        TermKey::Char('t') if ctrl && !shift => Some(GuiAction::SidebarFocus),
        // Ctrl+1..=9
        TermKey::Char(c) if ctrl && !shift && c.is_ascii_digit() && *c != '0' => {
            Some(GuiAction::TabIndex(*c as u8 - b'0'))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::NamedKey;

    fn m(ctrl: bool, shift: bool) -> Mods {
        Mods {
            ctrl,
            shift,
            alt: false,
        }
    }

    #[test]
    fn named_and_char_translation() {
        assert_eq!(
            to_term_key(&WKey::Named(NamedKey::Enter), None),
            Some(TermKey::Enter)
        );
        assert_eq!(
            to_term_key(&WKey::Named(NamedKey::ArrowUp), None),
            Some(TermKey::Up)
        );
        assert_eq!(
            to_term_key(&WKey::Named(NamedKey::F5), None),
            Some(TermKey::F(5))
        );
        assert_eq!(
            to_term_key(&WKey::Character("a".into()), None),
            Some(TermKey::Char('a'))
        );
    }

    #[test]
    fn split_and_focus_shortcuts() {
        assert_eq!(
            terminal_shortcut(&TermKey::Char('e'), m(true, true)),
            Some(GuiAction::SplitH)
        );
        assert_eq!(
            terminal_shortcut(&TermKey::Char('O'), m(true, true)),
            Some(GuiAction::SplitV)
        );
        assert_eq!(
            terminal_shortcut(&TermKey::Char('x'), m(true, true)),
            Some(GuiAction::ClosePane)
        );
        assert_eq!(
            terminal_shortcut(&TermKey::Left, m(true, true)),
            Some(GuiAction::FocusLeft)
        );
        assert_eq!(
            terminal_shortcut(&TermKey::Char('b'), m(true, true)),
            Some(GuiAction::Broadcast)
        );
    }

    #[test]
    fn prompt_jump_shortcuts() {
        assert_eq!(
            fixed_shortcut(&TermKey::Char('p'), m(true, true)),
            Some(GuiAction::PromptPrev)
        );
        assert_eq!(
            fixed_shortcut(&TermKey::Char('N'), m(true, true)),
            Some(GuiAction::PromptNext)
        );
        // 修飾なしは PTY へ（None）
        assert_eq!(fixed_shortcut(&TermKey::Char('p'), m(false, false)), None);
    }

    #[test]
    fn tab_shortcuts() {
        assert_eq!(
            terminal_shortcut(&TermKey::PageDown, m(true, false)),
            Some(GuiAction::NextTab)
        );
        assert_eq!(
            terminal_shortcut(&TermKey::Tab, m(true, false)),
            Some(GuiAction::NextTab)
        );
        assert_eq!(
            terminal_shortcut(&TermKey::Tab, m(true, true)),
            Some(GuiAction::PrevTab)
        );
        assert_eq!(
            terminal_shortcut(&TermKey::Char('3'), m(true, false)),
            Some(GuiAction::TabIndex(3))
        );
        assert_eq!(
            terminal_shortcut(&TermKey::F(1), m(false, false)),
            Some(GuiAction::Launcher)
        );
    }

    #[test]
    fn copy_paste_and_scroll() {
        assert_eq!(
            terminal_shortcut(&TermKey::Char('c'), m(true, true)),
            Some(GuiAction::Copy)
        );
        assert_eq!(
            terminal_shortcut(&TermKey::Char('v'), m(true, true)),
            Some(GuiAction::Paste)
        );
        assert_eq!(
            terminal_shortcut(&TermKey::PageUp, m(false, true)),
            Some(GuiAction::ScrollPageUp)
        );
    }

    #[test]
    fn plain_key_not_shortcut() {
        assert_eq!(
            terminal_shortcut(&TermKey::Char('a'), m(false, false)),
            None
        );
        assert_eq!(terminal_shortcut(&TermKey::Enter, m(false, false)), None);
    }

    #[test]
    fn keymap_default_matches_terminal_shortcut() {
        // 既定 Keymap + fixed_shortcut の合成が、旧 terminal_shortcut と同じ結果になること。
        use mot_core::keymap::Keymap;
        let km = Keymap::default();
        let resolve = |tk: TermKey, mods: Mods| -> Option<GuiAction> {
            if let Some((k, mm)) = to_keymap_key(&tk, mods) {
                if let Some(a) = km.lookup(k, mm) {
                    return Some(action_to_gui(a));
                }
            }
            fixed_shortcut(&tk, mods)
        };
        let cases = [
            (TermKey::F(1), m(false, false)),
            (TermKey::Char('t'), m(true, false)),
            (TermKey::F(4), m(false, false)),
            (TermKey::F(5), m(false, false)),
            (TermKey::F(2), m(false, false)),
            (TermKey::F(3), m(false, false)),
            (TermKey::PageDown, m(true, false)),
            (TermKey::PageUp, m(true, false)),
            (TermKey::Char('c'), m(true, true)),
            (TermKey::Char('v'), m(true, true)),
            (TermKey::Char('f'), m(true, true)),
            (TermKey::Char('d'), m(true, true)),
            (TermKey::Char('r'), m(true, true)),
            (TermKey::PageUp, m(false, true)),
            (TermKey::PageDown, m(false, true)),
            (TermKey::Char('e'), m(true, true)),
            (TermKey::Char('o'), m(true, true)),
            (TermKey::Char('x'), m(true, true)),
            (TermKey::Char('b'), m(true, true)),
            (TermKey::Left, m(true, true)),
            (TermKey::Tab, m(true, false)),
            (TermKey::Tab, m(true, true)),
            (TermKey::Char('3'), m(true, false)),
        ];
        for (tk, mods) in cases {
            assert_eq!(
                resolve(tk, mods),
                terminal_shortcut(&tk, mods),
                "mismatch for {tk:?} {mods:?}"
            );
        }
    }

    #[test]
    fn config_keys_override() {
        // config.keys で next_tab を Ctrl+j に割当てると keymap 経由で解決される。
        use mot_core::keymap::Keymap;
        use mot_core::model::KeyBinding;
        let km = Keymap::from_config(&[KeyBinding {
            key: "j".into(),
            mods: "CTRL".into(),
            action: "next_tab".into(),
        }]);
        let (k, mm) = to_keymap_key(&TermKey::Char('j'), m(true, false)).unwrap();
        assert_eq!(
            km.lookup(k, mm).map(action_to_gui),
            Some(GuiAction::NextTab)
        );
    }
}
