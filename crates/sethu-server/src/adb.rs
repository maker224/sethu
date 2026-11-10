//! Android device control through `adb`.
//!
//! Screenshots use `exec-out screencap -p`; input uses the device's `input`
//! command. This deliberately avoids scrcpy/H.264 so it works with any stock
//! Android build, at the cost of a low frame rate.

use std::{process::Stdio, sync::Arc, time::Duration};

use bytes::Bytes;
use tokio::{process::Command, time::timeout};

use crate::{error::AppError, util::png_size};

#[derive(Clone)]
pub struct AdbClient {
    program: Arc<str>,
    serial: Arc<str>,
    timeout: Duration,
}

impl AdbClient {
    pub fn new(program: &str, serial: &str, timeout: Duration) -> Self {
        Self {
            program: Arc::from(program),
            serial: Arc::from(serial),
            timeout,
        }
    }

    async fn run(&self, args: Vec<String>) -> Result<Vec<u8>, AppError> {
        let mut command = Command::new(&*self.program);
        command
            .arg("-s")
            .arg(&*self.serial)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let output = timeout(self.timeout, command.output())
            .await
            .map_err(|_| {
                AppError::BadGateway(format!("adb timed out talking to {}", self.serial))
            })?
            .map_err(|err| {
                AppError::BadGateway(format!(
                    "could not run adb ({}): {err}. Install Android platform-tools on the Sethu host.",
                    self.program
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail: String = stderr.trim().chars().take(300).collect();
            return Err(AppError::BadGateway(if detail.is_empty() {
                format!("adb exited with {}", output.status)
            } else {
                format!("adb: {detail}")
            }));
        }
        Ok(output.stdout)
    }

    /// `device`, `offline`, `unauthorized`, ... as reported by adb.
    pub async fn state(&self) -> Result<String, AppError> {
        let output = self.run(vec!["get-state".into()]).await?;
        Ok(String::from_utf8_lossy(&output).trim().to_string())
    }

    pub async fn screenshot(&self) -> Result<Bytes, AppError> {
        let output = self
            .run(vec!["exec-out".into(), "screencap".into(), "-p".into()])
            .await?;
        if png_size(&output).is_none() {
            return Err(AppError::BadGateway(
                "adb screencap did not return a PNG (is the device unlocked and authorized?)"
                    .into(),
            ));
        }
        Ok(Bytes::from(output))
    }

    pub async fn tap(&self, x: u32, y: u32) -> Result<(), AppError> {
        self.run(vec![
            "shell".into(),
            "input".into(),
            "tap".into(),
            x.to_string(),
            y.to_string(),
        ])
        .await
        .map(|_| ())
    }

    pub async fn swipe(
        &self,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        duration_ms: u32,
    ) -> Result<(), AppError> {
        self.run(vec![
            "shell".into(),
            "input".into(),
            "swipe".into(),
            x1.to_string(),
            y1.to_string(),
            x2.to_string(),
            y2.to_string(),
            duration_ms.clamp(50, 5_000).to_string(),
        ])
        .await
        .map(|_| ())
    }

    pub async fn type_text(&self, text: &str) -> Result<(), AppError> {
        let encoded = encode_input_text(text)?;
        if encoded.is_empty() {
            return Ok(());
        }
        self.run(vec!["shell".into(), "input".into(), "text".into(), encoded])
            .await
            .map(|_| ())
    }

    pub async fn press(&self, name: &str) -> Result<(), AppError> {
        let code = match name {
            "home" => "3",
            "back" => "4",
            "recents" => "187",
            "power" => "26",
            "wake" => "224",
            "volume_up" => "24",
            "volume_down" => "25",
            _ => return Err(AppError::BadRequest("unsupported button".into())),
        };
        self.run(vec![
            "shell".into(),
            "input".into(),
            "keyevent".into(),
            code.into(),
        ])
        .await
        .map(|_| ())
    }

    /// Push a local file into the device's public Downloads folder.
    pub async fn push_download(&self, local: &std::path::Path, name: &str) -> Result<(), AppError> {
        let safe_name = crate::files::sanitize_filename(name)
            .ok_or_else(|| AppError::BadRequest("invalid filename".into()))?;
        self.run(vec![
            "push".into(),
            local.to_string_lossy().into_owned(),
            format!("/sdcard/Download/{safe_name}"),
        ])
        .await
        .map(|_| ())
    }
}

/// `adb shell input text` interprets `%s` as a space and goes through the
/// device shell, so only a safe character set is accepted. Anything else is
/// rejected rather than risking shell metacharacters on the device.
fn encode_input_text(text: &str) -> Result<String, AppError> {
    if text.chars().count() > 2_000 {
        return Err(AppError::BadRequest(
            "text input is limited to 2000 characters".into(),
        ));
    }
    let mut encoded = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            ' ' => encoded.push_str("%s"),
            'a'..='z' | 'A'..='Z' | '0'..='9' => encoded.push(ch),
            '.' | ',' | '!' | '?' | '@' | '-' | '_' | '+' | '=' | '/' | ':' => encoded.push(ch),
            other => {
                return Err(AppError::BadRequest(format!(
                    "character {other:?} cannot be sent through adb input text"
                )))
            }
        }
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_encoding_handles_spaces_and_rejects_shell_metacharacters() {
        assert_eq!(encode_input_text("hello world").unwrap(), "hello%sworld");
        assert_eq!(encode_input_text("room 42!").unwrap(), "room%s42!");
        assert!(encode_input_text("danger; rm -rf /").is_err());
        assert!(encode_input_text("quote'").is_err());
        assert!(encode_input_text("$HOME").is_err());
    }

    #[test]
    fn text_encoding_limit() {
        assert!(encode_input_text(&"a".repeat(2_001)).is_err());
        assert!(encode_input_text(&"a".repeat(2_000)).is_ok());
    }
}
