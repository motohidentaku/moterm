//! ランチャー（接続先選択 UI）の検索・グルーピングモデル。UI 非依存。
//!
//! 仕様の正: moterm/docs/design.md §4.1・README「キーボード操作/ランチャー」。
//! 左ペイン = "All" + グループ一覧（ホスト数付き）、右ペイン = 選択グループのホスト一覧。
//! インクリメンタル検索は fuzzy（サブシーケンス）マッチ。

use crate::model::{GroupCfg, Profile};

/// 左ペイン先頭の擬似グループ名（全ホスト）
pub const ALL_GROUP: &str = "All";

/// 大文字小文字無視のサブシーケンスマッチ。マッチしたらスコア（大きいほど良い）。
/// 連続一致・先頭一致にボーナス。日本語も chars() 単位でそのまま扱う。
/// 空クエリは常に Some(0)。
pub fn fuzzy_match(query: &str, text: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut score: i64 = 0;
    let mut qi = 0usize;
    let mut last_hit: Option<usize> = None;
    for (ti, &c) in t.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            score += 1;
            if ti == 0 {
                score += 3; // 先頭一致ボーナス
            }
            if let Some(prev) = last_hit {
                if ti == prev + 1 {
                    score += 2; // 連続一致ボーナス
                }
            }
            last_hit = Some(ti);
            qi += 1;
        }
    }
    if qi == q.len() {
        Some(score)
    } else {
        None
    }
}

/// 左ペインの1行（グループ）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupEntry {
    /// グループ名（"All" を含む）
    pub name: String,
    /// 表示名（config.groups[].label があればそれ、無ければ name）
    pub label: String,
    /// 属するホスト数
    pub count: usize,
}

/// ランチャーの状態モデル（プロファイル一覧＋グループ構成）
#[derive(Debug, Clone)]
pub struct LauncherModel {
    profiles: Vec<Profile>,
    groups_cfg: Vec<GroupCfg>,
    groups: Vec<GroupEntry>,
}

impl LauncherModel {
    /// profiles（マージ済み一覧）と config.groups からモデルを構築する。
    /// グループは profiles 中の group 出現順。label は config.groups を反映。
    pub fn new(profiles: Vec<Profile>, groups_cfg: Vec<GroupCfg>) -> Self {
        let mut names: Vec<&str> = Vec::new();
        for p in &profiles {
            if let Some(g) = p.group.as_deref() {
                if !names.contains(&g) {
                    names.push(g);
                }
            }
        }
        let mut groups = vec![GroupEntry {
            name: ALL_GROUP.into(),
            label: ALL_GROUP.into(),
            count: profiles.len(),
        }];
        for name in names {
            let label = groups_cfg
                .iter()
                .find(|c| c.name == name)
                .and_then(|c| c.label.clone())
                .unwrap_or_else(|| name.to_string());
            let count = profiles
                .iter()
                .filter(|p| p.group.as_deref() == Some(name))
                .count();
            groups.push(GroupEntry {
                name: name.to_string(),
                label,
                count,
            });
        }
        LauncherModel {
            profiles,
            groups_cfg,
            groups,
        }
    }

    /// 左ペイン: "All" + グループ一覧（ホスト数付き）
    pub fn groups(&self) -> &[GroupEntry] {
        &self.groups
    }

    /// 全プロファイル（定義順）
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    /// 右ペイン: 選択グループのホスト一覧。
    /// None または "All" は全ホスト。config.groups[].order 指定時は
    /// order に載っている名前を先頭にその順で、残りは定義順で続ける。
    pub fn hosts(&self, group: Option<&str>) -> Vec<&Profile> {
        let name = match group {
            None => return self.profiles.iter().collect(),
            Some(g) if g == ALL_GROUP => return self.profiles.iter().collect(),
            Some(g) => g,
        };
        let mut list: Vec<&Profile> = self
            .profiles
            .iter()
            .filter(|p| p.group.as_deref() == Some(name))
            .collect();
        if let Some(order) = self
            .groups_cfg
            .iter()
            .find(|c| c.name == name)
            .and_then(|c| c.order.as_ref())
        {
            // 安定ソート: order 掲載分はその位置、非掲載分は末尾（元の相対順を維持）
            list.sort_by_key(|p| {
                order
                    .iter()
                    .position(|n| *n == p.name)
                    .unwrap_or(usize::MAX)
            });
        }
        list
    }

