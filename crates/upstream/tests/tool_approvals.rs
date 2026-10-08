//! The policy cases of `tests/unit/tool_approvals.spec.ts`: which tools ask
//! for the approval of a person.

mod support;

use mymcps_core::models::{Mcp, McpTransport};
use mymcps_upstream::UpstreamTool;
use mymcps_upstream::approvals::{
    APPROVAL_NOTE, ToolApprovalChoice, ToolApprovalMode, assign_tool_approvals,
    default_tool_approval, saved_tool_approvals, tool_approval_mode, tool_approval_modes,
    with_approval_notes,
};
use serde_json::{Value, json};
use support::providers;

use ToolApprovalMode::{Ask, Auto};

fn mcp(tool_approvals: Option<&str>) -> Mcp {
    Mcp {
        name: "CRM".into(),
        transport: McpTransport::Http,
        tool_approvals: tool_approvals.map(str::to_owned),
        ..Default::default()
    }
}

fn google_ads(tool_approvals: Option<&str>) -> Mcp {
    Mcp {
        transport: McpTransport::Builtin,
        builtin_key: Some("google-ads".into()),
        ..mcp(tool_approvals)
    }
}

fn choice(name: &str, mode: ToolApprovalMode) -> ToolApprovalChoice {
    ToolApprovalChoice {
        name: name.to_owned(),
        mode,
    }
}

// Tool approvals: which tools ask

