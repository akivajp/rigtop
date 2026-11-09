# Microsoft Store の申請内容

Partner Center（日本語の画面）で rigtop を申請するときに入れる内容です（アプリとゲーム → rigtop → 申請の開始）。Markdown を表示した状態からコピーして、欄に貼っていく想定で書いています。1 つずつ欄に入れるもの（製品の機能、検索用語、キャプション）はリストにしています。掲載情報の文は README と内容をそろえてください。

Store 版は zip 版と次の点が違う前提で書いています: 新しい版の確認と自分の更新をしない（Store が更新を配る）、LibreHardwareMonitor を入れる「set up…」は手順の案内だけ、設定ファイルはパッケージの LocalState に置く、GUI はスタートメニューから、端末版は端末で `rigtop` として起動できる（実行エイリアス。Store はパッケージに 1 つのアプリケーションしか認めず、エイリアスも 1 つまでのため、`rigtop-gui` のコマンドは無い）。

## 価格と使用可能状況

- **市場**: すべての市場
- **表示**: 一般公開。Store で見つけられるようにする
- **スケジュール**: できるだけ早く（実際の公開は、下の「申請オプション」の公開の保留で決める）
- **価格**: 無料。試用版なし、セールなし

## プロパティ

- **カテゴリ**: ユーティリティ & ツール
- **プライバシー ポリシー**: 「個人情報を使用する」を選び、次の URL を入れる。rigtop は何も外へ送らないが、プロセスの一覧（コマンドライン・ユーザー名）を表示し、利用者が設定した遠隔ホストに ssh でつなぐため。README のプライバシーの節に、接続先を Store 版の動きに合わせて書いてある
  - https://github.com/akivajp/rigtop#privacy
- **Web サイト**: https://github.com/akivajp/rigtop
- **サポートの連絡先情報**: https://github.com/akivajp/rigtop/issues
- **製品の公表**: デフォルトのまま
  - オフ: Store の商取引を使わない購入（購入の機能は無い）、アクセシビリティのテスト済み（Store のチェックリストで確かめていない）、ペンとインク、生成 AI の機能（アプリの機能として無い。開発に AI を使ったことは関係ない）
  - オン: 代替ドライブへのインストール、OneDrive へのバックアップ（設定ファイルと警告の履歴だけ）、録画とブロードキャスト（ゲーム向けで影響なし）
- **システム要件**: Windows 11 のほかは無し（NVIDIA の GPU の値はドライバーの `nvml.dll` から読む。温度・ファン・CPU の電力は LibreHardwareMonitor が要る。下の掲載情報の冒頭に書く）

## 年齢別レーティング

- **アンケート**: 「International Age Rating Coalition (IARC) のアンケートへ回答する準備ができています」
- **アプリの種類**: 「その他のすべてのアプリの種類」（例にユーティリティ・ツールが入っている）
- **評価機関から直接取得した評価を使用するか、物理メディアで配布するか**: いいえ
- **その後の質問**: すべて「いいえ」（暴力・性的な表現・言葉づかい・ギャンブル・薬物なし、利用者どうしのやり取りや共有なし、位置情報の共有なし、デジタルの購入なし、自由な Web の閲覧なし）。結果はどの地域でも一番低い区分（IARC 3+）になるはず

## パッケージ

- リリースのワークフローの成果物 `store-msix` の `rigtop-vX.Y.Z-x64.msix` を上げる（`-test` の付いたものではない）
- デバイス ファミリ: Windows 10/11 デスクトップ（マニフェストでは Windows 11 以上を指定）
- パッケージの言語は英語（en-us）だけ（アプリの画面が英語のみのため。ポリシー 10.7 は、宣言した言語にアプリを対応させることを求めている）。日本語の掲載情報は「その他のストア登録情報の言語」として足す（下）

## 申請オプション

