# moterm 開発・検証イメージ
# ホストにはツールチェーンが無い（make/gcc/sudo 不可）ため、
# ビルド・テスト・GUI 検証（Xvfb）・SSH 統合テスト（sshd）を全てこのイメージ内で行う。
FROM rust:1-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    # ビルト時依存（ring / mlua vendored 等の C コンパイル）
    gcc g++ make pkg-config \
    # winit (X11) の実行時 dlopen 対象
    libx11-6 libx11-xcb1 libxcb1 libxkbcommon0 libxkbcommon-x11-0 \
    libxcursor1 libxrandr2 libxi6 libxext6 \
    # GUI 検証用の仮想ディスプレイとスクリーンショット・入力自動化
    xvfb xauth x11-apps imagemagick xdotool \
    # 端末描画用フォント（CJK 検証込み）
    fonts-dejavu-core fonts-noto-cjk \
    # SSH 統合テスト（サーバ）と proxy_jump 用クライアント
    openssh-server openssh-client \
    # rustfmt/clippy
    && rustup component add rustfmt clippy \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd \
    # xattr 非対応の FS 上でビルドすると openssh-server の postinst が落ち、
    # 特権分離ユーザ sshd が作られないまま half-configured で残る。その状態でも
    # sshd を起動できるよう明示的に作る（既にあれば何もしない）。
    && (id -u sshd >/dev/null 2>&1 \
        || useradd -r -M -d /run/sshd -s /usr/sbin/nologin sshd)

WORKDIR /work
