/// Native telemetry extracted directly from tool-specific data.
#[derive(Debug, Clone, Default)]
pub struct NativeStats {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub duration_secs: Option<u64>,
    pub files_touched: Vec<String>,
    pub tool_call_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_stats_default_is_zero() {
        let s = NativeStats::default();
        assert!(s.input_tokens.is_none());
        assert!(s.output_tokens.is_none());
        assert_eq!(s.tool_call_count, 0);
    }
}
