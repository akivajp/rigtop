# Microsoft Store の申請内容

rigtop（Store ID `9N6JMV1QNDCK`、https://apps.microsoft.com/detail/9n6jmv1qndck ）の申請の内容です。最初の申請（2026-11-10、v0.32.0）は Partner Center（日本語の画面）に手で入れました。**掲載情報の文とスクリーンショットは、いまは `packaging/msix/store/` の TOML が正本で、Microsoft Store のワークフローが API で反映します**（下の「ストア登録情報」）。このファイルに残しているのは、API では入れられず画面で入れた欄（価格・プロパティ・年齢別レーティング・申請オプション）の記録です。

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

- タグを打つと、リリースのワークフローが成果物 `store-msix` の `rigtop-vX.Y.Z-x64.msix`（`-test` の付いたものではない）で下書きの申請を作る。画面から上げる必要は無い
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

英語（米国、`en-us`）はパッケージの言語。日本語（`ja`）は「その他のストア登録情報の言語」として足してある（アプリの画面が英語だけなので、パッケージの言語には入れていない）。

掲載情報（製品名・短い説明・説明・このバージョンの新機能・製品の機能・検索用語・著作権・追加のライセンス条項・開発元・スクリーンショットとキャプション）は、`packaging/msix/store/en-us.toml`・`ja.toml` が正本。最初の申請の内容を API で取り出して作った。

- Store へは Microsoft Store のワークフローで反映するので、画面に貼らない。最初の申請では、Markdown の表示からコピーして貼った説明文の**箇条書きの改行が消えて 1 行につながった**（v0.33.0 の申請で直した）。
- タグを打つとリリースのワークフローが、その版の MSIX と掲載情報で**下書き**の申請を作る。変わる項目と新しい文はそのジョブ（Microsoft Store submission）のログに出るので、それを見て、Microsoft Store のワークフロー（手動の実行）の `commit` で提出する。やめるときは `discard`。
- **API で作った申請は、提出するまで Partner Center の画面に変更が出ない**。画面で変えたり「送信して認定を受ける」を押したりすると、その申請は API で扱えなくなる。下書きは画面で触らない。
- **リリースのたびに `release_notes`（このバージョンの新機能）を、英語・日本語とも書き換えてからタグを打つ**。
- 掲載情報だけを直すとき（パッケージは変えない）は、Microsoft Store のワークフローを `submit` で動かす。スクリーンショットを差し替えたときは `images` も付ける（タグのリリースでは差し替えない）。
- 言語を足すときは、`<言語>.toml`（例: `de-de.toml`）を足す。

### スクリーンショットとロゴ

スクリーンショット（PNG、1600×900）は `packaging/msix/store/screenshots/` にある。`--demo` の架空の 3 台（ワークステーション・ノート PC・NAS）を、実機の代わりにオフスクリーンで描いた（手元の `tmp/store/shoot.sh`、README の画像と同じ方法）。アプリの画面が英語だけなので、英語と日本語で同じ画像を使い、キャプションだけ言語ごとに変えている（キャプションは各言語の TOML）。

| 順 | ファイル | 内容 |
| --- | --- | --- |
| 1 | `1-overview.png` | 3 台のオーバービュー |
| 2 | `2-workstation.png` | ワークステーションの詳細（コア・GPU 2 枚・ストレージ・プロセスのツリー） |
| 3 | `3-storage.png` | NAS（SMART の警告・RAID の再構築・ZFS・UPS） |
| 4 | `4-alerts.png` | 警告の一覧と履歴、スヌーズ |

- Windows で動いている画面は入れていない。共有の仮想マシンでは GPU・温度が出ず、ソフトウェア描画で rigtop-gui 自身が CPU 300% と出るため。入れるなら、実機の Windows で Store 版を表示して撮り（プロセス一覧は `/top` などで絞り、利用者名・ホスト名・他社のアプリ名が写らないようにする）、TOML に足して `submit`・`images` で反映する。
- Microsoft Store ロゴ（任意）は画面から登録済み: 1:1 のボックス アート（`store-box-1080.png`）と 9:16 のポスター アート。手元の `tmp/store/make_store_art.py` でアイコンの SVG から作った。API のワークフローはロゴを変えない。
