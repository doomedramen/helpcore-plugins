use anyhow::Context;
use reqwest::Client;

/// POST to helpcore's SSE chat endpoint and collect the full reply.
/// Returns `(reply_text, conversation_id)`.
pub async fn chat(
    helpcore_url: &str,
    token: &str,
    message: &str,
    conversation_id: Option<&str>,
) -> anyhow::Result<(String, Option<String>)> {
    let mut payload = serde_json::json!({ "message": message });
    if let Some(id) = conversation_id {
        payload["conversation_id"] = serde_json::Value::String(id.to_string());
    }

    let body = Client::new()
        .post(format!("{helpcore_url}/api/chat"))
        .bearer_auth(token)
        .json(&payload)
        .send()
        .await
        .context("failed to reach helpcore")?
        .error_for_status()
        .context("helpcore returned an error")?
        .text()
        .await
        .context("failed to read helpcore response")?;

    let mut reply = String::new();
    let mut out_conv_id: Option<String> = None;

    for block in body.split("\n\n") {
        let mut event = String::new();
        let mut data = String::new();
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("event:") {
                event = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("data:") {
                data = v.trim().to_string();
            }
        }
        match event.as_str() {
            "started" => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                    out_conv_id = v
                        .get("conversation_id")
                        .and_then(|id| id.as_str())
                        .map(String::from);
                }
            }
            "chunk" => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                    if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                        reply.push_str(delta);
                    }
                }
            }
            _ => {}
        }
    }

    if reply.is_empty() {
        anyhow::bail!("helpcore returned no text");
    }

    Ok((reply, out_conv_id))
}
