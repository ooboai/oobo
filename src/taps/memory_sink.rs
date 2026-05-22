//! In-memory [`TurnSink`] for tests. Collects turns + subagent links
//! into plain `Vec`s so tap tests assert on simple data shapes.

use super::{SubagentLink, TurnSink};
use crate::core::turn::Turn;

#[derive(Debug, Default)]
pub struct MemorySink {
    pub turns: Vec<Turn>,
    pub subagent_links: Vec<SubagentLink>,
}

impl TurnSink for MemorySink {
    fn accept_turn(&mut self, turn: Turn) {
        self.turns.push(turn);
    }
    fn accept_subagent_link(&mut self, link: SubagentLink) {
        self.subagent_links.push(link);
    }
}