- **公開の保留オプション**: 初回は「[今すぐ公開] を選択するまで、この提出物を公開しない」。告知の準備ができてから公開する
- **認定の注意書き**: この画面ではなく、リンク先の「追加のテスト情報」のページに、次の英語の文を貼る。ポリシー 10.2.4（Microsoft 以外のドライバーへの依存は認定の注意書きで開示）に当たりうるので、LibreHardwareMonitor のことも書いている

  > rigtop opens a window showing this PC: CPU, GPUs, memory, drives, network and processes. No account, sign-in or test data is needed. The package also has the terminal version: `rigtop` in a terminal (app execution alias). To see the multi-host features without other machines, run `rigtop --demo` in a terminal: it shows three made-up hosts.
  >
  > Optional: temperatures, fans and CPU power are read from LibreHardwareMonitor's local web server (127.0.0.1:8085) if the user installs and runs that separate program, which uses its own driver. rigtop does not install it or any driver, and works without it; the CPU panel then shows a note and a link to instructions.
  >
  > Remote hosts are monitored by running the user's own ssh.exe. When the user asks, rigtop can update rigtop on a remote host: that host downloads the official release from GitHub. Nothing is downloaded or run on this PC.

- **制限付き機能**（`runFullTrust` の理由。**500 文字まで**）: 次の英語の文を貼る

  > rigtop is a desktop (Win32) system monitor. It needs full trust to read system-wide CPU, memory, disk, network and process information, to end or close processes the user chooses (TerminateProcess, WM_CLOSE), to read NVIDIA GPUs through the driver's nvml.dll, to run the user's OpenSSH client (ssh.exe) for remote hosts and the commands the user sets up as widgets, and to read LibreHardwareMonitor or a UPS server on this PC if the user runs one.

## ストア登録情報

**英語（米国）** はパッケージの言語なので最初からある。**日本語** は「言語の追加と削除」→「その他のストア登録情報の言語」の「その他の言語を管理」で足す。

スクリーンショット（PNG、1600×900）は手元の `tmp/store/shots/`（コミットしない）にある。`--demo` の架空の 3 台（ワークステーション・ノート PC・NAS）を、実機の代わりにオフスクリーンで描いた（`tmp/store/shoot.sh`、README の画像と同じ方法）。アプリの画面が英語だけなので、**英語と日本語で同じ画像**を使い、キャプションだけ言語ごとに変える。番号の順に登録する。

| 順 | ファイル | 内容 |
| --- | --- | --- |
| 1 | `1-overview.png` | 3 台のオーバービュー |
| 2 | `2-workstation.png` | ワークステーションの詳細（コア・GPU 2 枚・ストレージ・プロセスのツリー） |
| 3 | `3-storage.png` | NAS（SMART の警告・RAID の再構築・ZFS・UPS） |
| 4 | `4-alerts.png` | 警告の一覧と履歴、スヌーズ |
| 5 | `5-windows.png` | **任意・未作成**。Windows で動いている画面。共有の仮想マシンでは GPU・温度が出ず、ソフトウェア描画で rigtop-gui 自身が CPU 300% と出るため撮っていない。入れるなら、実機の Windows で Store 版を表示して撮る（プロセス一覧は `/top` などで絞り、利用者名・ホスト名・他社のアプリ名が写らないようにする）。後の申請で足してもよい |

キャプション（「イメージの字幕の追加」、200 文字まで）は、各言語の掲載情報の節にある。

Microsoft Store ロゴ（任意）:

- **1:1 のボックス アート**: 登録をおすすめ（Store のいろいろな一覧で使われる）。`tmp/store/art/store-box-2160.png`（`tmp/store/make_store_art.py` でアイコンの SVG から作成）
- **9:16 のポスター アート**: 必要なのは Xbox だけ。入れるなら `tmp/store/art/store-poster-1440x2160.png`
- **タイル アイコン（300 / 150 / 71）**: 空のまま（パッケージのアイコンが使われる）

### English (United States)

**Product name**: rigtop

**Short description**:

> A system monitor for your PC and the other machines you look after: CPU, every GPU, memory, drives, network and processes, drive health and RAID, and many hosts over SSH on one screen.

**Description**:

