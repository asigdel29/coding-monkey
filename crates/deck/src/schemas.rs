/*
   File: crates/deck/src/schemas.rs

   Purpose
   WebSocket message schemas — validated server-side before any side
   effect. Mirrors `packages/deck/src/schemas.ts` exactly: any unknown
   shape is rejected with a structured reason that the audit log
   captures verbatim. No `to_string()` coercions on attacker input.

   The message set is small and stable; serde_json gets us 90% of the
   way there but we hand-validate string lengths and id charset
   because misshapen input is a security signal, not a parse error.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full Rust port from packages/deck/src/schemas.ts
   2026-06-09   Anubhav Sigdel  add agent.spawn/cancel/list (native agents)
*/

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Inbound WS message tagged by `type`. Each variant is a strict
/// shape — anything outside this enum is rejected by [`parse_ws_msg`].
///
/// Field-level docs are intentionally omitted: the variant docstring
/// describes intent, the field names mirror the wire protocol, and
/// `parse_ws_msg` is the authoritative validator for shape and bounds.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WsMsg {
    /// First message every client must send. Carries the session token.
    #[serde(rename = "auth")]
    Auth { token: String },
    /// Request the current tentacle list.
    #[serde(rename = "tentacle.list")]
    TentacleList,
    /// Create a new tentacle.
    #[serde(rename = "tentacle.create")]
    TentacleCreate {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<String>,
    },
    /// Remove a tentacle by id.
    #[serde(rename = "tentacle.remove")]
    TentacleRemove { id: String },
    /// List todos in a tentacle.
    #[serde(rename = "tentacle.todos")]
    TentacleTodos { id: String },
    /// Toggle a checkbox by 0-indexed line number.
    #[serde(rename = "tentacle.toggle")]
    TentacleToggle { id: String, line: usize },
    /// Read a tentacle's CONTEXT.md.
    #[serde(rename = "tentacle.context")]
    TentacleContext { id: String },
    /// Overwrite a tentacle's CONTEXT.md.
    #[serde(rename = "tentacle.writeContext")]
    TentacleWriteContext { id: String, content: String },
    /// Spawn a terminal.
    #[serde(rename = "term.spawn")]
    TermSpawn {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cmd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<Vec<String>>,
        #[serde(
            default,
            rename = "tentacleId",
            skip_serializing_if = "Option::is_none"
        )]
        tentacle_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cols: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rows: Option<u16>,
    },
    /// Forward keystrokes to a terminal.
    #[serde(rename = "term.input")]
    TermInput { id: String, data: String },
    /// Resize a terminal.
    #[serde(rename = "term.resize")]
    TermResize { id: String, cols: u16, rows: u16 },
    /// Kill a terminal.
    #[serde(rename = "term.kill")]
    TermKill { id: String },
    /// Spawn a native in-process agent for a task.
    #[serde(rename = "agent.spawn")]
    AgentSpawn {
        task: String,
        #[serde(
            default,
            rename = "tentacleId",
            skip_serializing_if = "Option::is_none"
        )]
        tentacle_id: Option<String>,
        #[serde(default, rename = "taskType", skip_serializing_if = "Option::is_none")]
        task_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tier: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        harness: Option<String>,
    },
    /// Cancel a running native agent by id.
    #[serde(rename = "agent.cancel")]
    AgentCancel { id: String },
    /// List running native agents.
    #[serde(rename = "agent.list")]
    AgentList,
}

const MAX_STR: usize = 64 * 1024;
const MAX_ID: usize = 256;
const MAX_TITLE: usize = 512;
/// Max length of a native agent's task prompt.
const MAX_TASK: usize = 16 * 1024;

static ID_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[A-Za-z0-9._\-]+$").expect("valid id regex"));

