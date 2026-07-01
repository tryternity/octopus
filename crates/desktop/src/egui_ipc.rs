//! Tauri→egui IPC client：连本地 TCP 发 JSON line；连不上则 spawn octopus-egui。
//! 单实例锁 = port 文件 {pid,port} + pid 存活检测。

use serde_json::json;
use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

/// port 文件路径：~/.octopus/egui-ipc.port（与 egui/src/ipc.rs::port_file 一致）
fn port_file() -> PathBuf {
    octopus_infra::octopus_config_home().join("egui-ipc.port")
}

/// pid 是否存活（Unix kill(pid,0) 语义：返回 0 = 存活）。
fn pid_alive(pid: u32) -> bool {
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

// 跨平台 kill(pid,0)：macOS/Linux 走 libc；Windows 走 OpenProcess。
#[cfg(unix)]
unsafe fn libc_kill(pid: i32, sig: i32) -> i32 {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    kill(pid, sig)
}
#[cfg(not(unix))]
unsafe fn libc_kill(_pid: i32, _sig: i32) -> i32 {
    0 // Windows：第一版不检 pid，靠 TCP 连接失败兜底
}

/// 读 port 文件。返回 (pid, port)。文件不存在/解析失败返回 None。
fn read_port_file() -> Option<(u32, u16)> {
    let text = std::fs::read_to_string(port_file()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let pid = v["pid"].as_u64()? as u32;
    let port = v["port"].as_u64()? as u16;
    Some((pid, port))
}

/// 解析 octopus-egui 二进制路径：与当前 exe 同目录（dev: target/debug；bundled: .app/Resources）。
fn egui_binary_path() -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("octopus-egui"));
    p.set_file_name("octopus-egui");
    p
}

/// 只探测连接已运行的 egui（**不 spawn**）。pid 死则清 stale port 文件。
fn try_connect() -> Option<TcpStream> {
    let (pid, port) = read_port_file()?;
    if !pid_alive(pid) {
        let _ = std::fs::remove_file(port_file());
        return None;
    }
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()
}

/// spawn octopus-egui（后台，不阻塞）。返回 spawn 是否成功（失败通常是二进制未编译）。
fn spawn_egui() -> std::io::Result<()> {
    let bin = egui_binary_path();
    match std::process::Command::new(&bin).spawn() {
        Ok(_) => {
            log::info!("已 spawn octopus-egui: {}", bin.display());
            Ok(())
        }
        Err(e) => {
            log::error!(
                "spawn octopus-egui 失败 ({}): {} —— 若未编译，请 `cargo build -p octopus-egui`（与 desktop 同 profile：debug/release）",
                bin.display(),
                e
            );
            Err(e)
        }
    }
}

/// 发一条 JSON line。无运行实例时 spawn **一次**（避免循环反复 spawn 出多实例），
/// 随后轮询连接 ~3s；spawn 失败（二进制缺失等）立即放弃。
fn send(payload: serde_json::Value) {
    let mut spawned = false;
    for _attempt in 0..30 {
        if let Some(mut stream) = try_connect() {
            let line = format!("{}\n", payload);
            if stream.write_all(line.as_bytes()).is_ok() {
                let _ = stream.flush();
                return;
            }
        }
        // 连不上：若尚未 spawn 过，spawn 一次（不重复——egui 端另有单例锁双保险）
        if !spawned {
            match spawn_egui() {
                Ok(()) => spawned = true,
                Err(_) => {
                    log::warn!("IPC 发送放弃（octopus-egui spawn 失败）: {}", payload);
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    log::warn!("IPC 发送失败（egui 进程未就绪 ~3s）: {}", payload);
}

/// 打开并选中笔记（OCR/ASR→notepad 场景）。
pub fn open_note(note_id: i64) {
    send(json!({"type":"open","note_id":note_id}));
}

/// 通知 egui 刷新列表（Tauri 侧写笔记后）。
pub fn notes_changed() {
    send(json!({"type":"notes_changed"}));
}

/// 托盘唤起：show + focus。
pub fn show() {
    send(json!({"type":"show"}));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn send_delivers_json_line_to_server() {
        // 起 mock server，写 port 文件指向它
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let _ = std::fs::write(
            port_file(),
            serde_json::json!({"pid": std::process::id(), "port": port}).to_string(),
        );
        listener.set_nonblocking(true).unwrap();

        send(json!({"type":"notes_changed"}));

        // accept 一条连接读一行
        let (mut s, _) = listener.accept().unwrap();
        s.set_nonblocking(false).unwrap();
        let mut buf = [0u8; 128];
        let n = s.read(&mut buf).unwrap();
        let line = String::from_utf8_lossy(&buf[..n]);
        assert!(line.contains("\"type\":\"notes_changed\""), "server 应收到消息: {}", line);

        let _ = std::fs::remove_file(port_file());
    }
}
