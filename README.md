<p align="center">
  <img src="assets/Efude_sub.png" alt="Efude logo" width="120">
</p>

<h1 align="center">Efude-neo</h1>

Efude-neoは、**852wa氏の[Efude](https://github.com/852wa/Efude)をもとにした派生版**です。元プロジェクトの作者と貢献者に敬意を表します。

## 更新履歴

### v0.3.2 (2026-09-30)

- キャンバスのサイズを変更した後、ベクターレイヤーの線を編集すると位置がずれる不具合を修正しました。

### v0.3.1 (2026-09-29)

- キャンバスより大きい画像を貼り付けたとき、端が欠ける不具合を修正しました。
- 未確定の貼り付け画像がタブ切替で消える不具合を修正しました。貼り付けできないレイヤーを選んだ場合も、画像を保持して再試行できます。
- 描画中にタブを切り替えると、別の原稿へ線が入り、元の線を正しく取り消せない不具合を修正しました。
- 描画中にタブを閉じると未保存の確認なしで線が消える不具合を修正し、終了時の未保存判定にも描画中の線を含めるようにしました。
- 新規作品が既存の作品目次や原稿を上書き・流用する不具合と、作品名によって選択したフォルダーの外へ保存される不具合を修正しました。
- 作品目次を保存できない場合、ページの追加・削除・並べ替え・ノンブル設定が保存済みの内容と食い違う不具合を修正しました。目次の保存には一時ファイルを使い、失敗時は元の目次を保ちます。
- ページの並べ替え後、ノンブルの小口側・ノド側が元のページ位置に残る不具合を修正しました。
- Undo・Redoの退避データを読み込めない場合、履歴を飛び越えたり保存状態を変えたりせず、エラーを表示するようにしました。

### v0.3.0 (2026-09-28)

- 塗り残し・消し忘れチェックを追加。小さな透明穴、孤立した点、仕上がり範囲外の描画を強調して確認できます。
- タイムラプスのMP4出力と、仕上がり時間・完成画像の静止時間・動画サイズの指定に対応しました。
- マクロの対象レイヤー指定と、手順の追加・削除・並べ替え・値の編集を追加しました。
- ブラシセットの取り込み前に、重複と同名ブラシの扱いを確認できるようにしました。初期動作は今のブラシへの追加です。
- 漫画の一括PNG出力で、全タブの未保存内容と既存PNGを確認できるようにしました。保存・書き出し失敗時の復元も改善しました。

### v0.2.0 (2026-09-28)

- GPUブラシで、ブラシに追従するグレインを使用したときのCPU描画との差を修正しました。該当する組み合わせはCPU描画へ切り替えます。
- 複数タブを開いたとき、非表示のタブも自動バックアップの対象にし、未保存の作品も次回起動時に復旧できるようにしました。
- 下絵ガイド、制作タイムラプス、操作マクロを追加しました。

## 追加機能

- **下絵ガイド**: 「表示 > 下絵ガイド」から画像をキャンバスの描画レイヤーの下へ配置し、不透明度・位置・倍率を変更できます。ガイドは文書に保存され、通常のPNG・JPEG・PSD書き出しには入りません。
- **塗り残し・消し忘れチェック**: 「編集」メニューから対象レイヤーと仕上がり範囲を選び、透明穴・孤立点・範囲外の候補を強調します。作品の画素は変更せず、候補を押して拡大確認できます。
- **制作タイムラプス**: 「記録」メニューから開始し、ガイドを動画へ含めるかを開始時に選べます。OFFでも編集中はガイドを表示でき、動画からは除きます。MP4は1〜600秒の仕上がり時間（完成画像の静止時間を含む）と動画サイズ・収め方を指定できます。MP4・MJPEG AVIとも30fpsです。
- **操作マクロ**: レイヤーの新規作成・複製・名前・表示・不透明度・合成モードを記録し、対象レイヤーと手順を編集して別の文書で再生できます。対象不足や同名レイヤーは実行前に確認し、再生は1回のUndoで戻せます。ブラシの線や画面操作は記録対象外です。
- **取り込み・一括出力の確認**: ブラシセットは適用前に重複・同名の扱いを確認でき、直前の取り込みを戻せます。漫画の一括PNGは未保存のページと同名PNGを確認し、現在の内容を使うか、原稿を保存してから出力するかを選べます。
- **作業の復旧**: 保存していないタブも含め、変更のある各タブを定期的に別々の復旧ファイルへ保存します。異常終了後の起動時にサムネイルで選んで復元でき、元の作品ファイルは上書きしません。通常の自動バックアップは保存済み文書の隣に引き続き作成します。

### 記録データの保存先

タイムラプスの連番JPEGは `%LOCALAPPDATA%\Efude-neo\timelapse\` に、作業の復旧データは `%LOCALAPPDATA%\Efude-neo\recovery\` に、マクロは `%APPDATA%\Efude-neo\macros\` に保存します。タイムラプスの記録を終了した後も元のJPEGは残り、「記録 > 以前の記録から動画を書き出す…」から再書き出しできます。AVIが1.9GBを超える場合はJPEGフレームを利用してください。


<p align="center">
  <b>An open-source painting and manga app for Windows, written in Rust.</b><br>
  オープンソースのお絵かき・漫画制作アプリ
</p>

<p align="center">
  <a href="https://github.com/AiWithYou/Efude-neo/releases"><b>Releases</b></a> ·
  <a href="docs/USER_GUIDE.md">User Guide</a> ·
  <a href="#日本語">日本語</a>
</p>

![Efude: an illustration open on the canvas, with the brush presets, colour wheel and tool settings beside it](docs/images/screenshot.png)

Efude-neo builds on Efude's responsive drawing feel, illustration and manga
tools, and characterful brushes.

> **Public test.** Efude is under active development. Files and settings may
> change between versions — keep backups of important work.

## Download

Download the Windows build from [Efude-neo Releases](https://github.com/AiWithYou/Efude-neo/releases/latest). The [original Efude releases](https://github.com/852wa/Efude/releases) are separate from this fork.

- `Efude-v<version>-windows-x64.zip` — portable: unzip anywhere and run `efude.exe`.
- `Efude-v<version>-windows-x64.msi` — installer.

MP4 timelapse export needs FFmpeg with `libx264`, installed separately. Choose its executable in the export dialog, or use FFmpeg on PATH. AVI export needs no FFmpeg.
Macros saved by 0.3, including old macros you edit and save, cannot be read by 0.2. Old macros can still be opened in 0.3.

Windows 10 or 11 (64-bit). A pen tablet works through Windows Ink or WinTab.
The builds are not code-signed yet, so Windows SmartScreen may warn on first
start ("More info" → "Run anyway").

## Features

- **Drawing**: pen, pencil, brush, watercolor, airbrush, blender, blur, smudge and eraser presets; stabilization; pressure curves per brush and for the whole app; tapers; tip and grain textures (templates included); wet mixing; anti-aliasing levels; a transparent colour that erases with any brush or fill; brush sets you can name, arrange and export.
- **Layers**: raster layers, vector layers whose lines stay editable Bézier curves (move anchors and handles later), and folders (drag rows to reorder or put layers into folders), blend modes, opacity, clipping, masks, locking, reference layers.
- **Selection and editing**: rectangle, ellipse, lasso, polygon, magic wand, color range, selection brush and quick mask; move, transform and mesh warp; copy and paste (also images from other apps); flood fill.
- **Filters**: blur (Gaussian, lens, smooth, motion), sharpen, noise reduction, tone curve, levels, hue/saturation/brightness, color grading, chromatic aberration, mosaic, noise and line width — each with a before/after preview.
- **Manga**: page setup with bleed and trim, panel splitting, screentones, focus and speed lines, speech balloons with vertical text, and multi-page books with print export.
- **Files**: the `.efude` format (layers kept), PSD import and export, PNG / JPEG / BMP / GIF import, PNG / JPEG export, automatic backups.
- **Canvas**: exact pixels at every zoom (100% = one screen pixel), a checkerboard behind transparent parts, rotation and flipping, rulers and symmetry, tabs for several documents.

See the [User Guide](docs/USER_GUIDE.md) for how to use it.

## Build from source

Requires Rust 1.95 or later.

```powershell
cargo run -p efude-app --release
```

The workspace is split into engine crates (`efude-core`, `efude-input`,
`efude-stroke`, `efude-brush`, `efude-canvas`, `efude-gpu`, `efude-io`,
`efude-comic`) and the application (`efude-ui`, `efude-app`). File formats are
described in [docs/spec](docs/spec). Releases are built by GitHub Actions; see
[docs/RELEASING.md](docs/RELEASING.md).

## License

- Engine crates: MIT OR Apache-2.0 ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
- Application crates (`efude-ui`, `efude-app`) and assets: MPL-2.0 ([LICENSE-MPL](LICENSE-MPL)).
- Third-party notices ship with each release (`THIRD_PARTY_NOTICES.txt`); see also [NOTICE](NOTICE).
- The name and logo are covered by the [trademark policy](TRADEMARKS.md).

---

## 日本語

Efude-neoは、Rustで作られたWindows向けのオープンソースのお絵かき・漫画制作アプリの派生版です。
気持ちよく描ける書き味、イラストと漫画のための一通りの機能、個性のあるブラシを目指しています。

> **公開テスト版です。** 開発中のため、バージョンによってファイルや設定が変わることがあります。大切な作品はバックアップを取ってください。

### ダウンロード

[Efude-neo の Releases](https://github.com/AiWithYou/Efude-neo/releases/latest) からWindows版をダウンロードできます。[元のEfudeの配布](https://github.com/852wa/Efude/releases)はこの派生版とは別です。

- `Efude-v<バージョン>-windows-x64.zip` — ポータブル版。好きな場所に展開して `efude.exe` を起動します。
- `Efude-v<バージョン>-windows-x64.msi` — インストーラー。

MP4のタイムラプス出力には、`libx264`を含むFFmpegが別途必要です。書き出し画面で実行ファイルを指定するか、PATHにあるFFmpegを使います。AVIはFFmpeg不要です。
0.3で保存したマクロ（旧マクロを編集して保存したものを含む）は0.2では読めません。旧マクロは0.3でも読み込めます。

Windows 10 / 11（64ビット）。ペンタブレットは Windows Ink または WinTab で使えます。
まだコード署名をしていないため、初回起動時に Windows SmartScreen の警告が出ることがあります（「詳細情報」→「実行」）。

### 主な機能

- **描画**: ペン・鉛筆・筆・水彩・エアブラシ・色混ぜ・ぼかし・指先・消しゴム、手ブレ補正、ブラシごととアプリ全体の筆圧カーブ、入り抜き、先端とグレイン（テンプレート付き）、混色、アンチエイリアス4段階、どのブラシや塗りつぶしでも消せる透明色。ブラシは名前を付けて並べ、セットとして書き出せます。
- **レイヤー**: ラスターレイヤー、線をあとからベジェの制御点で編集できるベクターレイヤー、フォルダー（行をドラッグして並べ替え・フォルダーへ入れる）、合成モード、不透明度、クリッピング、マスク、ロック、参照レイヤー。
- **選択と編集**: 矩形・楕円・投げ縄・多角形・自動選択・色域・選択ペン・クイックマスク、移動・変形・メッシュ変形、コピー＆貼り付け（他のアプリの画像も可）、塗りつぶし。
- **フィルター**: ぼかし（ガウス・レンズ・スムーズ・移動）、シャープ、ノイズ除去、トーンカーブ、レベル補正、色相・彩度・明度、カラーグレーディング、色収差、モザイク、ノイズ、線の太さ。どれも適用前後をプレビューできます。
- **漫画**: 裁ち落とし・仕上がり線つきの原稿設定、コマ割り、スクリーントーン、集中線・流線、縦書きのフキダシ、複数ページの作品と印刷用書き出し。
- **ファイル**: `.efude` 形式（レイヤーを保持）、PSDの読み込み・書き出し、PNG / JPEG / BMP / GIFの読み込み、PNG / JPEGの書き出し、自動バックアップ。
- **キャンバス**: どの倍率でも画素をそのまま表示（100%で画面の1ピクセル）、透明部分は市松模様で表示、回転・反転、定規と対称定規、複数ドキュメントのタブ。

使い方は [ユーザーガイド](docs/USER_GUIDE.md) を参照してください。

### ソースからのビルド

Rust 1.95以降が必要です。

```powershell
cargo run -p efude-app --release
```

### ライセンス

エンジン部分は MIT OR Apache-2.0、アプリ部分（`efude-ui`・`efude-app`）と素材は MPL-2.0 です。同梱ライブラリの表記は各リリースの `THIRD_PARTY_NOTICES.txt` に含まれます。名前とロゴの扱いは [商標ポリシー](TRADEMARKS.md) のとおりです。
