//! Observe actual rmcp messages from the two executable application examples.
use rmcp::{
    ServerHandler, ServiceExt,
    model::{CallToolRequestParams, ReadResourceRequestParams},
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::timeout;

pub async fn observe(server: impl ServerHandler) -> Value {
    timeout(Duration::from_secs(10), async {
        let (left, right) = tokio::io::duplex(64 * 1024);
        let (server, client) = tokio::join!(server.serve(left), ().serve(right));
        let server = server.unwrap();
        let client = client.unwrap();
        let capabilities = serde_json::to_value(&client.peer_info().unwrap().capabilities).unwrap();
        let tools = client.list_all_tools().await.unwrap();
        let resources = client.list_all_resources().await.unwrap();
        let contents = client.read_resource(ReadResourceRequestParams::new("ui://parts/viewer")).await.unwrap();
        let request: CallToolRequestParams = serde_json::from_value(json!({
            "name":"open_catalog", "arguments":{},
            "_meta":{"openai/session":"synthetic-app-session"}
        })).unwrap();
        let result = client.call_tool(request).await.unwrap();
        let observed = json!({"capabilities":capabilities,"tools":tools,"resources":resources,"contents":contents,"result":result});
        let (client_stop, server_stop) = tokio::join!(client.cancel(), server.cancel());
        client_stop.unwrap();
        server_stop.unwrap();
        observed
    }).await.expect("in-memory peers finish and stop")
}

pub fn assert_catalog_contract(observed: &Value) {
    assert_eq!(
        observed["capabilities"]["extensions"]["io.modelcontextprotocol/ui"],
        json!({})
    );
    assert_eq!(
        observed["tools"][0]["_meta"],
        json!({
            "ui":{"resourceUri":"ui://parts/viewer"},
            "openai/ui":{"entrypoints":[{"type":"global"},{"type":"thread"}]}
        })
    );
    assert_eq!(observed["resources"].as_array().unwrap().len(), 1);
    assert_eq!(observed["resources"][0]["uri"], "ui://parts/viewer");
    assert_eq!(
        observed["resources"][0]["mimeType"],
        "text/html;profile=mcp-app"
    );
    let content = &observed["contents"]["contents"][0];
    assert_eq!(content["uri"], "ui://parts/viewer");
    assert_eq!(content["mimeType"], "text/html;profile=mcp-app");
    assert_eq!(
        content["text"],
        include_str!("../../examples/support/parts.html")
    );
    assert_eq!(
        content["_meta"],
        json!({
            "ui":{"csp":{"connectDomains":[],"resourceDomains":[]}},
            "openai/ui":{"availableDisplayModes":["inline","fullscreen"],"preferredDisplayMode":"inline"}
        })
    );
    assert_eq!(
        observed["result"]["structuredContent"],
        json!({"parts":["bolt","washer","nut"]})
    );
    assert!(!observed["result"]["content"].as_array().unwrap().is_empty());
}
