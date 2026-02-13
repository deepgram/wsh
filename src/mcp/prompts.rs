// MCP prompt handlers
//
// Exposes wsh skills as MCP prompts. Each skill is a markdown document
// that teaches AI agents patterns and strategies for using wsh.

use rmcp::model::*;

struct SkillDef {
    name: &'static str,
    description: &'static str,
    path: &'static str,
}

const SKILLS: &[SkillDef] = &[
    SkillDef {
        name: "wsh:core",
        description: "API primitives and the send/wait/read/decide loop (MCP-adapted)",
        path: "skills/wsh/core-mcp/SKILL.md",
    },
    SkillDef {
        name: "wsh:drive-process",
        description: "Running CLI commands, handling prompts, command-response workflows",
        path: "skills/wsh/drive-process/SKILL.md",
    },
    SkillDef {
        name: "wsh:tui",
        description: "Operating full-screen terminal applications (vim, htop, lazygit)",
        path: "skills/wsh/tui/SKILL.md",
    },
    SkillDef {
        name: "wsh:multi-session",
        description: "Parallel session orchestration",
        path: "skills/wsh/multi-session/SKILL.md",
    },
    SkillDef {
        name: "wsh:agent-orchestration",
        description: "Driving other AI agents through terminal interfaces",
        path: "skills/wsh/agent-orchestration/SKILL.md",
    },
    SkillDef {
        name: "wsh:monitor",
        description: "Watching and reacting to terminal activity",
        path: "skills/wsh/monitor/SKILL.md",
    },
    SkillDef {
        name: "wsh:visual-feedback",
        description: "Using overlays and panels to communicate with users",
        path: "skills/wsh/visual-feedback/SKILL.md",
    },
    SkillDef {
        name: "wsh:input-capture",
        description: "Capturing keyboard input for dialogs and approvals",
        path: "skills/wsh/input-capture/SKILL.md",
    },
    SkillDef {
        name: "wsh:generative-ui",
        description: "Building dynamic interactive terminal experiences",
        path: "skills/wsh/generative-ui/SKILL.md",
    },
];

pub async fn list_prompts() -> Result<ListPromptsResult, ErrorData> {
    let prompts = SKILLS
        .iter()
        .map(|s| Prompt::new(s.name, Some(s.description), None))
        .collect();

    Ok(ListPromptsResult {
        prompts,
        next_cursor: None,
        meta: None,
    })
}

pub async fn get_prompt(name: &str) -> Result<GetPromptResult, ErrorData> {
    let skill = SKILLS
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| {
            ErrorData::invalid_params(format!("unknown prompt: {name}"), None)
        })?;

    let content = std::fs::read_to_string(skill.path).map_err(|e| {
        ErrorData::internal_error(
            format!("failed to read skill file: {e}"),
            None,
        )
    })?;

    Ok(GetPromptResult {
        description: Some(skill.description.to_string()),
        messages: vec![PromptMessage::new_text(
            PromptMessageRole::User,
            content,
        )],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_prompts_returns_nine_prompts() {
        let result = list_prompts().await.unwrap();
        assert_eq!(result.prompts.len(), 9);
    }

    #[tokio::test]
    async fn list_prompts_has_expected_names() {
        let result = list_prompts().await.unwrap();
        let names: Vec<&str> = result.prompts.iter().map(|p| p.name.as_str()).collect();

        assert!(names.contains(&"wsh:core"));
        assert!(names.contains(&"wsh:drive-process"));
        assert!(names.contains(&"wsh:tui"));
        assert!(names.contains(&"wsh:multi-session"));
        assert!(names.contains(&"wsh:agent-orchestration"));
        assert!(names.contains(&"wsh:monitor"));
        assert!(names.contains(&"wsh:visual-feedback"));
        assert!(names.contains(&"wsh:input-capture"));
        assert!(names.contains(&"wsh:generative-ui"));
    }

    #[tokio::test]
    async fn list_prompts_all_have_descriptions() {
        let result = list_prompts().await.unwrap();
        for prompt in &result.prompts {
            assert!(
                prompt.description.is_some(),
                "prompt {} should have a description",
                prompt.name
            );
        }
    }

    #[tokio::test]
    async fn get_prompt_core_returns_content() {
        // This test reads from disk — requires running from project root
        let result = get_prompt("wsh:core").await.unwrap();
        assert!(result.description.is_some());
        assert_eq!(result.messages.len(), 1);

        let msg = &result.messages[0];
        assert_eq!(msg.role, PromptMessageRole::User);

        match &msg.content {
            PromptMessageContent::Text { text } => {
                assert!(
                    text.contains("wsh:core-mcp"),
                    "core prompt should contain 'wsh:core-mcp' (from frontmatter)"
                );
                assert!(
                    text.contains("wsh_run_command"),
                    "MCP-adapted core skill should reference wsh_run_command"
                );
            }
            _ => panic!("expected text content"),
        }
    }

    #[tokio::test]
    async fn get_prompt_nonexistent_returns_error() {
        let result = get_prompt("nonexistent").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn get_prompt_drive_process_returns_content() {
        let result = get_prompt("wsh:drive-process").await.unwrap();
        assert!(result.description.is_some());
        assert_eq!(result.messages.len(), 1);

        match &result.messages[0].content {
            PromptMessageContent::Text { text } => {
                assert!(!text.is_empty(), "drive-process skill should not be empty");
            }
            _ => panic!("expected text content"),
        }
    }
}
