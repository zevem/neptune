//! The adapters an SSH host is given: a script the bootstrap evaluates there,
//! made from the same hook and batch-argument tables as the local adapters.
//! The host needs a POSIX shell and nothing of Neptune's.
use super::agents::{
    OPENCODE_PLUGIN, PI_EXTENSION, batch_arguments, claude_hooks, codex_hooks, quote,
};
use neptune_model::AgentKind;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

const TEMPLATE: &str = include_str!("agent-remote.sh");
/// What a hook on the host runs; the adapter names its directory for it.
const HOOK: &str = "sh \"$NEPTUNE_AGENT_DIR/hook\"";

/// The script, the same for every terminal: the credential is its argument.
pub(super) fn installer() -> &'static str {
    static SCRIPT: OnceLock<String> = OnceLock::new();
    SCRIPT.get_or_init(render)
}

fn render() -> String {
    let batch = AgentKind::ALL
        .into_iter()
        .flat_map(|kind| {
            batch_arguments(kind).map(move |word| format!("{}:{word}", kind.executable()))
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let codex = codex_hooks(HOOK)
        .iter()
        .map(|hook| format!("-c {}", quote(hook)))
        .collect::<Vec<_>>()
        .join(" ");
    let names = AgentKind::ALL
        .into_iter()
        .map(AgentKind::executable)
        .collect::<Vec<_>>()
        .join(" ");
    let claude = serde_json::json!({ "hooks": claude_hooks(HOOK) }).to_string();
    let script = TEMPLATE
        .replace("@BATCH@", &batch)
        .replace("@CODEX_HOOKS@", &codex)
        .replace("@CLAUDE_SETTINGS@", &claude)
        .replace("@NAMES@", &names)
        .replace("@OPENCODE_PLUGIN@", OPENCODE_PLUGIN.trim_end())
        .replace("@PI_EXTENSION@", PI_EXTENSION.trim_end());
    // The host keeps one installation and replaces it when this text changes.
    let stamp = hex::encode(&Sha256::digest(script.as_bytes())[..8]);
    script.replace("@STAMP@", &stamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installer_is_made_from_the_local_adapters_tables() {
        let script = installer();
        for placeholder in ["@BATCH@", "@STAMP@", "@NAMES@", "@CODEX_HOOKS@", "_PLUGIN@"] {
            assert!(!script.contains(placeholder), "{placeholder} was left");
        }
        for kind in AgentKind::ALL {
            assert!(script.contains(&format!("{}:--help", kind.executable())));
        }
        assert!(script.contains("for agent in claude codex opencode gemini pi omp;"));
        assert!(script.contains("claude:--print") && script.contains("codex:exec"));
        // Every hook the local adapters add, run through the host's hook.
        for event in ["UserPromptSubmit", "PermissionRequest", "Stop"] {
            assert!(script.contains(&format!("hook\\\" {event}\"")), "{event}");
        }
        assert!(script.contains("hooks.Interrupt="));
        // Nothing of this machine: no path of this executable, no helper.
        assert!(!script.contains("--agent-hook") && !script.contains("NEPTUNE_AGENT_HELPER"));
        assert!(script.contains("export const Neptune") && script.contains("pi.on("));
    }
}
