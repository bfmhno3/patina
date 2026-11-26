//! `patina-mcp`: an MCP server over stdio.
//!
//! It answers queries against the owner's Patina data, limited to the identities
//! listed in `[mcp] visible_identities`. What a client may write depends on its level in
//! `[mcp]`: nothing, captures and proposals the owner reviews, or direct commands.
#![forbid(unsafe_code)]

mod server;

use anyhow::{Context as _, Result};
use patina_core::engine::Patina;
use patina_core::store::{self, Store};
use rmcp::ServiceExt as _;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let store = Store::new(store::default_root()?, store::device_id()?);
    let handler = server::Handler::new(Patina::new(store));
    let running = handler
        .serve(rmcp::transport::stdio())
        .await
        .context("cannot start the MCP server")?;
    running.waiting().await.context("MCP server failed")?;
    Ok(())
}
