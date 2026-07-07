//! ホスト一覧のデータ保持。NEO-UI の常設サイドバーが `model`（グループ/ホスト）と
//! `query`（Filter 絞り込み）を参照する。
//! （旧クラシックUIのフルスクリーン・ランチャー描画/入力・編集フォームは廃止した。）

use mot_core::launcher::LauncherModel;
use mot_core::model::{GroupCfg, Profile};

pub struct LauncherState {
    /// サイドバー Filter の入力文字列。
    pub query: String,
    /// グループ/ホストのデータモデル（fuzzy フィルタ含む）。
    pub model: LauncherModel,
}

impl LauncherState {
    pub fn new(profiles: Vec<Profile>, groups: Vec<GroupCfg>) -> Self {
        LauncherState {
            query: String::new(),
            model: LauncherModel::new(profiles, groups),
        }
    }

    /// 設定リロード時にモデルを作り直す（query は維持）。
    pub fn rebuild(&mut self, profiles: Vec<Profile>, groups: Vec<GroupCfg>) {
        self.model = LauncherModel::new(profiles, groups);
    }
}
