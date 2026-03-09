/// A single message from a transcript.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Message {
    pub role: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<i64>,
}
