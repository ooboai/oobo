use crate::core::anchor::{Anchor, SessionLink};

use super::transcripts::CollectedTranscript;

pub(in crate::git) fn persist_anchor_local(
    _project_root: &str,
    anchor: &Anchor,
    _session_links: &[SessionLink],
) {
    if let Err(errors) = anchor.validate() {
        eprintln!(
            "oobo: warning: anchor invariant check failed for {}: {}",
            anchor.commit_hash,
            errors.join("; ")
        );
    }
}

pub(in crate::git) fn persist_anchor_portable(
    project_root: &str,
    anchor: &Anchor,
    session_links: &[SessionLink],
    transcripts: &[CollectedTranscript],
) {
    if let Err(e) = super::orphan::write_anchor(project_root, anchor, session_links, transcripts) {
        eprintln!("oobo: warning: could not write anchor to orphan branch: {e}");
    }
    super::anchor_cache::invalidate(project_root);
}

#[cfg(test)]
mod tests {}
