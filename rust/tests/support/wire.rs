// Raw JSON-RPC avoids rmcp's narrower typed MRTR client union.
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

pub fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientInfo":{"name":"wire-host","version":"1"},
        "io.modelcontextprotocol/clientCapabilities":{"extensions":{"openai/elicitation":{"form":{}}}},
        "test/keep":{"trace":"original context"}
    })
}
pub struct Rpc {
    read: BufReader<ReadHalf<DuplexStream>>,
    write: WriteHalf<DuplexStream>,
    pub legacy_answer: Option<Value>,
    pub legacy_requests: Vec<Value>,
}
impl Rpc {
    pub fn new(stream: DuplexStream) -> Self {
        let (read, write) = tokio::io::split(stream);
        Self {
            read: BufReader::new(read),
            write,
            legacy_answer: None,
            legacy_requests: vec![],
        }
    }
    pub async fn send(&mut self, value: Value) {
        self.write
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
        self.write.flush().await.unwrap();
    }
    pub async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        loop {
            let mut line = String::new();
            assert_ne!(
                self.read.read_line(&mut line).await.unwrap(),
                0,
                "unexpected transport EOF"
            );
            let message: Value = serde_json::from_str(&line).unwrap();
            if message.get("method").is_some() {
                assert_eq!(
                    message["method"], "openai/elicitation/create",
                    "unexpected separate server request on modern flow"
                );
                let answer = self
                    .legacy_answer
                    .clone()
                    .expect("modern forms must be embedded, not separate RPCs");
                self.legacy_requests.push(message.clone());
                self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":answer}))
                    .await;
                continue;
            }
            assert_eq!(message["id"], id);
            return message;
        }
    }
    pub async fn modern(&mut self, id: u64, method: &str, mut params: Value) -> Value {
        params["_meta"] = meta();
        self.request(id, method, params).await
    }
}