> On Windows, temperatures, fans and CPU power come from LibreHardwareMonitor, a free program installed and run separately. Without it, rigtop shows everything else.
>
> rigtop shows how your PC is doing, and the workstations, laptops, home servers and NAS you look after, side by side: CPU load per core, every GPU, memory, drives, network and the processes. It also watches what other monitors mostly skip: drive health and RAID.
>
> On this PC:
> • CPU load per core and clock; temperatures, fans and power through LibreHardwareMonitor
> • NVIDIA GPUs: load, video memory, temperature and power, and each process's GPU use and video memory
> • Memory, drives with their I/O and health (wear, spare, temperature), network with link speeds and addresses
> • Processes with search, a tree view, details, end task and kill
>
> Other machines over SSH:
> • Add Linux, macOS or Windows hosts that have rigtop installed. rigtop runs it there through the OpenSSH client built into Windows, with your SSH keys: no server to run, no port to open.
> • An overview with a card per host, alerts first if you like
> • From Linux hosts: SMART, mdadm RAID with resync progress, ZFS pools and scrubs, btrfs errors, the container or virtual machine of each process, AMD and Intel GPUs, and UPSes through Network UPS Tools
> • When you ask, rigtop updates rigtop on a remote host to its own version: that host downloads the official release.
>
> Alerts for failing drives, degraded RAID, nearly full disks, hot CPUs, GPUs and drives, and memory running out, with a history kept between runs. Snooze one for an hour, a day, or until it clears; set the thresholds per host.
>
> Also: panels you can resize, rearrange and hide; graphs from one minute to 24 hours; zoom; widgets that show the output of your own commands; and a terminal version, "rigtop".
>
> rigtop sends no telemetry. The app is in English. It is free and open source (MIT or Apache 2.0).

**Screenshot captions** (in the order of the screenshots):

- Every machine you look after on one screen, with what needs attention marked
- A workstation over SSH: CPU cores and temperatures, two GPUs, drive health and processes with their GPU use
- A NAS over SSH: a drive failing SMART, a RAID resync, a ZFS pool and the UPS
- Alerts now and their history; snooze one for an hour, a day, or until it clears
- （5 枚目を入れるとき）rigtop on Windows: CPU cores, temperatures and fans through LibreHardwareMonitor, the GPU and processes

**What's new in this version**: 空欄のまま（Partner Center の説明では、初回の申請では空にする）

**Product features** (one per line):

- CPU load per core, clock, temperatures, fans and power
- Every GPU with each process's GPU use and video memory (NVIDIA)
- Memory, drives with I/O and health, network with link speeds
- Processes with search, tree view, details, end task and kill
- Many hosts over SSH with an overview, no server or open port
- Drive health from Linux hosts: SMART, mdadm, ZFS and btrfs
- Containers and virtual machines per process on Linux hosts
- UPS state through Network UPS Tools
- Alerts with a history and snooze; thresholds per host
- Updates rigtop on remote hosts when you ask
- Resizable, rearrangeable panels; graphs up to 24 hours
- Widgets that show the output of your own commands
- A terminal version, rigtop
- Free and open source

**Search terms** (7 at most, one per box):

- system monitor
- resource monitor
- hardware monitor
- GPU monitor
- server monitoring
- SSH
- RAID

**Copyright and trademark info**: Copyright (c) 2026 Akiva Miura

**Additional license terms**: MIT License or Apache License 2.0, at your option: https://github.com/akivajp/rigtop#license

**Developed by**: Akiva Miura

### Japanese

**製品名**: rigtop

**短い説明**:

> この PC と、管理しているほかのマシンをまとめて見るシステムモニター。CPU・すべての GPU・メモリ・ドライブ・ネットワーク・プロセスに、ドライブの健康状態と RAID、SSH でつないだ複数のホストを 1 画面で。

**説明**:

