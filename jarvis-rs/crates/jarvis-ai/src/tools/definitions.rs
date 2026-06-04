//! Built-in tool definitions and format converters.

use crate::ToolDefinition;

/// Create the built-in tool definitions that Jarvis exposes to AI models.
pub fn builtin_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "run_command".to_string(),
            description: "Execute a shell command and return its output.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "working_directory": {
                        "type": "string",
                        "description": "Working directory for the command (optional)"
                    }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read the contents of a file.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative path to the file"
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "write_file".to_string(),
            description: "Write content to a file, creating it if it doesn't exist.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write"
                    }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDefinition {
            name: "search_files".to_string(),
            description: "Search for files matching a pattern using glob.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern (e.g., '**/*.rs')"
                    },
                    "directory": {
                        "type": "string",
                        "description": "Root directory to search from"
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: "search_content".to_string(),
            description: "Search file contents for a regex pattern.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "directory": {
                        "type": "string",
                        "description": "Root directory to search from"
                    },
                    "file_pattern": {
                        "type": "string",
                        "description": "Glob pattern to filter files (e.g., '*.rs')"
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: "list_directory".to_string(),
            description: "List files and directories at a given path.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory path"
                    }
                },
                "required": ["path"]
            }),
        },
    ]
}

/// The read-only subset of the built-in tools, safe to expose to an assistant
/// that must have ZERO command-execution / write capability.
///
/// Excludes `run_command` and `write_file` entirely, so the model cannot even
/// request them.
pub fn read_only_tools() -> Vec<ToolDefinition> {
    const READ_ONLY: &[&str] = &[
        "read_file",
        "search_files",
        "search_content",
        "list_directory",
    ];
    builtin_tools()
        .into_iter()
        .filter(|t| READ_ONLY.contains(&t.name.as_str()))
        .collect()
}

/// Convert a tool definition to the Claude API format.
pub fn to_claude_tool(tool: &ToolDefinition) -> serde_json::Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.parameters,
    })
}

/// Convert a tool definition to the OpenAI Chat Completions function format.
///
/// OpenAI nests the tool under `{type:"function", function:{...}}` and uses
/// `parameters` (a JSON Schema object) where Claude uses `input_schema`.
pub fn to_openai_tool(tool: &ToolDefinition) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters,
        },
    })
}

/// Convert a tool definition to the Gemini `functionDeclarations[]` entry format.
///
/// Gemini groups declarations under `tools:[{functionDeclarations:[...]}]`. Each
/// declaration carries `name`, `description`, and `parameters` (a JSON Schema
/// object — the SAME shape Jarvis stores), so this is a near-passthrough.
pub fn to_gemini_tool(tool: &ToolDefinition) -> serde_json::Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.parameters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_tools_exclude_dangerous_tools() {
        let names: Vec<String> = read_only_tools().into_iter().map(|t| t.name).collect();
        assert!(!names.contains(&"run_command".to_string()));
        assert!(!names.contains(&"write_file".to_string()));
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"list_directory".to_string()));
    }

    #[test]
    fn to_openai_tool_nests_under_function() {
        let tool = &read_only_tools()[0];
        let v = to_openai_tool(tool);
        assert_eq!(v["type"], "function");
        assert_eq!(v["function"]["name"], tool.name);
        assert_eq!(v["function"]["description"], tool.description);
        assert_eq!(v["function"]["parameters"], tool.parameters);
    }

    #[test]
    fn to_gemini_tool_is_flat_declaration() {
        let tool = &read_only_tools()[0];
        let v = to_gemini_tool(tool);
        assert_eq!(v["name"], tool.name);
        assert_eq!(v["description"], tool.description);
        assert_eq!(v["parameters"], tool.parameters);
        // Gemini does not wrap in {type:"function", function:{...}}.
        assert!(v.get("type").is_none());
        assert!(v.get("function").is_none());
    }
}
