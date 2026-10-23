# rigtop

> **開発中です。** まだ使える状態ではありません。

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

## SMART の健康状態（任意）

SMART の読み取りには root が必要です。rigtop 自身は root で動かさず、デバイスの権限も緩めません。代わりに root の systemd タイマーが、全ドライブの `smartctl -j -a` の結果を `/var/lib/rigtop/smart/` に書き出し、rigtop はそのファイルを読みます。スリープ中のディスクは起こしません。

```sh
sudo apt install smartmontools   # ディストリビューションに合わせて
sudo install -m 755 contrib/smart/rigtop-smart-collect /usr/local/libexec/
sudo install -m 644 contrib/smart/rigtop-smart.service contrib/smart/rigtop-smart.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now rigtop-smart.timer
```

結果のファイルは誰でも読める権限で、ドライブのシリアル番号を含みます。

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
```

## ライセンス

[Apache License 2.0](LICENSE-APACHE) と [MIT License](LICENSE-MIT) のどちらかを選んで利用できます。