    /// fuzzy 絞り込み。name/host/group を対象に最高スコアで評価し、スコア降順
    /// （同点は定義順）で返す。空クエリは全件。
    pub fn filter(&self, query: &str) -> Vec<&Profile> {
        let mut scored: Vec<(i64, usize, &Profile)> = self
            .profiles
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let best = [
                    Some(p.name.as_str()),
                    Some(p.host.as_str()),
                    p.group.as_deref(),
                ]
                .into_iter()
                .flatten()
                .filter_map(|t| fuzzy_match(query, t))
                .max()?;
                Some((best, i, p))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, _, p)| p).collect()
    }
}

/// クイック接続入力 `[user@]host[:port]` のパース。
/// 空・不正（空ホスト・不正ポート等）は None。IPv6 角括弧は非対応（moterm 同様）。
pub fn parse_quick_connect(input: &str) -> Option<(Option<String>, String, u16)> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let (user, rest) = match s.split_once('@') {
        Some((u, r)) => {
            if u.is_empty() {
                return None;
            }
            (Some(u.to_string()), r)
        }
        None => (None, s),
    };
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok().filter(|&n| n != 0)?),
        None => (rest, 22),
    };
    if host.is_empty() || host.contains('@') || host.contains(char::is_whitespace) {
        return None;
    }
    Some((user, host.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, host: &str, group: Option<&str>) -> Profile {
        Profile {
            name: name.into(),
            host: host.into(),
            group: group.map(Into::into),
            port: 22,
            ..Profile::default()
        }
    }

    fn model() -> LauncherModel {
        LauncherModel::new(
            vec![
                profile("web", "10.0.0.10", Some("production")),
                profile("db", "10.0.0.20", Some("production")),
                profile("stg-web", "10.1.0.10", Some("staging")),
                profile("laptop", "192.168.1.2", None),
            ],
            vec![GroupCfg {
                name: "production".into(),
                label: Some("本番環境".into()),
                connect: "parallel".into(),
                order: Some(vec!["db".into(), "web".into()]),
            }],
        )
    }

    // ---- fuzzy_match ----

    #[test]
    fn fuzzy_basic() {
        assert!(fuzzy_match("web", "web").is_some());
        assert!(fuzzy_match("wb", "web").is_some()); // サブシーケンス
        assert!(fuzzy_match("WEB", "web").is_some()); // 大文字小文字無視
        assert!(fuzzy_match("webx", "web").is_none());
        assert!(fuzzy_match("x", "web").is_none());
        assert_eq!(fuzzy_match("", "anything"), Some(0)); // 空クエリ
    }

    #[test]
    fn fuzzy_scoring_prefers_consecutive_and_prefix() {
        // 先頭からの連続一致 > 途中の連続一致 > 飛び飛び
        let prefix = fuzzy_match("web", "web-server").unwrap();
        let middle = fuzzy_match("web", "my-web").unwrap();
        let sparse = fuzzy_match("web", "w1e2b3").unwrap();
        assert!(prefix > middle, "prefix={prefix} middle={middle}");
        assert!(middle > sparse, "middle={middle} sparse={sparse}");
    }

    #[test]
    fn fuzzy_japanese() {
        assert!(fuzzy_match("本番", "本番環境").is_some());
        assert!(fuzzy_match("本境", "本番環境").is_some());
        assert!(fuzzy_match("開発", "本番環境").is_none());
    }

    // ---- LauncherModel ----

    #[test]
    fn groups_are_all_plus_appearance_order_with_counts_and_labels() {
        let m = model();
        let g = m.groups();
        assert_eq!(g.len(), 3);
        assert_eq!((g[0].name.as_str(), g[0].count), ("All", 4));
        assert_eq!((g[1].name.as_str(), g[1].count), ("production", 2));
        assert_eq!(g[1].label, "本番環境"); // config.groups の label 反映
        assert_eq!((g[2].name.as_str(), g[2].count), ("staging", 1));
        assert_eq!(g[2].label, "staging"); // label 省略時は name
    }

    #[test]
    fn hosts_respects_group_selection_and_order() {
        let m = model();
        assert_eq!(m.hosts(Some("All")).len(), 4);
        assert_eq!(m.hosts(None).len(), 4);
        // order = { db, web } を反映
        let prod: Vec<&str> = m
            .hosts(Some("production"))
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(prod, ["db", "web"]);
        // order の無いグループは定義順
        let stg: Vec<&str> = m
            .hosts(Some("staging"))
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(stg, ["stg-web"]);
        assert!(m.hosts(Some("nope")).is_empty());
    }

    #[test]
    fn hosts_order_keeps_unlisted_last() {
        let m = LauncherModel::new(
            vec![
                profile("a", "h1", Some("g")),
                profile("b", "h2", Some("g")),
                profile("c", "h3", Some("g")),
            ],
            vec![GroupCfg {
                name: "g".into(),
                label: None,
                connect: "parallel".into(),
                order: Some(vec!["c".into()]),
            }],
        );
        let names: Vec<&str> = m.hosts(Some("g")).iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["c", "a", "b"]);
    }

    #[test]
    fn filter_matches_name_host_and_group() {
        let m = model();
        // name
        let by_name: Vec<&str> = m.filter("laptop").iter().map(|p| p.name.as_str()).collect();
        assert_eq!(by_name, ["laptop"]);
        // host
        let by_host: Vec<&str> = m
            .filter("192.168")
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(by_host, ["laptop"]);
        // group
        assert_eq!(m.filter("production").len(), 2);
        // 空クエリ = 全件（定義順）
        assert_eq!(m.filter("").len(), 4);
        // 不一致
        assert!(m.filter("zzz").is_empty());
    }

    #[test]
    fn filter_sorts_by_score_desc() {
        let m = model();
        // "web" は name="web"（完全一致・先頭）が name="stg-web"（途中一致）より先
        let names: Vec<&str> = m.filter("web").iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["web", "stg-web"]);
    }

    // ---- parse_quick_connect ----

    #[test]
    fn quick_connect_forms() {
        assert_eq!(parse_quick_connect("host"), Some((None, "host".into(), 22)));
        assert_eq!(
            parse_quick_connect("host:2222"),
            Some((None, "host".into(), 2222))
        );
        assert_eq!(
            parse_quick_connect("user@host"),
            Some((Some("user".into()), "host".into(), 22))
        );
        assert_eq!(
            parse_quick_connect(" user@10.0.0.1:2022 "),
            Some((Some("user".into()), "10.0.0.1".into(), 2022))
        );
    }

    #[test]
    fn quick_connect_rejects_invalid() {
        assert_eq!(parse_quick_connect(""), None);
        assert_eq!(parse_quick_connect("   "), None);
        assert_eq!(parse_quick_connect("@host"), None); // 空ユーザ
        assert_eq!(parse_quick_connect("user@"), None); // 空ホスト
        assert_eq!(parse_quick_connect("host:"), None); // 空ポート
        assert_eq!(parse_quick_connect("host:abc"), None);
        assert_eq!(parse_quick_connect("host:0"), None);
        assert_eq!(parse_quick_connect("host:70000"), None); // u16 範囲外
        assert_eq!(parse_quick_connect("h:1:2"), None); // 多重コロン
        assert_eq!(parse_quick_connect("a@b@c"), None);
        assert_eq!(parse_quick_connect("ho st"), None);
    }
}