#[test]
fn lets_every_tool_run_until_the_admin_says_otherwise() {
    let builtins = providers::registry();
    assert_eq!(
        tool_approval_mode(&builtins, &mcp(None), "delete_contact"),
        Auto
    );
    assert_eq!(
        tool_approval_mode(
            &builtins,
            &mcp(Some(r#"{"delete_contact":"ask"}"#)),
            "delete_contact"
        ),
        Ask
    );
    assert_eq!(
        tool_approval_mode(
            &builtins,
            &mcp(Some(r#"{"delete_contact":"ask"}"#)),
            "list_contacts"
        ),
        Auto
    );
    // An empty column is nothing saved.
    assert_eq!(
        tool_approval_mode(&builtins, &mcp(Some("")), "delete_contact"),
        Auto
    );
}

#[test]
fn asks_by_default_for_the_built_in_tools_that_commit_money() {
    let builtins = providers::registry();
    let mut google_ads = google_ads(None);

    assert_eq!(
        default_tool_approval(&builtins, &google_ads, "update_campaign_budget"),
        Ask
    );
    assert_eq!(
        default_tool_approval(&builtins, &google_ads, "list_campaigns"),
        Auto
    );
    assert_eq!(
        tool_approval_mode(&builtins, &google_ads, "update_campaign_budget"),
        Ask
    );

    google_ads.tool_approvals = Some(r#"{"update_campaign_budget":"auto"}"#.into());
    assert_eq!(
        tool_approval_mode(&builtins, &google_ads, "update_campaign_budget"),
        Auto
    );
    // A tool the MCP does not have, or that is named like a property of every object.
    assert_eq!(
        tool_approval_mode(&builtins, &google_ads, "constructor"),
        Auto
    );
    assert_eq!(
        tool_approval_mode(&builtins, &google_ads, "__proto__"),
        Auto
    );

    // The same tool names on an MCP that is not this provider follow no default.
    assert_eq!(
        default_tool_approval(&builtins, &mcp(None), "update_campaign_budget"),
        Auto
    );
    let unknown = Mcp {
        builtin_key: Some("unknown".into()),
        ..mcp(None)
    };
    assert_eq!(
        default_tool_approval(&builtins, &unknown, "update_campaign_budget"),
        Auto
    );
}

#[test]
fn asks_for_every_tool_when_what_is_saved_cannot_be_read() {
    let builtins = providers::registry();
    for unreadable in ["{", "[]", "\"ask\"", r#"{"delete_contact":"sometimes"}"#] {
        let broken = mcp(Some(unreadable));
        assert_eq!(saved_tool_approvals(&broken), None, "{unreadable}");
        assert_eq!(
            tool_approval_mode(&builtins, &broken, "list_contacts"),
            Ask,
            "{unreadable}"
        );
        assert_eq!(
            tool_approval_modes(&builtins, &broken, ["a", "b"]),
            [("a", Ask), ("b", Ask)],
            "{unreadable}"
        );
    }
}

#[test]
fn saves_the_choices_that_differ_from_the_defaults_and_nothing_else() {
    let builtins = providers::registry();
    let mut google_ads = google_ads(None);
    assign_tool_approvals(
        &builtins,
        &mut google_ads,
        &[
            choice("update_campaign_budget", Ask),
            choice("set_campaign_status", Auto),
            choice("add_keywords", Ask),
            choice("list_campaigns", Auto),
        ],
    );
    assert_eq!(
        google_ads.tool_approvals.as_deref(),
        Some(r#"{"set_campaign_status":"auto","add_keywords":"ask"}"#)
    );

    assign_tool_approvals(
        &builtins,
        &mut google_ads,
        &[choice("update_campaign_budget", Ask)],
    );
    assert_eq!(google_ads.tool_approvals, None);
}

#[test]
fn reads_what_is_saved_by_tool_name_in_the_order_it_was_saved() {
    let saved = saved_tool_approvals(&mcp(Some(
        r#"{"zeta":"ask"," spaced ":"auto","":"ask","zeta":"auto"}"#,
    )))
    .unwrap();
    assert_eq!(saved.names().collect::<Vec<_>>(), ["zeta", " spaced ", ""]);
    // A name saved twice has its last value.
    assert_eq!(saved.get("zeta"), Some(Auto));
    assert_eq!(saved.get(" spaced "), Some(Auto));
    assert_eq!(saved.get(""), Some(Ask));
    assert_eq!(saved.get("spaced"), None);
    assert_eq!(saved.len(), 3);

    let nothing = saved_tool_approvals(&mcp(None)).unwrap();
    assert!(nothing.is_empty());
    assert_eq!(nothing.get("zeta"), None);
}

#[test]
fn reads_the_mode_of_several_tools_in_one_read_of_what_is_saved() {
    let builtins = providers::registry();
    let google_ads = google_ads(Some(r#"{"list_campaigns":"ask","add_keywords":"ask"}"#));
    assert_eq!(
        tool_approval_modes(
            &builtins,
            &google_ads,
            [
                "list_campaigns",
                "update_campaign_budget",
                "add_keywords",
                "unknown"
            ]
        ),
        [
            ("list_campaigns", Ask),
            ("update_campaign_budget", Ask),
            ("add_keywords", Ask),
            ("unknown", Auto),
        ]
    );
}

#[test]
fn tells_agents_which_tools_wait_for_a_person() {
    let builtins = providers::registry();
    let tool = |name: &str, description: Option<&str>| UpstreamTool {
        name: name.to_owned(),
        description: description.map(str::to_owned),
        input_schema: json!({ "type": "object" }),
    };
    let listed = vec![
        tool("list_contacts", Some("Lists contacts.")),
        tool("delete_contact", Some("Deletes a contact.")),
        tool("merge_contacts", None),
        tool("purge", Some("")),
    ];
    let saved = mcp(Some(
        r#"{"delete_contact":"ask","merge_contacts":"ask","purge":"ask"}"#,
    ));

    let noted = with_approval_notes(&builtins, &saved, listed.clone());
    assert_eq!(noted[0], listed[0]);
    assert_eq!(
        noted[1].description.as_deref(),
        Some(format!("Deletes a contact.\n\n{APPROVAL_NOTE}").as_str())
    );
    assert_eq!(noted[2].description.as_deref(), Some(APPROVAL_NOTE));
    assert_eq!(noted[3].description.as_deref(), Some(APPROVAL_NOTE));
    assert_eq!(noted[1].input_schema, listed[1].input_schema);

    // What cannot be read lets no tool run: every one says so.
    let noted = with_approval_notes(&builtins, &mcp(Some("{")), listed.clone());
    assert!(noted.iter().all(|tool| {
        tool.description
            .as_deref()
            .unwrap()
            .ends_with(APPROVAL_NOTE)
    }));
    assert_eq!(
        with_approval_notes(&builtins, &mcp(None), listed.clone()),
        listed
    );

    assert_eq!(
        APPROVAL_NOTE,
        "Needs approval: the first call is not run and returns a link for a person to approve in MyMCPs. Once they have, call the tool again with the same arguments."
    );
    let _: Value = serde_json::to_value(Ask).unwrap();
    assert_eq!(serde_json::to_value(Ask).unwrap(), json!("ask"));
    assert_eq!(
        serde_json::from_value::<ToolApprovalChoice>(json!({ "name": "purge", "mode": "auto" }))
            .unwrap(),
        choice("purge", Auto)
    );
}
