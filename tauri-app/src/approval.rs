//! Holds a run while the user decides whether a tool may run.
//!
//! The prompt itself reaches the UI as an [`EngineEvent::ApprovalRequest`] on
//! the run's existing channel — this side only parks the waiting call and
//! resolves it when an answer arrives.

use std::{collections::HashMap, sync::Mutex, time::Duration};

use nexus_core::tools::{Approval, ApprovalGate, RunContext, ToolCall};
use tokio::sync::oneshot;

/// How long a prompt waits before giving up.
///
/// Times out to *deny*: an unanswered question is not consent, and someone who
/// walked away should not come back to find the model acted on their behalf.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

/// Identifies one prompt.
///
/// Includes the run id so an approval in one conversation cannot release a
/// call waiting in another.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PromptKey {
    run_id: String,
    call_id: String,
}

#[derive(Default)]
struct Inner {
    waiting: HashMap<PromptKey, oneshot::Sender<Approval>>,
    /// Answers that arrived before the call parked itself.
    ///
    /// The engine emits ApprovalRequest to the UI just *before* blocking on
    /// the gate, so an answer can in principle race the park. Buffering it
    /// turns a silently ignored click into a correct one.
    early: HashMap<PromptKey, Approval>,
}

#[derive(Default)]
pub struct ApprovalRouter {
    inner: Mutex<Inner>,
}

impl ApprovalRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the user's decision.
    ///
    /// Returns whether it landed anywhere — `false` means no such prompt and
    /// none is expected imminently, e.g. the run was cancelled.
    pub fn resolve(&self, run_id: &str, call_id: &str, approval: Approval) -> bool {
        let key = PromptKey {
            run_id: run_id.to_string(),
            call_id: call_id.to_string(),
        };

        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };

        if let Some(sender) = inner.waiting.remove(&key) {
            return sender.send(approval).is_ok();
        }

        // Nothing parked yet; hold the answer for the call that is about to
        // arrive. Bounded by the run being abandoned or the process exiting.
        inner.early.insert(key, approval);
        true
    }

    /// Deny everything a run left waiting, and discard its buffered answers.
    ///
    /// Called on cancel, so an aborted run cannot execute a tool because an
    /// approval landed after it stopped.
    pub fn abandon_run(&self, run_id: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };

        let keys: Vec<PromptKey> = inner
            .waiting
            .keys()
            .filter(|key| key.run_id == run_id)
            .cloned()
            .collect();

        for key in keys {
            if let Some(sender) = inner.waiting.remove(&key) {
                let _ = sender.send(Approval::Deny);
            }
        }

        inner.early.retain(|key, _| key.run_id != run_id);
    }
}

#[async_trait::async_trait]
impl ApprovalGate for ApprovalRouter {
    async fn request(&self, context: &RunContext, call: &ToolCall) -> Approval {
        let key = PromptKey {
            run_id: context.run_id.clone(),
            call_id: call.id.clone(),
        };

        let receiver = {
            let Ok(mut inner) = self.inner.lock() else {
                // No way to deliver a prompt means the only safe answer is no.
                return Approval::Deny;
            };

            if let Some(answer) = inner.early.remove(&key) {
                return answer;
            }

            let (sender, receiver) = oneshot::channel();
            inner.waiting.insert(key.clone(), sender);
            receiver
        };

        let answer = tokio::time::timeout(APPROVAL_TIMEOUT, receiver).await;

        // Clear the slot on every path out, so a timed-out prompt cannot be
        // answered later by a stale click.
        if let Ok(mut inner) = self.inner.lock() {
            inner.waiting.remove(&key);
        }

        match answer {
            Ok(Ok(approval)) => approval,
            // Timed out, or the sender was dropped with the run.
            _ => Approval::Deny,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "write_file".to_string(),
            arguments: serde_json::json!({ "path": "/tmp/x" }),
        }
    }

    /// Spawn a request and wait until it has actually parked.
    async fn park(router: &Arc<ApprovalRouter>) -> tokio::task::JoinHandle<Approval> {
        let handle = {
            let router = router.clone();
            tokio::spawn(async move {
                router
                    .request(&RunContext::new("s1", "r1"), &call("c1"))
                    .await
            })
        };

        while router.inner.lock().unwrap().waiting.is_empty() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        handle
    }

    #[tokio::test]
    async fn approving_releases_the_waiting_call() {
        let router = Arc::new(ApprovalRouter::new());
        let waiting = park(&router).await;

        assert!(router.resolve("r1", "c1", Approval::Approve));
        assert_eq!(waiting.await.unwrap(), Approval::Approve);
    }

    #[tokio::test]
    async fn denying_blocks_the_call() {
        let router = Arc::new(ApprovalRouter::new());
        let waiting = park(&router).await;

        assert!(router.resolve("r1", "c1", Approval::Deny));
        assert_eq!(waiting.await.unwrap(), Approval::Deny);
    }

    /// The reason RunContext carries a run id: an answer for one run must not
    /// release a call waiting in another.
    #[tokio::test]
    async fn an_answer_for_another_run_does_not_release_this_one() {
        let router = Arc::new(ApprovalRouter::new());
        let waiting = park(&router).await;

        // Same call id, different run.
        router.resolve("r2", "c1", Approval::Approve);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished(), "the call should still be waiting");

        router.resolve("r1", "c1", Approval::Deny);
        assert_eq!(waiting.await.unwrap(), Approval::Deny);
    }

    #[tokio::test]
    async fn abandoning_a_run_denies_everything_it_left_waiting() {
        let router = Arc::new(ApprovalRouter::new());
        let waiting = park(&router).await;

        router.abandon_run("r1");
        assert_eq!(waiting.await.unwrap(), Approval::Deny);
    }

    /// The UI is told a prompt exists just before the call blocks, so an
    /// answer can beat the park. It must still count.
    #[tokio::test]
    async fn an_answer_that_arrives_early_is_not_lost() {
        let router = Arc::new(ApprovalRouter::new());

        router.resolve("r1", "c1", Approval::Approve);

        let decision = router
            .request(&RunContext::new("s1", "r1"), &call("c1"))
            .await;
        assert_eq!(decision, Approval::Approve);
    }

    #[tokio::test]
    async fn an_early_answer_is_consumed_only_once() {
        let router = Arc::new(ApprovalRouter::new());
        router.resolve("r1", "c1", Approval::Approve);

        assert_eq!(
            router
                .request(&RunContext::new("s1", "r1"), &call("c1"))
                .await,
            Approval::Approve
        );

        // A second call with the same id must ask again rather than reuse it.
        let waiting = park(&router).await;
        router.abandon_run("r1");
        assert_eq!(waiting.await.unwrap(), Approval::Deny);
    }

    #[tokio::test]
    async fn abandoning_a_run_clears_its_buffered_answers() {
        let router = Arc::new(ApprovalRouter::new());
        router.resolve("r1", "c1", Approval::Approve);
        router.abandon_run("r1");

        // The buffered approval is gone, so this parks and then times out or
        // is denied — never silently approved.
        let waiting = park(&router).await;
        router.abandon_run("r1");
        assert_eq!(waiting.await.unwrap(), Approval::Deny);
    }
}
