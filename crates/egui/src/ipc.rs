//! 本地 TCP IPC server（Tauri 主进程 → egui）。
//! - bind 127.0.0.1:0，端口写 ~/.octopus/egui-ipc.port（{pid,port}，单实例锁）。
//! - JSON line：每行一条。收到消息经 mpsc 推给 egui 主线程。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::sync::mpsc::Sender;

/// port 文件路径：~/.octopus/egui-ipc.port
pub fn port_file() -> std::path::PathBuf {
    octopus_infra::octopus_config_home().join("egui-ipc.port")
}

/// IPC 消息（Tauri → egui）。JSON line，`type` tag 区分。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcMsg {
    /// 打开并选中某笔记（OCR/ASR→notepad）。
    Open { note_id: i64 },
    /// Tauri 侧写笔记后通知刷新列表。
    NotesChanged,
    /// 托盘唤起：show + focus。
    Show,
}

/// 写 port 文件（{pid,port}）。单实例锁的 server 侧凭证。
fn write_port_file(port: u16) -> Result<()> {
    let path = port_file();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).ok();
    }
    let body = serde_json::json!({ "pid": std::process::id(), "port": port });
    std::fs::write(&path, body.to_string())
        .with_context(|| format!("写 port 文件失败: {}", path.display()))?;
    Ok(())
}

/// 启动 IPC server（后台线程）。返回后主线程可从 rx 收消息。
/// 启动失败不阻断 UI（记事本仍可独立用，只是收不到外部 open/refresh）。
pub fn start(tx: Sender<IpcMsg>) {
    std::thread::spawn(move || {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(e) => {
                log::error!("IPC bind 失败: {}", e);
                return;
            }
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        if let Err(e) = write_port_file(port) {
            log::error!("{}", e);
        }
        log::info!("egui IPC listening on 127.0.0.1:{}", port);

        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(&stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break }; // 对端断开
                    let line = line.trim();
                    if line.is_empty() { continue; }
                    match serde_json::from_str::<IpcMsg>(line) {
                        Ok(msg) => {
                            log::info!("IPC recv: {:?}", msg);
                            let _ = tx.send(msg);
                        }
                        Err(e) => log::warn!("IPC 解析失败 ({}): {}", e, line),
                    }
                }
                let _ = stream.shutdown(std::net::Shutdown::Both);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn server_receives_json_line_messages() {
        let (tx, rx) = mpsc::channel::<IpcMsg>();
        start(tx);
        // 轮询 port 文件直到写出
        let port = loop {
            if let Ok(text) = std::fs::read_to_string(port_file()) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(p) = v["port"].as_u64() {
                        break p as u16;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        writeln!(stream, "{}", serde_json::json!({"type":"open","note_id":42})).unwrap();
        writeln!(stream, "{}", serde_json::json!({"type":"notes_changed"})).unwrap();
        stream.flush().unwrap();

        let m1 = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let m2 = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(m1, IpcMsg::Open { note_id: 42 }));
        assert!(matches!(m2, IpcMsg::NotesChanged));

        let _ = std::fs::remove_file(port_file());
    }

    #[test]
    fn ipc_msg_roundtrip() {
        let open = serde_json::to_string(&IpcMsg::Open { note_id: 7 }).unwrap();
        assert!(open.contains("\"type\":\"open\""));
        let parsed: IpcMsg = serde_json::from_str(&open).unwrap();
        assert!(matches!(parsed, IpcMsg::Open { note_id: 7 }));
    }
}
