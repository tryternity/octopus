# MNN 预编译包（vendor）

ocr-rs（OCR 依赖）默认在 release 构建时从 GitHub 下载 MNN 预编译包：

```
https://github.com/zibo-chen/MNN-Prebuilds/releases/download/dev/mnn-dev-macos-universal.tar.gz
```

断网或受限时该下载会失败、卡死构建（`ocr-rs` build.rs 的 `download_file` panic）。本目录把该 tarball 提交进工程，`run-octopus.sh` 的 `seed_mnn_prebuilt()` 会在构建前把它拷进 ocr-rs 的 `target/*/build/ocr-rs-*/out/prebuilt/` 缓存目录——ocr-rs `build.rs` 命中已存在即跳过下载（build.rs:313），extract 后正常链接。

## 文件
- `mnn-dev-macos-universal.tar.gz`（~7.3MB）—— macOS universal（x86_64 + aarch64），MNN prebuilt 版本 tag = `dev`。

## 更新
ocr-rs 升级、或 MNN 版本 tag 变化时，重新下载覆盖即可：

```sh
curl -L -o mnn-dev-macos-universal.tar.gz \
  https://github.com/zibo-chen/MNN-Prebuilds/releases/download/dev/mnn-dev-macos-universal.tar.gz
```

## 跨平台
当前只 vendor 了 macOS universal（对应 `get_prebuilt_asset_name` 的 `mnn-dev-macos-universal`）。Linux/Windows 构建仍走 ocr-rs 在线下载——seed 仅按同名文件填充，无对应平台 tarball 时不影响 ocr-rs 原有下载逻辑。需要 Linux/Windows 离线时，把对应 asset 也放进本目录并扩展 `seed_mnn_prebuilt` 的文件名匹配。
