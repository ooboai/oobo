pub fn index_single_session(
    _session_id: &str,
    _source: &str,
    _project_path: &str,
    _state: Option<&crate::hooks::state::ActiveSession>,
) -> Result<(), String> {
    Ok(())
}
