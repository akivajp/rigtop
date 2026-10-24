# rigtop

> **開発中です。** Linux の TUI は動きます。GUI と遠隔監視はまだありません。

btop に倣った Linux 向けのシステムモニターです。収集部分を共通にして、2つの画面を用意する予定です。

- **`rigtop`** — ターミナル UI（TUI）
- **`rigtop-gui`** — ネイティブ GUI（WebView 不使用）。btop 風の配置で、各パネルをリサイズできる

[English README](README.md)

## 目標

- btop と同じ一覧性: CPU（全体・コアごと・温度）、全 GPU、メモリ、ストレージ、ネットワーク、プロセス一覧（kill / 強制 kill）を1画面に。
- 既存のモニターが手薄な**ストレージの健康状態**: btrfs の RAID 構成とデバイスごとのエラー回数、mdadm の状態、NVMe の温度、SMART。
- プロセスごとの GPU 使用率と VRAM（NVIDIA、NVML 経由）。
- CPU + GPU の消費電力の合計、ファン回転数（各系統に任意の名前を付けられる）。
- **SSH だけで複数ホストを監視**: `rigtop --agent` が標準入出力で計測値を流すので、常駐プロセスもポート開放も不要。

## ロードマップ

1. 収集部分と TUI（Linux）
2. ネイティブ GUI（Linux）
3. Windows 対応（macOS はビルドのみ、実機検証なし）
4. 登録した複数ホストの SSH 経由の監視

## 使い方

```sh
cargo run --release -p rigtop            # ターミナル UI
cargo run --release -p rigtop -- --help  # オプション（--interval-ms、--config、--dump-json）
```

| キー | 動作 |
|---|---|
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | プロセスを選ぶ |
| `←` `→` / `r` | 並べ替えの列を変える / 逆順にする |
| `/` | 名前・コマンド・ユーザー・pid で絞り込む（`Enter` で確定、`Esc` で解除） |
| `t` | カーネルスレッドの表示を切り替える |
| `k` / `K` | 選んだプロセスに SIGTERM / SIGKILL を送る（送る前に確認する） |
| `n` | 表示するネットワークインターフェースを切り替える |
| `+` / `-` | 更新間隔を長く / 短くする |
| `q` | 終了 |
| マウス | プロセスをクリックで選択、列の見出しをクリックで並べ替え（もう一度で逆順）、ホイールでスクロール |

マウス操作が有効なため、多くの端末では Shift+ドラッグで文字を選択できます。

## SMART と btrfs の scrub 結果（任意）

SMART と btrfs の scrub 結果は root しか読めません。rigtop 自身は root で動かさず、デバイスの権限も緩めません。代わりに root の systemd タイマーが10分ごとに集めて `/var/lib/rigtop/` の下に誰でも読めるファイルとして置き、rigtop はそれを読みます。

- `smart/<デバイス名>.json`: 全ドライブの `smartctl -j -a` の結果（スリープ中のディスクは起こさない）
- `btrfs/scrub.status.<fsid>`: btrfs-progs の scrub 状態ファイルの写し。全データを最後に検査した時刻として表示する

```sh
sudo apt install smartmontools   # ディストリビューションに合わせて
sudo install -m 755 contrib/collect/rigtop-collect /usr/local/libexec/
sudo install -m 644 contrib/collect/rigtop-collect.service contrib/collect/rigtop-collect.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now rigtop-collect.timer
```

SMART のファイルにはドライブのシリアル番号が含まれます。

## 設定

`~/.config/rigtop/config.toml`（Windows は `%APPDATA%\rigtop\config.toml`）。どの項目も省略できます。

```toml
[sensors]
# センサーの各系統の表示名。キーは rigtop が表示する系統のキー
labels = { "nct6686:nct6683.2592/fan1" = "CPU ファン" }
# 隠す系統（未接続で -40 ℃ と出るサーミスタなど）
hidden = ["nct6686:nct6683.2592/temp4"]

[smart]
dir = "/var/lib/rigtop/smart"

[btrfs]
scrub_dir = "/var/lib/rigtop/btrfs"
```

## ライセンス

[Apache License 2.0](LICENSE-APACHE) と [MIT License](LICENSE-MIT) のどちらかを選んで利用できます。
