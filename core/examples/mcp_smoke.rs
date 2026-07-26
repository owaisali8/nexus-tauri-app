//! Live check: connect to a real MCP server and call one of its tools.
//!
//! Needs `npx` on PATH; the first run downloads the server package.
//!
//! ```bash
//! cargo run -p nexus-core --example mcp_smoke
//! ```

use nexus_core::tools::{
    AutoApprove, Effect, RunContext, ToolCall, ToolRegistry,
    mcp::{McpManager, McpServerConfig},
};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = McpServerConfig {
        id: "memory".to_string(),
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-memory".to_string(),
        ],
        env: HashMap::new(),
        enabled: true,
    };

    println!(
        "== connecting: {} {} ==",
        config.command,
        config.args.join(" ")
    );
    let (manager, failures) = McpManager::connect_all(&[config]).await;

    for failure in &failures {
        println!("  FAILED: {failure}");
    }
    if manager.server_ids().is_empty() {
        return Err("no MCP server connected".into());
    }
    println!("  connected: {:?}", manager.server_ids());

    let mut registry = ToolRegistry::new();
    manager.register_into(&mut registry);

    let specs = registry.specs();
    println!("\n== tools ({}) ==", specs.len());
    for spec in &specs {
        println!("  {}", spec.name);
        // Every MCP tool must be gated regardless of what the server claims.
        assert_eq!(
            registry.effect_of(&spec.name),
            Some(Effect::SideEffecting),
            "{} is not gated",
            spec.name
        );
    }
    assert!(!specs.is_empty(), "the server exposed no tools");

    // create_entities is the memory server's canonical write.
    let target = specs
        .iter()
        .find(|spec| spec.name.ends_with("create_entities"))
        .ok_or("expected a create_entities tool")?;

    println!("\n== calling {} ==", target.name);
    let outcome = registry
        .invoke(
            &RunContext::new("smoke-session", "smoke-run"),
            &ToolCall {
                id: "call-1".to_string(),
                name: target.name.clone(),
                arguments: serde_json::json!({
                    "entities": [{
                        "name": "nexus",
                        "entityType": "project",
                        "observations": ["verified the MCP round trip"]
                    }]
                }),
            },
            // Read-only in effect here, and this must stay non-interactive.
            &AutoApprove,
        )
        .await;

    println!("  ok     : {}", outcome.ok);
    println!("  output : {}", outcome.output);

    assert!(outcome.ok, "the tool call failed: {}", outcome.output);
    println!("\nOK — MCP connect, tools/list, and tools/call verified.");

    Ok(())
}
