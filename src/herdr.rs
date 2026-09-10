use std::{
    env,
    process::{Command, Stdio},
    thread,
};

use serde_json::Value;

use crate::{config::NotificationsConfig, paths::expand_path};

pub(crate) fn herdr_bin() -> String {
    env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into())
}
pub(crate) fn herdr_json<const N: usize>(args: [&str; N]) -> Result<Value, String> {
    let out = Command::new(herdr_bin())
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}
pub(crate) fn focus_agent(target: &str) -> Result<(), String> {
    focus_agent_with(target, herdr_json)
}

fn focus_agent_with(
    target: &str,
    mut request: impl FnMut([&str; 3]) -> Result<Value, String>,
) -> Result<(), String> {
    let focused = request(["agent", "focus", target])?;
    let tab = focused
        .pointer("/result/agent/tab_id")
        .and_then(Value::as_str)
        .filter(|tab| !tab.is_empty())
        .ok_or_else(|| "Herdr did not return the focused agent's tab".to_string())?;
    // agent.focus changes server selection, but Herdr 0.8/0.9 only projects
    // explicit tab/workspace/pane focus requests into visible shell clients.
    // Use the returned live tab, not the potentially stale picker entry.
    request(["tab", "focus", tab]).map(|_| ())
}

pub(crate) fn run_herdr<const N: usize>(args: [&str; N]) -> Result<(), String> {
    let status = Command::new(herdr_bin())
        .args(args)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("herdr exited with {status}"))
    }
}

pub(crate) fn run_herdr_quiet<const N: usize>(args: [&str; N]) -> Result<(), String> {
    let mut command = Command::new(herdr_bin());
    command.args(args);
    run_command_quiet(&mut command)
}

fn run_command_quiet(command: &mut Command) -> Result<(), String> {
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(format!("herdr exited with {}", output.status))
    } else {
        Err(stderr)
    }
}

pub(crate) fn notify_done(body: &str, config: &NotificationsConfig) {
    notify(body, "done", config);
}

pub(crate) fn notify_error(body: &str, config: &NotificationsConfig) {
    notify(body, "request", config);
}

fn notify(body: &str, default_sound: &'static str, config: &NotificationsConfig) {
    if !config.enabled {
        return;
    }
    let body = truncate(body, 180);
    let (sound, custom_sound) = notification_audio(config, default_sound);
    let mut command = Command::new(herdr_bin());
    command.args([
        "notification",
        "show",
        "Herdr Navigator",
        "--body",
        &body,
        "--position",
        "top-right",
        "--sound",
        sound,
    ]);
    let _ = run_command_quiet(&mut command);
    if let Some(path) = custom_sound {
        play_custom_sound(path);
    }
}

fn notification_audio<'a>(
    config: &'a NotificationsConfig,
    default_sound: &'static str,
) -> (&'static str, Option<&'a str>) {
    if !config.audio {
        return ("none", None);
    }
    match config.sound.trim().to_ascii_lowercase().as_str() {
        "default" => (default_sound, None),
        "custom" => (
            "none",
            config
                .custom_sound
                .as_deref()
                .filter(|path| !path.is_empty()),
        ),
        _ => ("none", None),
    }
}

fn play_custom_sound(path: &str) {
    let path = expand_path(path);
    if !path.is_file() {
        return;
    }
    thread::spawn(move || {
        #[cfg(target_os = "macos")]
        let players: &[(&str, &[&str])] = &[("afplay", &[])];
        #[cfg(not(target_os = "macos"))]
        let players: &[(&str, &[&str])] = &[("pw-play", &[]), ("paplay", &[]), ("aplay", &[])];

        for (player, args) in players {
            let status = Command::new(player)
                .args(*args)
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if status.is_ok_and(|status| status.success()) {
                break;
            }
        }
    });
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut out: String = value.chars().take(max_chars).collect();
    if value.chars().count() > max_chars {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NotificationsConfig;

    #[test]
    fn notification_audio_resolves_silent_default_herdr_and_custom() {
        let silent = NotificationsConfig::default();
        assert_eq!(notification_audio(&silent, "request"), ("none", None));

        let herdr = NotificationsConfig {
            enabled: true,
            audio: true,
            sound: "default".into(),
            custom_sound: None,
        };
        assert_eq!(notification_audio(&herdr, "done"), ("done", None));

        let custom = NotificationsConfig {
            enabled: true,
            audio: true,
            sound: "custom".into(),
            custom_sound: Some("~/sounds/navigator.wav".into()),
        };
        assert_eq!(
            notification_audio(&custom, "done"),
            ("none", Some("~/sounds/navigator.wav"))
        );
    }

    #[test]
    fn agent_selection_activates_the_live_tab_in_visible_clients() {
        let mut calls = Vec::new();
        focus_agent_with("w1:p2", |args| {
            calls.push(args.map(str::to_string));
            Ok(serde_json::json!({"result":{"agent":{"tab_id":"w2:t9"}}}))
        })
        .unwrap();
        assert_eq!(
            calls,
            vec![
                ["agent", "focus", "w1:p2"].map(str::to_string),
                ["tab", "focus", "w2:t9"].map(str::to_string),
            ]
        );
    }

    #[test]
    fn failed_agent_focus_does_not_activate_a_tab() {
        let mut calls = 0;
        let result = focus_agent_with("w1:p2", |_| {
            calls += 1;
            Err("agent no longer exists".into())
        });
        assert_eq!(result, Err("agent no longer exists".into()));
        assert_eq!(calls, 1);
    }

    #[test]
    fn agent_selection_reports_missing_tab_and_activation_failures() {
        for tab_id in [Value::Null, Value::String(String::new())] {
            let mut calls = 0;
            let result = focus_agent_with("w1:p2", |_| {
                calls += 1;
                Ok(serde_json::json!({"result":{"agent":{"tab_id":tab_id}}}))
            });
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
        let result = focus_agent_with("w1:p2", |args| {
            if args[0] == "tab" {
                Err("tab closed".into())
            } else {
                Ok(serde_json::json!({"result":{"agent":{"tab_id":"w1:t1"}}}))
            }
        });
        assert_eq!(result, Err("tab closed".into()));
    }

    #[test]
    fn quiet_command_captures_child_output() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf ignored; printf failure >&2; exit 7"]);

        assert_eq!(run_command_quiet(&mut command), Err("failure".into()));
    }
}