/// Validate a raw `serde_json::Value` against the strict schema. Returns
/// a structured error so the audit logger can record the rejection
/// reason without echoing attacker input verbatim.
pub fn parse_ws_msg(raw: &Value) -> Result<WsMsg, String> {
    let obj = raw.as_object().ok_or_else(|| "not-object".to_string())?;
    let kind = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "bad-type".to_string())?;
    if kind.len() > 64 {
        return Err("bad-type".into());
    }

    let str_field = |name: &str, min: usize, max: usize| -> Result<String, String> {
        let v = obj
            .get(name)
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("{kind}.{name}"))?;
        if v.len() < min || v.len() > max {
            return Err(format!("{kind}.{name}"));
        }
        Ok(v.to_string())
    };
    let opt_str = |name: &str, min: usize, max: usize| -> Result<Option<String>, String> {
        match obj.get(name) {
            None => Ok(None),
            Some(v) if v.is_null() => Ok(None),
            Some(v) => {
                let s = v.as_str().ok_or_else(|| format!("{kind}.{name}"))?;
                if s.len() < min || s.len() > max {
                    return Err(format!("{kind}.{name}"));
                }
                Ok(Some(s.to_string()))
            }
        }
    };
    let id_field = |name: &str| -> Result<String, String> {
        let s = obj
            .get(name)
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("{kind}.{name}"))?;
        if s.is_empty() || s.len() > MAX_ID || !ID_RE.is_match(s) {
            return Err(format!("{kind}.{name}"));
        }
        Ok(s.to_string())
    };
    let opt_id = |name: &str| -> Result<Option<String>, String> {
        match obj.get(name) {
            None => Ok(None),
            Some(v) if v.is_null() => Ok(None),
            Some(v) => {
                let s = v.as_str().ok_or_else(|| format!("{kind}.{name}"))?;
                if s.is_empty() || s.len() > MAX_ID || !ID_RE.is_match(s) {
                    return Err(format!("{kind}.{name}"));
                }
                Ok(Some(s.to_string()))
            }
        }
    };
    let num_field = |name: &str, min: i64, max: i64| -> Result<i64, String> {
        let v = obj
            .get(name)
            .and_then(|v| v.as_i64())
            .ok_or_else(|| format!("{kind}.{name}"))?;
        if v < min || v > max {
            return Err(format!("{kind}.{name}"));
        }
        Ok(v)
    };
    let opt_num = |name: &str, min: i64, max: i64| -> Result<Option<i64>, String> {
        match obj.get(name) {
            None => Ok(None),
            Some(v) if v.is_null() => Ok(None),
            Some(v) => {
                let n = v.as_i64().ok_or_else(|| format!("{kind}.{name}"))?;
                if n < min || n > max {
                    return Err(format!("{kind}.{name}"));
                }
                Ok(Some(n))
            }
        }
    };

    let msg = match kind {
        "auth" => WsMsg::Auth {
            token: str_field("token", 16, 256)?,
        },
        "tentacle.list" => WsMsg::TentacleList,
        "tentacle.create" => WsMsg::TentacleCreate {
            title: str_field("title", 1, MAX_TITLE)?,
            context: opt_str("context", 0, MAX_STR)?,
        },
        "tentacle.remove" => WsMsg::TentacleRemove {
            id: id_field("id")?,
        },
        "tentacle.todos" => WsMsg::TentacleTodos {
            id: id_field("id")?,
        },
        "tentacle.toggle" => {
            let id = id_field("id")?;
            let line = num_field("line", 0, 100_000)? as usize;
            WsMsg::TentacleToggle { id, line }
        }
        "tentacle.context" => WsMsg::TentacleContext {
            id: id_field("id")?,
        },
        "tentacle.writeContext" => WsMsg::TentacleWriteContext {
            id: id_field("id")?,
            content: str_field("content", 0, MAX_STR)?,
        },
        "term.spawn" => {
            let cmd = opt_str("cmd", 1, 256)?;
            let tentacle_id = opt_id("tentacleId")?;
            let cols = opt_num("cols", 1, 1024)?.map(|n| n as u16);
            let rows = opt_num("rows", 1, 1024)?.map(|n| n as u16);
            let args = match obj.get("args") {
                None => None,
                Some(v) if v.is_null() => None,
                Some(v) => {
                    let arr = v.as_array().ok_or_else(|| "term.spawn.args".to_string())?;
                    if arr.len() > 64 {
                        return Err("term.spawn.args".into());
                    }
                    let mut out = Vec::with_capacity(arr.len());
                    for a in arr {
                        let s = a.as_str().ok_or_else(|| "term.spawn.args[]".to_string())?;
                        if s.len() > 1024 {
                            return Err("term.spawn.args[]".into());
                        }
                        out.push(s.to_string());
                    }
                    Some(out)
                }
            };
            WsMsg::TermSpawn {
                cmd,
                args,
                tentacle_id,
                cols,
                rows,
            }
        }
        "term.input" => WsMsg::TermInput {
            id: id_field("id")?,
            data: str_field("data", 0, MAX_STR)?,
        },
        "term.resize" => WsMsg::TermResize {
            id: id_field("id")?,
            cols: num_field("cols", 1, 1024)? as u16,
            rows: num_field("rows", 1, 1024)? as u16,
        },
        "term.kill" => WsMsg::TermKill {
            id: id_field("id")?,
        },
        "agent.spawn" => WsMsg::AgentSpawn {
            task: str_field("task", 1, MAX_TASK)?,
            tentacle_id: opt_id("tentacleId")?,
            task_type: opt_str("taskType", 1, 64)?,
            tier: opt_str("tier", 1, 32)?,
            harness: opt_str("harness", 1, 64)?,
        },
        "agent.cancel" => WsMsg::AgentCancel {
            id: id_field("id")?,
        },
        "agent.list" => WsMsg::AgentList,
        _ => return Err("unknown-type".into()),
    };
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_auth_with_valid_token() {
        let v = json!({ "type": "auth", "token": "0123456789abcdef" });
        let m = parse_ws_msg(&v).unwrap();
        assert!(matches!(m, WsMsg::Auth { .. }));
    }

    #[test]
    fn rejects_short_token() {
        let v = json!({ "type": "auth", "token": "short" });
        assert!(parse_ws_msg(&v).is_err());
    }

    #[test]
    fn rejects_unknown_type() {
        let v = json!({ "type": "rce.exec" });
        assert_eq!(parse_ws_msg(&v).err().as_deref(), Some("unknown-type"));
    }

    #[test]
    fn rejects_id_with_path_traversal() {
        let v = json!({ "type": "tentacle.remove", "id": "../etc/passwd" });
        assert!(parse_ws_msg(&v).is_err());
    }

    #[test]
    fn parses_term_spawn_with_optional_fields_omitted() {
        let v = json!({ "type": "term.spawn" });
        let m = parse_ws_msg(&v).unwrap();
        match m {
            WsMsg::TermSpawn {
                cmd,
                args,
                tentacle_id,
                cols,
                rows,
            } => {
                assert!(cmd.is_none() && args.is_none() && tentacle_id.is_none());
                assert!(cols.is_none() && rows.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_term_resize_with_bounds() {
        let v = json!({ "type": "term.resize", "id": "term_abc", "cols": 80, "rows": 24 });
        match parse_ws_msg(&v).unwrap() {
            WsMsg::TermResize { cols, rows, .. } => {
                assert_eq!(cols, 80);
                assert_eq!(rows, 24);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn rejects_oversize_args_array() {
        let big_args: Vec<String> = (0..200).map(|i| format!("a{i}")).collect();
        let v = json!({ "type": "term.spawn", "args": big_args });
        assert!(parse_ws_msg(&v).is_err());
    }

    #[test]
    fn parses_agent_spawn_minimal() {
        let v = json!({ "type": "agent.spawn", "task": "fix the bug" });
        match parse_ws_msg(&v).unwrap() {
            WsMsg::AgentSpawn {
                task,
                tentacle_id,
                harness,
                ..
            } => {
                assert_eq!(task, "fix the bug");
                assert!(tentacle_id.is_none() && harness.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_agent_spawn_with_options() {
        let v = json!({
            "type": "agent.spawn", "task": "t", "tentacleId": "main",
            "taskType": "review", "tier": "powerful", "harness": "native"
        });
        match parse_ws_msg(&v).unwrap() {
            WsMsg::AgentSpawn {
                tentacle_id,
                task_type,
                tier,
                harness,
                ..
            } => {
                assert_eq!(tentacle_id.as_deref(), Some("main"));
                assert_eq!(task_type.as_deref(), Some("review"));
                assert_eq!(tier.as_deref(), Some("powerful"));
                assert_eq!(harness.as_deref(), Some("native"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn rejects_agent_spawn_without_task() {
        let v = json!({ "type": "agent.spawn" });
        assert!(parse_ws_msg(&v).is_err());
    }

    #[test]
    fn parses_agent_cancel_and_list() {
        assert!(matches!(
            parse_ws_msg(&json!({ "type": "agent.cancel", "id": "agent_abc" })).unwrap(),
            WsMsg::AgentCancel { .. }
        ));
        assert!(matches!(
            parse_ws_msg(&json!({ "type": "agent.list" })).unwrap(),
            WsMsg::AgentList
        ));
    }
}
