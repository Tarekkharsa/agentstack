//! `agentstack explain` surfaces the trust facts for a capability: provenance,
//! the secrets it needs and whether they resolve, and its safety signals.

use std::fs;

use agentstack::commands::explain::explain_text;

#[test]
fn explain_server_reports_secret_and_safety() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://kibana.example/mcp\"\n\
         headers = { Authorization = \"Bearer ${ZZ_UNSET_TOKEN}\" }\n",
    )
    .unwrap();

    let out = explain_text("kibana", Some(&proj)).unwrap();
    assert!(out.contains("MCP server · http"));
    assert!(out.contains("kibana.example"), "shows the endpoint host");
    assert!(out.contains("${ZZ_UNSET_TOKEN}") && out.contains("not set"));
    assert!(out.contains("network egress"));

    // Unknown capability → a helpful error, not a panic.
    assert!(explain_text("nope-not-here", Some(&proj)).is_err());
}
