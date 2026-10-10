//! hoshi-bridge: turns GitHub and agent activity into game events.
//!
//! Placeholder. The event format and readers land through the M3 issues.

/// An outside event the game may react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeEvent {
    /// A pull request was merged.
    PullRequestMerged { repo: String, number: u64 },
    /// A CI run finished.
    CiFinished { repo: String, success: bool },
    /// A coding agent reported activity.
    AgentActivity { agent: String, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_compare() {
        let e = BridgeEvent::CiFinished {
            repo: "o/r".into(),
            success: true,
        };
        assert_eq!(e.clone(), e);
    }
}
