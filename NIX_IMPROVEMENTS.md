# Nix 利用者向け改善レポート

調査日: 2026-06-16 / 対象: `slice` (`slice-command`) の Nix 構成（`flake.nix` / `default.nix` / `shell.nix` / `flake.lock` / `.github/workflows/build_nix.yml`）

> **検証メモ**: 調査環境に nix が無いため `nix build` / `nix flake check` の実行検証は未実施。各項目の **要実機確認** は実機または CI でのビルド確認が必要であることを示す。慣行・メンテ状況・非推奨化は公式ドキュメント/各リポジトリで確認済み。

## 要約

flake は現状でも動作する。改善余地は **2 系統**:

- **(a) 衛生面の負債** — 出力スキーマが旧式（`defaultPackage`/`devShell` は非推奨だが後方互換で動作中）、`meta` が空、`flake.lock` が陳腐化（naersk は 2023-10、nixpkgs は約 15 か月前）、`nix fmt` 未対応。
- **(b) 実ユーザー体験ギャップ（最重要）** — **Nix 経由のユーザーだけシェル補完と man ページを受け取れない**。リリース tarball / Homebrew は同梱しているが、Nix パッケージは裸のバイナリのみ。

重い機構（NixOS module / Cachix / crane 全面移行）は本規模の leaf CLI には不要。

---

## 改善ポイント（優先度順）

### ~~1. シェル補完 + man ページをパッケージへ同梱 〔最大の実利・中規模〕~~ ✅ 対処済み (PR #236)

> 実装メモ: bash/zsh/fish 補完 + man を `postInstall` で同梱。PowerShell は `installShellFiles` 非対応かつ Nix が自動ロードしない非標準パスになるため除外。naersk は `$out/bin` へバイナリ配置後に `postInstall` を実行するため `$out/bin/slice` を直接呼べる（実機・CIで検証済み）。

~~バイナリは `slice --generate complete-bash|complete-zsh|complete-fish|man` で自前生成できる。`postInstall` で取り込む:~~

```nix
nativeBuildInputs = [ pkgs.installShellFiles ];
postInstall = ''
  installShellCompletion --cmd slice \
    --bash <($out/bin/slice --generate complete-bash) \
    --zsh  <($out/bin/slice --generate complete-zsh) \
    --fish <($out/bin/slice --generate complete-fish)
  $out/bin/slice --generate man > slice.1
  installManPage slice.1
'';
```

- ~~利益: `nix profile add` で補完と `man slice` が付き、tarball と同等になる。~~
- ~~**要実機確認**: sandbox 下で `$out` パスに正しく落ちるか。~~ → 検証済み（bash/zsh/fish 補完 + `slice.1.gz` が正しいパスに配置）。

### ~~2. flake 出力を現行スキーマへ + `meta` 付与 + `pname`/`version` を Cargo.toml 由来に 〔クイックウィン〕~~ ✅ 対処済み (PR #234)

> 実装メモ: `pname = cargoToml.package.name` はパッケージ名が `slice-command` になり既存 `nix profile` を壊すため、naersk の `name = "slice"` を採用。`version` のみ Cargo.toml 由来。`license` は `[ asl20 mit ]` で実機ビルド確認済み。

**Before**:
```nix
defaultPackage = naersk-lib.buildPackage {
    name = "slice";
    src = ./.;
};
devShell = with pkgs; mkShell {
  buildInputs = [ cargo rustc rustfmt rustPackages.clippy ];
  RUST_SRC_PATH = rustPlatform.rustLibSrc;
};
```

**After**（`eachDefaultSystem` 内、`pkgs` はスコープ内）:
```nix
let cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml); in
{
  packages.default = naersk-lib.buildPackage {
    pname = cargoToml.package.name;       # slice-command
    version = cargoToml.package.version;  # 0.5.0
    src = ./.;
    meta = with pkgs.lib; {
      description = "Slice file contents using Python-like slice notation";
      homepage = "https://github.com/ChanTsune/slice";
      license = with licenses; OR [ asl20 mit ];  # Apache-2.0 OR MIT（選択）
      mainProgram = "slice";
    };
  };
  devShells.default = pkgs.mkShell {
    buildInputs = with pkgs; [ cargo rustc rustfmt rustPackages.clippy ];
    RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
  };
}
```

- ~~利益: 非推奨 warning 解消、ライセンス/説明が tooling から可読、version 付きで `nix profile` がアップグレードを認識。~~
- ~~注意: `pname = "slice-command"` にするとバイナリ名フォールバックが `slice-command` を探して `nix run` が壊れるため、`meta.mainProgram = "slice"` が**必須**（上記に含む）。~~
- ~~**要実機確認**: `lib.licenses.OR` コンビネータが pin 済み nixpkgs に存在するか（lock 更新と合わせて確認）。CI の legacy job も `nix-build . -A default` へ追従要。~~

### ~~3. `flake.lock` を更新 〔クイックウィン〕~~ ✅ 対処済み (PR #234)

~~`nix flake update` 一発。naersk(2023-10) / 推移 nixpkgs(2024-01) を捨て、root nixpkgs(2025-03、約 15 か月遅れ) も更新。~~

- ~~利益: 現行 toolchain / バイナリキャッシュのヒット。~~
- ~~**要実機確認**: 更新後もビルドが通るか。~~ → naersk 2026-06 / nixpkgs 2026-06 / utils 2024-11 に更新、実機ビルド確認済み。

### ~~4. `nix fmt` を有効化 〔クイックウィン〕~~ ✅ 対処済み (PR #237)

