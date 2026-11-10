//! Minimal WebDriverAgent client.
//!
//! Sethu uses WDA's session-less endpoints (`/screenshot`, `/wda/tap`, ...)
//! which are available in the stock WebDriverAgent build that ships with
//! Appium. No Appium server is required.

use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use serde_json::{json, Value};

use crate::error::AppError;

#[derive(Clone)]
pub struct WdaClient {
    base: reqwest::Url,
    http: reqwest::Client,
    timeout: Duration,
}

impl WdaClient {
    pub fn new(base: reqwest::Url, http: reqwest::Client, timeout: Duration) -> Self {
        Self {
            base,
            http,
            timeout,
        }
    }

    pub fn from_target(
        target: &str,
        http: reqwest::Client,
        timeout: Duration,
    ) -> Result<Self, AppError> {
        let base = reqwest::Url::parse(target)
            .map_err(|_| AppError::BadRequest("invalid WebDriverAgent URL".into()))?;
        Ok(Self::new(base, http, timeout))
    }

    fn endpoint(&self, path: &str) -> reqwest::Url {
        let mut url = self.base.clone();
        let prefix = self.base.path().trim_end_matches('/');
        url.set_path(&format!("{prefix}{path}"));
        url.set_query(None);
        url.set_fragment(None);
        url
    }

    async fn get_json(&self, path: &str) -> Result<Value, AppError> {
        let request = self
            .http
            .get(self.endpoint(path))
            .timeout(self.timeout)
            .header("accept", "application/json");
        self.send(request).await
    }

    async fn post_json(&self, path: &str, body: Value) -> Result<Value, AppError> {
        let request = self
            .http
            .post(self.endpoint(path))
            .timeout(self.timeout)
            .json(&body);
        self.send(request).await
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value, AppError> {
        let response = request.send().await.map_err(|err| {
            let reason = if err.is_timeout() {
                "timed out".to_string()
            } else if err.is_connect() {
                "could not connect".to_string()
            } else {
                err.to_string()
            };
            AppError::BadGateway(format!("WebDriverAgent {reason} at {}", self.base))
        })?;

        let status = response.status();
        let body = response.bytes().await.map_err(|err| {
            AppError::BadGateway(format!("reading WebDriverAgent response: {err}"))
        })?;

        if !status.is_success() {
            let detail = String::from_utf8_lossy(&body);
            let detail = detail.chars().take(300).collect::<String>();
            return Err(AppError::BadGateway(format!(
                "WebDriverAgent returned {status}: {detail}"
            )));
        }
        if body.is_empty() {
            return Ok(Value::Null);
        }
        let value: Value = serde_json::from_slice(&body).map_err(|err| {
            AppError::BadGateway(format!("WebDriverAgent sent invalid JSON: {err}"))
        })?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(AppError::BadGateway(format!("WebDriverAgent: {error}")));
        }
        Ok(value)
    }

    /// PNG screenshot at native device resolution.
    pub async fn screenshot(&self) -> Result<Bytes, AppError> {
        let value = self.get_json("/screenshot").await?;
        let encoded = value
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::BadGateway("screenshot response had no value".into()))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|err| AppError::BadGateway(format!("invalid screenshot data: {err}")))?;
        Ok(Bytes::from(bytes))
    }

    pub async fn status(&self) -> Result<Value, AppError> {
        self.get_json("/status").await
    }

    /// Screen size in logical points. Used to translate screenshot pixels into
    /// the point-based coordinates WDA expects for touch events.
    pub async fn screen_points(&self) -> Option<(f64, f64)> {
        let value = self.get_json("/wda/screen").await.ok()?;
        let width = value.pointer("/value/screenSize/width")?.as_f64()?;
        let height = value.pointer("/value/screenSize/height")?.as_f64()?;
        (width > 0.0 && height > 0.0).then_some((width, height))
    }

    pub async fn tap(&self, x: f64, y: f64) -> Result<(), AppError> {
        self.post_json("/wda/tap", json!({ "x": x, "y": y }))
            .await
            .map(|_| ())
    }

    pub async fn swipe(
        &self,
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
        duration: f64,
    ) -> Result<(), AppError> {
        self.post_json(
            "/wda/dragfromtoforduration",
            json!({
                "fromX": from_x,
                "fromY": from_y,
                "toX": to_x,
                "toY": to_y,
                "duration": duration.clamp(0.05, 5.0),
            }),
        )
        .await
        .map(|_| ())
    }

    pub async fn type_text(&self, text: &str) -> Result<(), AppError> {
        let value: Vec<String> = text.chars().map(|c| c.to_string()).collect();
        if value.is_empty() {
            return Ok(());
        }
        self.post_json("/wda/keys", json!({ "value": value }))
            .await
            .map(|_| ())
    }

    pub async fn press(&self, name: &str) -> Result<(), AppError> {
        match name {
            "home" => self
                .post_json("/wda/homescreen", json!({}))
                .await
                .map(|_| ()),
            "lock" => self
                .post_json("/wda/device/lock", json!({}))
                .await
                .map(|_| ()),
            "unlock" => self
                .post_json("/wda/device/unlock", json!({}))
                .await
                .map(|_| ()),
            other => {
                if !other.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(AppError::BadRequest("unsupported button".into()));
                }
                self.post_json("/wda/pressButton", json!({ "name": other }))
                    .await
                    .map(|_| ())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_respects_base_path() {
        let http = reqwest::Client::new();
        let client =
            WdaClient::from_target("http://127.0.0.1:8100/prefix", http, Duration::from_secs(1))
                .unwrap();
        assert_eq!(
            client.endpoint("/screenshot").as_str(),
            "http://127.0.0.1:8100/prefix/screenshot"
        );
    }
}