> Windows では、温度・ファン・CPU の電力は、別に入れて動かす無料のソフト LibreHardwareMonitor から読みます。無くてもそれ以外はすべて表示します。
>
> rigtop は、この PC と、管理しているワークステーション・ノート PC・自宅のサーバー・NAS の様子を並べて表示します。コアごとの CPU の負荷、すべての GPU、メモリ、ドライブ、ネットワーク、プロセスに加えて、ほかのモニターではあまり見られないドライブの健康状態と RAID も見張ります。
>
> この PC では:
> • コアごとの CPU の負荷とクロック。温度・ファン・電力は LibreHardwareMonitor から
> • NVIDIA の GPU: 負荷・ビデオメモリ・温度・電力と、プロセスごとの GPU の使用率とビデオメモリ
> • メモリ、ドライブの読み書きと健康状態（摩耗・予備領域・温度）、ネットワークのリンク速度とアドレス
> • プロセスの検索・ツリー表示・詳細・タスクの終了・強制終了
>
> SSH でほかのマシンも:
> • rigtop を入れた Linux・macOS・Windows のホストを追加できます。Windows 標準の OpenSSH クライアントと手元の SSH の鍵で、相手のマシンの rigtop を動かします。常駐のサーバーも、ポートの開放も要りません。
> • ホストごとのカードを並べたオーバービュー。注意が要るホストを先頭にすることも
> • Linux のホストからは: SMART、mdadm の RAID と再構築の進み具合、ZFS のプールとスクラブ、btrfs のエラー、プロセスごとのコンテナーや仮想マシン、AMD・Intel の GPU、Network UPS Tools 経由の UPS
> • 指示すると、遠隔ホストの rigtop を手元と同じ版に更新します（そのホストが公式のリリースをダウンロードします）
>
> ドライブの故障の兆候、RAID の縮退、残り少ないディスク、CPU・GPU・ドライブの高温、メモリ不足を警告し、履歴も残します。1 時間・1 日・解消するまで黙らせることができ、しきい値はホストごとに設定できます。
>
> そのほか: パネルの大きさ・配置・表示を変えられる、1 分から 24 時間までのグラフ、拡大・縮小、自分のコマンドの出力を表示するウィジェット、端末版の「rigtop」。
>
> 利用状況のデータは送りません。アプリの画面は英語です。無料のオープンソースです（MIT または Apache 2.0）。

**スクリーンショットのキャプション**（スクリーンショットの順に）:

- 管理しているマシンを 1 画面に。注意が要るものは色で分かる
- SSH でつないだワークステーション: CPU のコアと温度、2 枚の GPU、ドライブの健康状態、プロセスごとの GPU の使用量
- SSH でつないだ NAS: SMART の警告が出たドライブ、RAID の再構築、ZFS のプール、UPS
- 今の警告と履歴。1 時間・1 日・解消するまで黙らせることも
- （5 枚目を入れるとき）Windows での rigtop: CPU のコア、LibreHardwareMonitor 経由の温度とファン、GPU、プロセス

**このバージョンの新機能**: 空欄のまま

**製品の機能**（1 行に 1 つ）:

- コアごとの CPU の負荷・クロック・温度・ファン・電力
- すべての GPU と、プロセスごとの GPU の使用率とビデオメモリ（NVIDIA）
- メモリ、ドライブの読み書きと健康状態、ネットワークのリンク速度
- プロセスの検索・ツリー表示・詳細・タスクの終了・強制終了
- SSH で複数のホストを監視。サーバーもポートの開放も不要
- Linux のホストのドライブの健康状態: SMART・mdadm・ZFS・btrfs
- Linux のホストのプロセスごとのコンテナーと仮想マシン
- Network UPS Tools 経由の UPS の状態
- 警告の履歴とスヌーズ。しきい値はホストごとに
- 指示すると遠隔ホストの rigtop を更新
- パネルの大きさと配置を変えられる。グラフは 24 時間まで
- 自分のコマンドの出力を表示するウィジェット
- 端末版の rigtop
- 無料のオープンソース

**検索用語**（7 つまで、1 つの欄に 1 つ）:

- システムモニター
- リソースモニター
- ハードウェアモニター
- GPU
- サーバー監視
- SSH
- system monitor

**著作権と商標の情報**: Copyright (c) 2026 Akiva Miura

**追加のライセンス条項**: MIT License または Apache License 2.0 のいずれかを選択できます: https://github.com/akivajp/rigtop#license

**開発元**: Akiva Miura
