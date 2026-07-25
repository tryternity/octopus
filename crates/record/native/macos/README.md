# octopus-sck-helper

octopus 屏幕录制功能的 macOS helper 子进程（ScreenCaptureKit 封装）。

## 来源

Vendor 自 openscreen：
- 原文件：`electron/native/screencapturekit/Sources/OpenScreenScreenCaptureKitHelper/main.swift`
- 上游 commit：`f57e36e25448b5af6c7b1b271066fe5beb9b8a49`
- 修改声明见 `LICENSE`

## 构建

```bash
# 手动构建（debug）
cd crates/record/native/macos
swift build

# 推荐用脚本（release + universal binary，拷贝到 desktop/binaries/）
./scripts/build-macos-helper.sh
```

## 协议

与主进程通过 JSON-over-stdio 通信：

- **录制模式**：argv[1] = RecordingRequest JSON（schema 见 `crates/record/src/protocol.rs`）
- **子命令模式**：
  - `--check-permission`：emit `{"event":"permission-status","granted":bool}` 并退出
  - `--request-permission`：同上，但触发系统授权弹窗
  - `--list-displays`：emit `{"displays":[{id,name,width,height,is_primary}]}`
  - `--list-windows`：emit `{"windows":[{id,title,app_name,width,height}]}`
  - `--list-microphones`：emit `{"microphones":[{id,name}]}`
- **stdout 事件**：每行一个 JSON（详见 protocol.rs `HelperEvent`）
- **stdin 命令**：`pause` / `resume` / `stop`

## License

MIT（基于 openscreen，原作者 Siddharth Vaddem）。详见 `LICENSE`。
