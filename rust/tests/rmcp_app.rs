//! The core SDK can serve an MCP App without the optional helper dependency.
#[path = "support/app_contract.rs"]
mod contract;
#[path = "../examples/rmcp_app.rs"]
mod example;

#[tokio::test]
async fn direct_rmcp_example_serves_the_application_contract() {
    let observed = contract::observe(example::CatalogServer::new().unwrap()).await;
    contract::assert_catalog_contract(&observed);
}
