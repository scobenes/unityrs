//! Model-facing discovery must explain the server, tools and arguments.
use super::mcp_support::{initialize, session};
use serde_json::{Value, json};

fn descriptive(name: &str, value: &Value) {
    let description = value.as_str().unwrap_or_default();
    assert!(
        description.split_whitespace().count() >= 4,
        "{name}: missing or too short: {description:?}"
    );
    let normalized = |text: &str| {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    assert_ne!(normalized(name), normalized(description), "{name}");
}

#[test]
fn initialize_instructions_are_documented() {
    session(&[], |s| {
        let init = s.request("initialize", initialize("2025-11-25"));
        let instructions = &init["result"]["instructions"];
        descriptive("instructions", instructions);
        let text = instructions.as_str().unwrap();
        for term in [
            "Unity",
            "unityrs_inspect",
            "unityrs_show",
            "unityrs_extract",
            "format",
        ] {
            assert!(text.contains(term), "missing {term}: {text}");
        }
        assert!(include_str!("../../../../docs/AGENTS.md").contains(text));
    });
}

#[test]
fn tools_and_properties_have_descriptions() {
    session(&["--out", "out"], |s| {
        s.ready();
        let list = s.request("tools/list", json!({}));
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 5);
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            descriptive(name, &tool["description"]);
            if name == "unityrs_extract" {
                assert!(
                    tool["description"]
                        .as_str()
                        .unwrap()
                        .contains("no_container")
                );
                // A false switch only omits its flag, so the CLI's container
                // aliases would be no-ops here: one switch, `no_container`.
                let properties = tool["inputSchema"]["properties"].as_object().unwrap();
                assert!(properties.contains_key("no_container"));
                assert!(!properties.contains_key("use_container"));
                assert!(!properties.contains_key("no_use_container"));
            }
            for (key, property) in tool["inputSchema"]["properties"].as_object().unwrap() {
                descriptive(key, &property["description"]);
            }
        }
    });
}
