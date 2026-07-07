//! mot-gui — winit + softbuffer + fontdue の CPU 描画による moterm GUI。
//! WezTerm 系 crate は使わない。ロジック（色・ペイン木・選択・キー変換）は純関数として
//! テスト可能な形で切り出し、winit 依存は app/main に閉じ込める。

pub mod font;
pub mod input;
pub mod launcherview;
pub mod neo;
pub mod neofont;
pub mod pane;
pub mod render;
pub mod selection;
pub mod sftpview;
pub mod termview;
pub mod theme;

pub mod app;
pub mod runtime;