```nix
formatter = pkgs.nixfmt-tree;  # 裸 nixfmt-rfc-style はディレクトリ "." を再帰処理できない
```

- ~~利益: ゼロ設定の `nix fmt`。~~
- ~~**要実機確認**: `nixfmt-tree` が pin 済み nixpkgs に存在するか（lock 更新後）。~~ → `nixfmt-tree-2.5.0` を固定 nixpkgs で確認。`.nix` 3ファイルを整形しコミット（冪等性確認済み）。`.git` fsmonitor ソケットは `nix fmt` に無影響。

### ~~5. README の Nix セクションを flake-first に 〔クイックウィン〕~~ ✅ 対処済み (PR #234)

~~`nix run github:ChanTsune/slice -- :5 file.txt`（試用）/ `nix profile add github:ChanTsune/slice`（導入、`install` は別名）/ `nix develop`・`direnv allow` を追記。`nix-env -f tarball` は非 flake 用フォールバックとして残す。表記揺れ `chantsune` → `ChanTsune` も統一。~~

### ~~6. CI を追従 〔クイックウィン〕~~ ✅ 対処済み (PR #234)

~~`build_nix.yml` の `build_legacy` を `nix-build . -A default` に、`build_flakes` に `nix flake check` を 1 ステップ追加、冗長な `extra_nix_config`（`cachix/install-nix-action@v31` は flakes を既定で有効化済み）を削除。~~

> 実装メモ: `build_legacy` は `nix-build . -A packages.x86_64-linux.default`（flake-compat はシステム階層をフラット化しないため `-A default` ではない）。`build_flakes` に `nix flake check` を追加し `extra_nix_config` を削除。

> ~~注: 旧スキーマ名の非推奨 warning は **非致命的**（exit 0）であり CI を失敗させない。~~

### 7. （任意）`doCheck` / `checks` 〔影響：低〕

`doCheck = true` で `nix build` が trycmd を通したバイナリを出すのは安い実利。一方 clippy/fmt の check 派生は既存 GitHub Actions（test.yml の 5 OS matrix + MSRV、rust-clippy.yml、format.yml）で十分カバー済みのため Nix 側に足すのは冗長。`doCheck` + 任意の `checks.default = self.packages.${system}.default` に留める。

### 8. （任意・長期）overlay / nixpkgs 上流化

- `overlays.default` で下流が `pkgs.slice` を得られるが、leaf CLI では実利小。
- nixpkgs 上流化は `slice` 名衝突なしで feasible（`pkgs/by-name/sl/slice/package.nix` を `rustPlatform.buildRustPackage` で）。ただし継続的な version-bump 義務が増えるため優先度低。flake を整えるのが先。

---

## やらない方がよい（過剰）

| 項目 | 却下理由 |
|---|---|
| NixOS / home-manager module | `slice` は daemon もサービスも設定ファイルも持たないステートレスフィルタ。module が設定すべきものが皆無。`environment.systemPackages = [ slice ]` で足りる |
| Cachix バイナリキャッシュ | ~6k LOC でコンパイル短い。さらに flake の `nixConfig.extra-substituters` は未信頼ユーザーの `nix run` に自動適用されず初回はソースビルド。account/CI secret の手間に見合わない |
| flake-parts へ移行 | 1 package + 1 devShell に module system は過剰。`eachDefaultSystem` のままでよい |
| `apps.default` 追加 | 派生名 = バイナリ名 `slice` なので `nix run` は既に解決。冗長 |
| Nix 側 clippy/fmt checks | 既存 GitHub Actions と重複 |
| naersk → crane 全面移行 | naersk は 2026 も健在。lock 更新で十分。ビルダーに触るなら nixpkgs 上流化を見据えた `rustPlatform.buildRustPackage` を検討 |

---

## 検証で訂正された通説

- **「naersk は死んだ」は誤り** — 2026 も活発にメンテされている。問題はフレームワークでなく陳腐化した lock。
- **dual-license の Nix 表記** — `[ asl20 mit ]` は nixpkgs マニュアル上「部分ごとに別ライセンス（AND）」を意味し、`Apache-2.0 OR MIT`（選択）と食い違う。`OR [ asl20 mit ]` が正確（`asl20`=Apache-2.0、`mit`=MIT。`apsl20`=Apple Public Source License と取り違えない）。
- **`nix fmt`** — 裸 `nixfmt-rfc-style` はディレクトリ渡しを再帰処理できない。現行推奨は `nixfmt-tree`（treefmt ラッパ）。
- **flake-compat の `edolstra/`→`NixOS/`** — GitHub リダイレクトが残るため URL 書換だけでは実利ほぼゼロ。価値は flake.lock 管理下の input にすること。
- **`cachix/install-nix-action@v31`** — flakes/nix-command を既定で有効化済み（`extra_nix_config` は冗長）。

---

## 推奨アクション（順序）

1. ~~flake.nix を現行スキーマ + `meta` + `pname`/`version` へ（#2）~~ ✅
2. ~~`flake.lock` を更新（#3）〔要実機〕~~ ✅
3. ~~CI を追従（#6）~~ ✅
4. ~~シェル補完 + man を `postInstall` 同梱（#1）〔要実機・最大の実利〕~~ ✅
5. ~~`formatter = pkgs.nixfmt-tree`（#4）~~ ✅
6. ~~README の Nix セクションを flake-first に（#5）~~ ✅
7. （任意）`doCheck`（#7）、overlay / 上流化（#8）

小 CLI の flake は薄く保つべきで、#1〜#6 で十分。**→ #1〜#6 すべて完了。残るは任意の #7・#8 のみ。**
