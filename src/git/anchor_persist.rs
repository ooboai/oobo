use crate::core::anchor::{Anchor, SessionLink};

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
