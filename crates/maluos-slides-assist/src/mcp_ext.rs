//! The MCP server with the fork's tools: wraps `maluos_slides_mcp::Server`, appends the extra
//! tools to `tools/list` and answers `tools/call` for them; everything else is upstream's.

use std::io::{BufRead, Write};

use maluos_slides_mcp::Backend;
use serde_json::{Value, json};

use crate::exec::call_tool;
use crate::tools::ToolTable;

pub struct Server {
    inner: maluos_slides_mcp::Server,
}

impl Server {
    pub fn new(backend: Box<dyn Backend>) -> Self {
        Server { inner: maluos_slides_mcp::Server::new(backend) }
    }

    pub fn backend(&mut self) -> &mut dyn Backend {
        self.inner.backend()
    }

    /// The stdio loop, as upstream's.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if let Some(reply) = self.handle_line(&line) {
                output.write_all(reply.as_bytes())?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(msg) => self.handle(msg).map(|v| v.to_string()),
            Err(_) => self.inner.handle_line(line),
        }
    }

    pub fn handle(&mut self, msg: Value) -> Option<Value> {
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();
        match method {
            "tools/list" => {
                let mut reply = self.inner.handle(msg)?;
                if let Some(tools) = reply.pointer_mut("/result/tools").and_then(Value::as_array_mut) {
                    tools.extend(ToolTable::extra_definitions());
                }
                Some(reply)
            }
            "tools/call" if ToolTable::is_extra(msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("")) => {
                let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("").to_string();
                let args = msg.pointer("/params/arguments").cloned().unwrap_or(json!({}));
                let r = call_tool(self.inner.backend(), &name, &args);
                Some(json!({"jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": r.to_value()}))
            }
            _ => self.inner.handle(msg),
        }
    }
}
