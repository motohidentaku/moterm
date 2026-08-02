-- moterm デモ設定（E2E 検証用）。コンテナ内 sshd（127.0.0.1:2222）への接続プロファイル。
local moterm = require 'moterm'
local config = moterm.config()

config.font_size = 16.0
config.color_scheme = 'Tokyo Night'
config.scrollback_lines = 2000
config.lang = 'ja'

config.profiles = {
  {
    name = 'local-demo',
    group = 'demo',
    host = '127.0.0.1',
    port = 2222,
    user = 'moterm',
    auth = { method = 'publickey', key = '/home/moterm/id_ed25519' },
    on_connect = { 'echo moterm-connected' },
  },
  {
    name = 'db',
    group = 'demo',
    host = '127.0.0.1',
    port = 2222,
    user = 'moterm',
    auth = { method = 'password' },
  },
}

config.groups = {
  { name = 'demo', label = 'デモ環境', connect = 'parallel' },
}

-- リモートのシステムメトリクス（右の情報パネル / F6 で開閉）
config.metrics = {
  enabled = true,
  interval_min = 1, -- 採取間隔（分）。0 で停止
  panel = true,     -- 起動時にパネルを開く
}

return config
