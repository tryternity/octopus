//! 测试用 in-process WebSocket server（`#[cfg(test)]` 公共 harness）。
//!
//! 4 家 provider（aliyun/bytedance/tencent/baidu）的 `run_xxx_session` WS 主循环零覆盖
//!（spec §2.2 要求的 `close_frame_emits_*` 测试缺失）。本 harness 起 in-process
//! tokio-tungstenite server（bind `127.0.0.1:0` 随机端口），让 provider 真走一遍 WS 协议，
//! 测试侧通过 handler 闭包控制「按剧本发响应 / 收客户端消息」，覆盖 Close 帧终态、
//! 空 Text 污染、稳态判定等 WS 边界 bug。
//!
//! **设计**：与项目现有 `download` crate 的 `httpmock` 真集成哲学一致——不引入 mockall
//! 等抽象层，用 tokio-tungstenite 自带 `accept_async` 起 localhost server。零新依赖
//!（tokio + tokio-tungstenite + futures-util 均已在 [dependencies]）。
//!
//! **两种用法**：
//! - [`WsTestServer::start`]：简单剧本模式——handler 接收 WebSocketStream 后自由收发
//!   （适于需要握手交互的场景，如 bytedance 要先收 init config 帧再发响应）
//! - [`WsTestServer::start_script`]：纯发消息剧本——server 只发 `Vec<Message>` 不读客户端
//!   （适于 baidu/aliyun 这类「客户端先发后收」的协议，测试只关心服务端发的响应）

use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

/// 测试用 in-process WS server。
///
/// `port` 供 provider `connect_async` 连接；`received` 收集客户端发来的消息供断言。
/// handler task 在客户端连接关闭后自然结束（不阻塞 drop）。
pub struct WsTestServer {
    port: u16,
    /// 客户端发来的消息（handler 通过 [`ServerHandle::received`] push）。
    pub received: Arc<Mutex<Vec<Message>>>,
}

impl WsTestServer {
    /// 启动 server，用自定义 handler 处理连接。
    ///
    /// handler 签名：`async fn(ws: WebSocketStream<TcpStream>, received: Arc<Mutex<Vec<Message>>>) -> Result<()>`
    /// handler 内可自由 `ws.next()` 收客户端消息（push 到 received）+ `ws.send(msg)` 发响应。
    /// handler 返回 Err 时仅 log（不影响测试断言——测试靠 received + provider 的 result_tx 判定）。
    pub async fn start<F, Fut>(handler: F) -> Self
    where
        F: FnOnce(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, Arc<Mutex<Vec<Message>>>) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("local_addr").port();
        let received: Arc<Mutex<Vec<Message>>> = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        tokio::spawn(async move {
            // 接受一个连接（测试场景只连一次）
            let (stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[test_ws_server] accept failed: {e}");
                    return;
                }
            };
            let ws = match accept_async(stream).await {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("[test_ws_server] accept_async failed: {e}");
                    return;
                }
            };
            if let Err(e) = handler(ws, received_clone).await {
                eprintln!("[test_ws_server] handler error: {e}");
            }
        });

        Self { port, received }
    }

    /// 纯发消息剧本模式：server 只按 `script` 依次发消息，不主动读客户端。
    ///
    /// 适用 baidu/aliyun 这类「客户端先发 START/init + PCM，服务端只发响应」的协议。
    /// server 发完 script 后关闭连接（发 Close 帧 + close）。
    pub async fn start_script(script: Vec<Message>) -> Self {
        Self::start(move |mut ws, received| async move {
            for msg in script {
                ws.send(msg).await?;
            }
            // 发完剧本，关闭连接（触发客户端 ws.next() 返 None 或 Close）
            ws.close(None).await.ok();
            // drain 客户端发的消息（START/PCM/finish 等），收集到 received
            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(m) => {
                        let mut r = received.lock().unwrap();
                        r.push(m);
                    }
                    Err(_) => break,
                }
            }
            Ok(())
        })
        .await
    }

    /// provider 连接用的 URL（`ws://127.0.0.1:{port}/`）。
    ///
    /// 带末尾 `/`（path 根）：tungstenite 0.24 的 `connect_async(&str)` 对无 path 的 URL
    ///（如 `ws://host:port?query`）构造 HTTP 请求行时 path 为空 → 非法，server `accept_async`
    /// 报 `HTTP format error`。加 `/` 后 provider 拼 query 得 `ws://host:port/?sn=x`，path=`/` 合法。
    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/", self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::connect_async;

    /// 验证 harness 基础功能：server 发一条 Text + Close，客户端能收到。
    #[tokio::test]
    async fn server_sends_script_messages() {
        let server = WsTestServer::start_script(vec![
            Message::Text(r#"{"hello":"world"}"#.into()),
        ])
        .await;
        let url = server.ws_url();

        let (mut ws, _) = connect_async(url).await.expect("connect");
        // 收第一条消息
        let first = ws.next().await.expect("msg").expect("ok");
        assert_eq!(first.into_text().unwrap(), r#"{"hello":"world"}"#);
    }

    /// 诊断：带 query string 的 ws:// URL 是否影响 connect_async + accept_async 握手。
    /// ws_url() 现在带末尾 `/`（path 根），provider 拼 query 得 `ws://host:port/?sn=x` 合法。
    #[tokio::test]
    async fn server_handles_url_with_query() {
        let server = WsTestServer::start_script(vec![
            Message::Text(r#"{"ok":true}"#.into()),
        ])
        .await;
        let url = format!("{}?sn=test-uuid-1234", server.ws_url());

        let (mut ws, _) = connect_async(url).await.expect("connect with query");
        let first = ws.next().await.expect("msg").expect("ok");
        assert_eq!(first.into_text().unwrap(), r#"{"ok":true}"#);
    }
    /// 验证 handler 模式：自定义收发逻辑（模拟「收 START → 发响应」交互）。
    #[tokio::test]
    async fn handler_receives_client_messages() {
        let server = WsTestServer::start(|mut ws, received| async move {
            // 收客户端的 START 帧
            if let Some(Ok(msg)) = ws.next().await {
                received.lock().unwrap().push(msg);
                // 回一条响应
                ws.send(Message::Text(r#"{"type":"ack"}"#.into())).await?;
            }
            ws.close(None).await.ok();
            Ok(())
        })
        .await;
        let url = server.ws_url();

        let (mut ws, _) = connect_async(url).await.expect("connect");
        ws.send(Message::Text(r#"{"type":"START"}"#.into())).await.unwrap();
        // 收响应
        let resp = ws.next().await.unwrap().unwrap();
        assert_eq!(resp.into_text().unwrap(), r#"{"type":"ack"}"#);

        // 等 server task 处理完（收消息入 received）
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let recv = server.received.lock().unwrap();
        assert_eq!(recv.len(), 1, "应收到客户端的 START 帧");
        assert_eq!(recv[0].clone().into_text().unwrap(), r#"{"type":"START"}"#);
    }
}
