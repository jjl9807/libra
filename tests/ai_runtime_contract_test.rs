//! Wave 1A runtime contract tests.
//!
//! Pin the `TaskExecutor` trait contract so any provider that implements
//! `CompletionModel` can be plugged into the runtime by wrapping it in a thin
//! adapter. Verifies the runtime can build a task prompt, dispatch through a
//! `TaskExecutor`, and surface the response back as a `TaskExecutionResult`.
//!
//! **Layer:** L1 — uses `MockCompletionModel`, no external dependencies.

mod helpers;

use std::path::PathBuf;

use async_trait::async_trait;
use helpers::mock_completion_model::MockCompletionModel;
use libra::internal::ai::{
    completion::{AssistantContent, CompletionModel, CompletionRequest},
    runtime::{
        Runtime, RuntimeConfig,
        contracts::{
            ApprovalMediationState, TaskExecutionContext, TaskExecutionError, TaskExecutionResult,
            TaskExecutionStatus, TaskExecutor,
        },
    },
};
use uuid::Uuid;

/// Generic adapter that turns any `CompletionModel` into a `TaskExecutor`.
///
/// Demonstrates the wiring an integrator would write to plug a custom provider into
/// the runtime: forward the prompt messages, capture the first text response as the
/// summary, fabricate a `run_id` if one was not supplied, and report
/// `TaskExecutionStatus::Completed`.
#[derive(Clone)]
struct CompletionBackedTaskExecutor<M> {
    model: M,
}

#[async_trait]
impl<M> TaskExecutor for CompletionBackedTaskExecutor<M>
where
    M: CompletionModel + Clone + Send + Sync,
{
    async fn execute_task_attempt(
        &self,
        context: TaskExecutionContext,
    ) -> Result<TaskExecutionResult, TaskExecutionError> {
        let response = self
            .model
            .completion(CompletionRequest::new(
                context
                    .prompt
                    .messages
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ))
            .await
            .map_err(|err| TaskExecutionError::Provider(err.to_string()))?;
        let summary = response.content.first().and_then(|content| match content {
            AssistantContent::Text(text) => Some(text.text.clone()),
            AssistantContent::ToolCall(_) => None,
        });

        Ok(TaskExecutionResult {
            task_id: context.task_id,
            run_id: context.run_id.unwrap_or_else(Uuid::new_v4),
            status: TaskExecutionStatus::Completed,
            evidence: vec![],
            summary,
        })
    }
}

/// Scenario: build the runtime's task prompt with a fixture provider/model pair,
/// dispatch a single attempt through `CompletionBackedTaskExecutor` backed by
/// `MockCompletionModel::text("attempt complete")`, and assert the result preserves
/// the supplied `task_id`, marks the attempt completed, and surfaces the model's
/// text as the summary. Acts as the contract pin proving the runtime actually
/// integrates a generic provider via the `TaskExecutor` trait alone.
#[tokio::test]
async fn generic_provider_can_execute_through_task_executor_contract() {
    let runtime = Runtime::new(RuntimeConfig {
        principal: "contract-test".into(),
    });
    let prompt = runtime
        .task_prompt_builder("mock", "scripted")
        .task("write tests", "prove the runtime contract")
        .build();
    let task_id = Uuid::new_v4();
    let executor = CompletionBackedTaskExecutor {
        model: MockCompletionModel::text("attempt complete"),
    };

    let result = executor
        .execute_task_attempt(TaskExecutionContext {
            thread_id: Uuid::new_v4(),
            task_id,
            run_id: None,
            working_dir: PathBuf::from("."),
            prompt,
            approval: ApprovalMediationState::RuntimeMediatedInteractive,
        })
        .await
        .expect("task attempt");

    assert_eq!(result.task_id, task_id);
    assert_eq!(result.status, TaskExecutionStatus::Completed);
    assert_eq!(result.summary.as_deref(), Some("attempt complete"));
}

/// W1-01: the runtime worker, rather than a UI adapter, owns per-session
/// serialized turn execution.  The executor deliberately waits so the test
/// can prove the second mutating turn cannot begin until the first exits.
#[tokio::test]
async fn agent_runtime_state_machine() {
    use std::sync::Arc;

    use libra::internal::ai::runtime::{
        AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, PrincipalContext,
        PrincipalRole, RuntimeExecutionContext, RuntimeTurnExecution, RuntimeTurnExecutor,
        RuntimeWorkerError, SecretRedactor, ToolBoundaryPolicy, ToolBoundaryRuntime, TurnRequest,
    };
    use tokio::{
        sync::{Mutex, Notify},
        time::{Duration, timeout},
    };

    struct BlockingExecutor {
        starts: Arc<Mutex<Vec<String>>>,
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for BlockingExecutor {
        async fn execute(
            &self,
            request: TurnRequest,
            context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            self.starts.lock().await.push(request.turn_id);
            self.started.notify_one();
            let cancellation = context.cancellation();
            tokio::select! {
                _ = self.release.notified() => Ok(RuntimeTurnExecution::Completed { summary: "done".to_string() }),
                _ = cancellation.cancelled() => Err(RuntimeWorkerError::Cancelled),
            }
        }
    }

    let starts = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let executor = Arc::new(BlockingExecutor {
        starts: starts.clone(),
        started: started.clone(),
        release: release.clone(),
    });
    let tool_boundary = ToolBoundaryRuntime::new(
        Uuid::new_v4(),
        PrincipalContext {
            principal_id: "runtime-contract-test".to_string(),
            role: PrincipalRole::Contributor,
        },
        ToolBoundaryPolicy::default_runtime(),
        SecretRedactor::default_runtime(),
        Arc::new(InMemoryAuditSink::default()),
    );
    let (handle, worker) =
        AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig::new(executor, tool_boundary));

    handle
        .submit(TurnRequest::new("session", "first", "first input", true))
        .await
        .expect("first turn accepted");
    handle
        .submit(TurnRequest::new("session", "second", "second input", true))
        .await
        .expect("second turn accepted");
    timeout(Duration::from_secs(1), started.notified())
        .await
        .expect("first turn started");
    assert_eq!(starts.lock().await.as_slice(), ["first"]);
    let snapshot = handle.snapshot("session").await.expect("runtime snapshot");
    assert_eq!(snapshot.active_turn_id.as_deref(), Some("first"));
    assert_eq!(snapshot.queued_turns, 1);

    release.notify_one();
    timeout(Duration::from_secs(1), started.notified())
        .await
        .expect("second turn started after first completed");
    assert_eq!(starts.lock().await.as_slice(), ["first", "second"]);
    release.notify_one();
    worker.abort();
}

#[test]
fn durable_intent_precedes_mutation_in_four_crash_windows() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use libra::internal::ai::{
        runtime::{
            DurableCommandCrashPoint, RuntimeCommandDurability, RuntimeCommandDurabilityError,
        },
        session::{
            CodeCommandIdentity, CodeCommandIntent, CodeCommandRecovery, CodeCommandStatus,
            CodeCommandStoreError, SessionJsonlStore,
        },
    };

    fn intent(command_id: &str, mutating: bool) -> CodeCommandIntent {
        CodeCommandIntent::new(
            CodeCommandIdentity::new("repo", "session", "contract-test", command_id),
            if mutating {
                "apply_patch"
            } else {
                "search_files"
            },
            format!("sha256:{command_id}"),
            mutating,
        )
    }

    for crash_point in [
        DurableCommandCrashPoint::BeforeIntentFsync,
        DurableCommandCrashPoint::AfterIntentFsyncBeforeDispatch,
        DurableCommandCrashPoint::AfterDispatchBeforeTerminalFsync,
        DurableCommandCrashPoint::AfterTerminalFsync,
    ] {
        let temp = tempfile::TempDir::new().expect("temporary session root");
        let durability =
            RuntimeCommandDurability::new(SessionJsonlStore::new(temp.path().join("session")));
        let command = intent("mutating", true);
        let dispatches = Arc::new(AtomicUsize::new(0));
        let dispatches_for_call = Arc::clone(&dispatches);

        let error = durability
            .execute(command.clone(), Some(crash_point), move || {
                dispatches_for_call.fetch_add(1, Ordering::SeqCst);
                Ok("mutation applied".to_string())
            })
            .expect_err("each injected crash must stop the caller");
        assert!(matches!(
            error,
            RuntimeCommandDurabilityError::InjectedCrash(point) if point == crash_point
        ));

        match crash_point {
            DurableCommandCrashPoint::BeforeIntentFsync => {
                assert_eq!(dispatches.load(Ordering::SeqCst), 0);
                assert!(matches!(
                    durability.recover(&command),
                    Err(RuntimeCommandDurabilityError::Store(
                        CodeCommandStoreError::MissingIntent { .. }
                    ))
                ));
            }
            DurableCommandCrashPoint::AfterIntentFsyncBeforeDispatch => {
                assert_eq!(dispatches.load(Ordering::SeqCst), 0);
                assert!(matches!(
                    durability
                        .recover(&command)
                        .expect("recover persisted intent"),
                    CodeCommandRecovery::Existing {
                        status: CodeCommandStatus::Indeterminate { .. }
                    }
                ));
            }
            DurableCommandCrashPoint::AfterDispatchBeforeTerminalFsync => {
                assert_eq!(dispatches.load(Ordering::SeqCst), 1);
                assert!(matches!(
                    durability
                        .recover(&command)
                        .expect("recover dispatched mutation"),
                    CodeCommandRecovery::Existing {
                        status: CodeCommandStatus::Indeterminate { .. }
                    }
                ));
            }
            DurableCommandCrashPoint::AfterTerminalFsync => {
                assert_eq!(dispatches.load(Ordering::SeqCst), 1);
                assert!(matches!(
                    durability
                        .recover(&command)
                        .expect("recover durable terminal result"),
                    CodeCommandRecovery::Existing {
                        status: CodeCommandStatus::Succeeded { .. }
                    }
                ));
            }
        }
    }

    let temp = tempfile::TempDir::new().expect("temporary read-only session root");
    let durability =
        RuntimeCommandDurability::new(SessionJsonlStore::new(temp.path().join("session")));
    let read_only = intent("read-only", false);
    let error = durability
        .execute(
            read_only.clone(),
            Some(DurableCommandCrashPoint::AfterDispatchBeforeTerminalFsync),
            || Ok("three matches".to_string()),
        )
        .expect_err("injected crash");
    assert!(matches!(
        error,
        RuntimeCommandDurabilityError::InjectedCrash(
            DurableCommandCrashPoint::AfterDispatchBeforeTerminalFsync
        )
    ));
    assert!(matches!(
        durability
            .recover(&read_only)
            .expect("recover read-only command"),
        CodeCommandRecovery::RetryReadOnly { .. }
    ));
    assert!(matches!(
        durability
            .retry_recovered_read_only(read_only, || Ok("three matches".to_string()))
            .expect("retry recovered read-only command"),
        CodeCommandStatus::Succeeded { .. }
    ));
}

#[tokio::test]
async fn recovered_mutating_command_fences_worker_session_before_admission() {
    use std::sync::Arc;

    use libra::internal::ai::{
        runtime::{
            AgentRuntimeWorker, AgentRuntimeWorkerConfig, ExternalTurnTrackingExecutor,
            InMemoryAuditSink, RuntimeCommandDurability, RuntimeWorkerError, ToolBoundaryRuntime,
            TurnRequest,
        },
        session::{CodeCommandIdentity, CodeCommandIntent, CodeCommandStatus, SessionJsonlStore},
    };

    let temp = tempfile::TempDir::new().expect("temporary session root");
    let durability =
        RuntimeCommandDurability::new(SessionJsonlStore::new(temp.path().join("session")));
    let intent = CodeCommandIntent::new(
        CodeCommandIdentity::new("repo", "session", "principal", "interrupted-turn"),
        "agent_runtime_turn",
        "sha256:request",
        true,
    );
    durability
        .admit(intent)
        .expect("persist interrupted intent");
    assert_eq!(
        durability
            .recover_pending_mutations()
            .expect("recover pending mutations"),
        vec![CodeCommandIdentity::new(
            "repo",
            "session",
            "principal",
            "interrupted-turn"
        )]
    );
    assert!(matches!(
        durability
            .session_store()
            .recover_code_command(&CodeCommandIdentity::new(
                "repo",
                "session",
                "principal",
                "interrupted-turn"
            ))
            .expect("read recovered command"),
        libra::internal::ai::session::CodeCommandRecovery::Existing {
            status: CodeCommandStatus::Indeterminate { .. }
        }
    ));

    let config = AgentRuntimeWorkerConfig::new(
        Arc::new(ExternalTurnTrackingExecutor),
        ToolBoundaryRuntime::system(Uuid::new_v4(), Arc::new(InMemoryAuditSink::default())),
    )
    .with_durability(durability, "repo", "principal")
    .with_recovered_reconciliation_session("session");
    let (handle, worker) = AgentRuntimeWorker::spawn(config);

    assert!(matches!(
        handle
            .submit(TurnRequest::new("session", "next-turn", "next input", true))
            .await,
        Err(RuntimeWorkerError::ReconciliationRequired { session_id }) if session_id == "session"
    ));
    worker.abort();
}

#[tokio::test]
async fn second_restart_keeps_reconciliation_fence_for_indeterminate_mutation() {
    use std::sync::Arc;

    use libra::internal::ai::{
        runtime::{
            AgentRuntimeWorker, AgentRuntimeWorkerConfig, ExternalTurnTrackingExecutor,
            InMemoryAuditSink, RuntimeCommandDurability, RuntimeWorkerError, ToolBoundaryRuntime,
            TurnRequest,
        },
        session::{CodeCommandIdentity, CodeCommandIntent, SessionJsonlStore},
    };

    let temp = tempfile::TempDir::new().expect("temporary session root");
    let session_root = temp.path().join("session");
    let first = RuntimeCommandDurability::new(SessionJsonlStore::new(session_root.clone()));
    let intent = CodeCommandIntent::new(
        CodeCommandIdentity::new("repo", "session", "principal", "interrupted-turn"),
        "agent_runtime_turn",
        "sha256:request",
        true,
    );
    first.admit(intent).expect("persist interrupted intent");
    assert_eq!(
        first
            .recover_pending_mutations()
            .expect("first restart fences pending mutation")
            .len(),
        1,
        "first restart must fence the pending mutating command"
    );

    // A later process opens the same durable log with no Pending rows left.
    // Recovery must still report the indeterminate mutation so callers keep the
    // in-memory reconciliation fence.
    let second = RuntimeCommandDurability::new(SessionJsonlStore::new(session_root));
    let recovered = second
        .recover_pending_mutations()
        .expect("second restart must still surface reconciliation");
    assert_eq!(
        recovered,
        vec![CodeCommandIdentity::new(
            "repo",
            "session",
            "principal",
            "interrupted-turn"
        )]
    );

    let config = AgentRuntimeWorkerConfig::new(
        Arc::new(ExternalTurnTrackingExecutor),
        ToolBoundaryRuntime::system(Uuid::new_v4(), Arc::new(InMemoryAuditSink::default())),
    )
    .with_durability(second, "repo", "principal")
    .with_recovered_reconciliation_session("session");
    let (handle, worker) = AgentRuntimeWorker::spawn(config);
    assert!(matches!(
        handle
            .submit(TurnRequest::new("session", "next-turn", "next input", true))
            .await,
        Err(RuntimeWorkerError::ReconciliationRequired { session_id }) if session_id == "session"
    ));
    worker.abort();
}

#[tokio::test]
async fn cancel_during_mutation_requires_reconciliation() {
    use std::sync::Arc;

    use libra::internal::ai::{
        runtime::{
            AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, InteractionState,
            PrincipalContext, PrincipalRole, RuntimeCommandDurability, RuntimeExecutionContext,
            RuntimeTurnExecution, RuntimeTurnExecutor, RuntimeWorkerError, SecretRedactor,
            ToolBoundaryPolicy, ToolBoundaryRuntime, TurnRequest,
        },
        session::{CodeCommandIdentity, CodeCommandRecovery, CodeCommandStatus, SessionJsonlStore},
    };
    use tokio::{
        sync::Notify,
        time::{Duration, timeout},
    };

    struct MutationExecutor {
        mutation_started: Arc<Notify>,
        release_first: Arc<Notify>,
        second_started: Arc<Notify>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for MutationExecutor {
        async fn execute(
            &self,
            request: TurnRequest,
            context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            if request.turn_id == "first" {
                context.mark_mutation_started();
                self.mutation_started.notify_one();
                self.release_first.notified().await;
                // A real adapter must report an actual terminal tool result.
                // This intentionally ambiguous result exercises the worker's
                // fail-closed reconciliation branch.
                return Err(RuntimeWorkerError::Cancelled);
            }
            self.second_started.notify_one();
            Ok(RuntimeTurnExecution::Completed {
                summary: "second completed".to_string(),
            })
        }
    }

    let mutation_started = Arc::new(Notify::new());
    let release_first = Arc::new(Notify::new());
    let second_started = Arc::new(Notify::new());
    let executor = Arc::new(MutationExecutor {
        mutation_started: Arc::clone(&mutation_started),
        release_first: Arc::clone(&release_first),
        second_started: Arc::clone(&second_started),
    });
    let boundary = ToolBoundaryRuntime::new(
        Uuid::new_v4(),
        PrincipalContext {
            principal_id: "runtime-cancel-test".to_string(),
            role: PrincipalRole::Contributor,
        },
        ToolBoundaryPolicy::default_runtime(),
        SecretRedactor::default_runtime(),
        Arc::new(InMemoryAuditSink::default()),
    );
    let temp = tempfile::TempDir::new().expect("temporary session JSONL root");
    let durability =
        RuntimeCommandDurability::new(SessionJsonlStore::new(temp.path().join("session")));
    let (handle, worker) = AgentRuntimeWorker::spawn(
        AgentRuntimeWorkerConfig::new(executor, boundary).with_durability(
            durability.clone(),
            "runtime-contract-repo",
            "runtime-cancel-test",
        ),
    );

    handle
        .submit(TurnRequest::new("session", "first", "apply patch", true))
        .await
        .expect("first turn accepted");
    timeout(Duration::from_secs(1), mutation_started.notified())
        .await
        .expect("mutation began");
    handle
        .submit(TurnRequest::new("session", "second", "next patch", true))
        .await
        .expect("second turn queued");

    handle
        .cancel("session", "first")
        .await
        .expect("cancel request accepted");
    assert_eq!(
        handle
            .snapshot("session")
            .await
            .expect("cancelling snapshot")
            .interaction,
        InteractionState::Cancelling,
        "a cancellation after mutation start must wait for executor reconciliation",
    );

    release_first.notify_one();
    timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = handle.snapshot("session").await.expect("snapshot");
            if matches!(
                snapshot.interaction,
                InteractionState::IndeterminateSideEffect { .. }
            ) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("ambiguous mutation result becomes indeterminate");
    assert!(matches!(
        durability
            .session_store()
            .recover_code_command(&CodeCommandIdentity::new(
                "runtime-contract-repo",
                "session",
                "runtime-cancel-test",
                "first",
            ))
            .expect("read durable cancellation result"),
        CodeCommandRecovery::Existing {
            status: CodeCommandStatus::Indeterminate { .. }
        }
    ));
    assert!(
        timeout(Duration::from_millis(100), second_started.notified())
            .await
            .is_err(),
        "the queued mutation must not begin while reconciliation is required"
    );
    assert!(matches!(
        handle
            .submit(TurnRequest::new("session", "third", "another patch", true))
            .await,
        Err(RuntimeWorkerError::ReconciliationRequired { .. })
    ));
    if let Err(error) = handle
        .submit(TurnRequest::new(
            "session",
            "fourth",
            "must not replay",
            true,
        ))
        .await
    {
        assert!(
            libra::internal::ai::runtime::runtime_worker_adapter_message(error)
                .contains("RECONCILIATION_REQUIRED"),
            "resubmit after cancel-indeterminate must expose a stable reconciliation code"
        );
    } else {
        panic!("resubmit after cancel-indeterminate must be rejected, not replayed");
    }
    worker.abort();
}

/// W2-02 AC3/AC4: while a Phase 0 IntentSpec review is pending
/// (`InteractionState::AwaitingIntentReview`), the worker — not
/// `pending_intent_review` in the TUI — is the durable owner of the mutation
/// fence. A follow-on turn on the same session (e.g. a stray mutating tool
/// call outside the `phase0_plan_tool_loop_config` allowlist) must queue
/// rather than execute, because the tracked Phase 0 turn stays active until
/// `IntentReviewAckDelivery` resolves it via `respond`. Only `confirm`
/// releases the fence for the queued turn to start.
#[tokio::test]
async fn intentspec_review_blocks_mutation() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use libra::internal::ai::runtime::{
        AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, InteractionResponse,
        InteractionState, PrincipalContext, PrincipalRole, RuntimeExecutionContext,
        RuntimeTurnExecution, RuntimeTurnExecutor, RuntimeWorkerError, SecretRedactor,
        ToolBoundaryPolicy, ToolBoundaryRuntime, TurnRequest,
        phase0::{IntentReviewAckDelivery, IntentReviewDecision},
    };
    use tokio::{
        sync::Notify,
        time::{Duration, timeout},
    };
    use tokio_util::sync::CancellationToken;

    struct MutatingExecutor {
        started: Arc<AtomicUsize>,
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for MutatingExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            _context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_one();
            Ok(RuntimeTurnExecution::Completed {
                summary: "mutation applied".to_string(),
            })
        }
    }

    let started = Arc::new(AtomicUsize::new(0));
    let notify = Arc::new(Notify::new());
    let executor = Arc::new(MutatingExecutor {
        started: Arc::clone(&started),
        notify: Arc::clone(&notify),
    });
    let boundary = ToolBoundaryRuntime::new(
        Uuid::new_v4(),
        PrincipalContext {
            principal_id: "intentspec-review-fence-test".to_string(),
            role: PrincipalRole::Contributor,
        },
        ToolBoundaryPolicy::default_runtime(),
        SecretRedactor::default_runtime(),
        Arc::new(InMemoryAuditSink::default()),
    );
    let (handle, worker) =
        AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig::new(executor, boundary));

    // Track the Phase 0 turn the way the TUI/web adapter does once
    // `submit_intent_draft` fires: `track_external_turn` keeps it active
    // while the tool loop itself has already exited.
    handle
        .track_external_turn(
            TurnRequest::new("session", "phase0-turn", "plan workflow", true),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("phase0 turn tracked");
    handle
        .register_interaction_with_delivery(
            "session",
            "phase0-turn",
            InteractionState::AwaitingIntentReview {
                interaction_id: "intent-1".to_string(),
            },
            Box::new(IntentReviewAckDelivery::new()),
        )
        .await
        .expect("worker owns the IntentSpec review interaction");

    // A follow-on turn must queue, not run, while the review is pending.
    handle
        .submit(TurnRequest::new(
            "session",
            "mutating-turn",
            "apply_patch",
            true,
        ))
        .await
        .expect("mutating turn is accepted into the queue");
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "no mutating tool call may execute before the IntentSpec review is confirmed"
    );
    let snapshot = handle
        .snapshot("session")
        .await
        .expect("snapshot while awaiting review");
    assert_eq!(
        snapshot.interaction,
        InteractionState::AwaitingIntentReview {
            interaction_id: "intent-1".to_string(),
        }
    );
    assert_eq!(snapshot.queued_turns, 1);

    // Confirming the review resolves the Phase 0 turn and releases the fence.
    handle
        .respond(
            "session",
            "phase0-turn",
            InteractionResponse::new("intent-1", IntentReviewDecision::Confirm.wire_id()),
        )
        .await
        .expect("confirm resolves the review");
    timeout(Duration::from_secs(1), notify.notified())
        .await
        .expect("queued mutating turn starts once the review is resolved");
    assert_eq!(started.load(Ordering::SeqCst), 1);
    worker.abort();
}

/// W2-02 recovery: Phase 0 must be durably terminalized before the review
/// gate parks, and the gate itself must be non-mutating so a crash cannot
/// reopen an indeterminate reconciliation fence.
#[tokio::test]
async fn intentspec_review_gate_is_non_mutating_after_phase0_terminalizes() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use libra::internal::ai::runtime::{
        AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, InteractionResponse,
        InteractionState, PrincipalContext, PrincipalRole, RuntimeExecutionContext,
        RuntimeTurnExecution, RuntimeTurnExecutor, RuntimeWorkerError, SecretRedactor,
        ToolBoundaryPolicy, ToolBoundaryRuntime, TurnRequest,
        phase0::{IntentReviewAckDelivery, IntentReviewDecision},
    };
    use tokio::time::{Duration, sleep};
    use tokio_util::sync::CancellationToken;

    struct MutatingExecutor {
        started: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for MutatingExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            _context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            Ok(RuntimeTurnExecution::Completed {
                summary: "mutation applied".to_string(),
            })
        }
    }

    let started = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(MutatingExecutor {
        started: Arc::clone(&started),
    });
    let boundary = ToolBoundaryRuntime::new(
        Uuid::new_v4(),
        PrincipalContext {
            principal_id: "intentspec-review-hold-queued".to_string(),
            role: PrincipalRole::Contributor,
        },
        ToolBoundaryPolicy::default_runtime(),
        SecretRedactor::default_runtime(),
        Arc::new(InMemoryAuditSink::default()),
    );
    let (handle, worker) =
        AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig::new(executor, boundary));

    handle
        .track_external_turn(
            TurnRequest::new("session", "phase0-turn", "plan workflow", true),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(true)),
        )
        .await
        .expect("phase0 turn tracked");
    handle
        .submit(TurnRequest::new(
            "session",
            "mutating-turn",
            "apply_patch",
            true,
        ))
        .await
        .expect("mutating turn queues behind phase0");
    assert_eq!(started.load(Ordering::SeqCst), 0);

    // Mirror the TUI register path: terminalize Phase 0 without releasing the
    // queue, then park a non-mutating review gate in front of it.
    handle
        .finish_external_turn(
            "session",
            "phase0-turn",
            Ok(RuntimeTurnExecution::CompletedHoldQueued {
                summary: "IntentSpec draft persisted; awaiting review".to_string(),
            }),
        )
        .await
        .expect("phase0 terminalizes without releasing the queue");
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "CompletedHoldQueued must not start queued mutations"
    );

    handle
        .track_external_turn(
            TurnRequest::new("session", "intent-review-turn", "IntentSpec review", false),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("non-mutating review gate can park in front of the queue");
    handle
        .register_interaction_with_delivery(
            "session",
            "intent-review-turn",
            InteractionState::AwaitingIntentReview {
                interaction_id: "intent-1".to_string(),
            },
            Box::new(IntentReviewAckDelivery::new()),
        )
        .await
        .expect("review gate owns AwaitingIntentReview");

    sleep(Duration::from_millis(50)).await;
    assert_eq!(started.load(Ordering::SeqCst), 0);
    assert_eq!(
        handle
            .snapshot("session")
            .await
            .expect("snapshot")
            .queued_turns,
        1
    );

    handle
        .respond(
            "session",
            "intent-review-turn",
            InteractionResponse::new("intent-1", IntentReviewDecision::Confirm.wire_id()),
        )
        .await
        .expect("confirm resolves the non-mutating gate");
    sleep(Duration::from_millis(100)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "confirm releases the held queue"
    );
    worker.abort();
}

/// W2-02 AC3: revise/cancel must discard turns queued under the IntentSpec
/// review fence. Completing the Phase 0 turn alone is not enough — those
/// queued mutations never received a confirmed IntentSpec.
#[tokio::test]
async fn intentspec_review_non_confirm_discards_queued_mutations() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use libra::internal::ai::runtime::{
        AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, InteractionResponse,
        InteractionState, PrincipalContext, PrincipalRole, RuntimeExecutionContext,
        RuntimeTurnExecution, RuntimeTurnExecutor, RuntimeWorkerError, SecretRedactor,
        ToolBoundaryPolicy, ToolBoundaryRuntime, TurnRequest,
        phase0::{IntentReviewAckDelivery, IntentReviewDecision},
    };
    use tokio::time::{Duration, sleep};
    use tokio_util::sync::CancellationToken;

    struct MutatingExecutor {
        started: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for MutatingExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            _context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            Ok(RuntimeTurnExecution::Completed {
                summary: "mutation applied".to_string(),
            })
        }
    }

    for decision in [IntentReviewDecision::Revise, IntentReviewDecision::Cancel] {
        let started = Arc::new(AtomicUsize::new(0));
        let executor = Arc::new(MutatingExecutor {
            started: Arc::clone(&started),
        });
        let boundary = ToolBoundaryRuntime::new(
            Uuid::new_v4(),
            PrincipalContext {
                principal_id: format!("intentspec-review-{}-fence", decision.wire_id()),
                role: PrincipalRole::Contributor,
            },
            ToolBoundaryPolicy::default_runtime(),
            SecretRedactor::default_runtime(),
            Arc::new(InMemoryAuditSink::default()),
        );
        let (handle, worker) =
            AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig::new(executor, boundary));

        handle
            .track_external_turn(
                TurnRequest::new("session", "phase0-turn", "plan workflow", true),
                CancellationToken::new(),
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .expect("phase0 turn tracked");
        handle
            .register_interaction_with_delivery(
                "session",
                "phase0-turn",
                InteractionState::AwaitingIntentReview {
                    interaction_id: "intent-1".to_string(),
                },
                Box::new(IntentReviewAckDelivery::new()),
            )
            .await
            .expect("worker owns the IntentSpec review interaction");
        handle
            .submit(TurnRequest::new(
                "session",
                "mutating-turn",
                "apply_patch",
                true,
            ))
            .await
            .expect("mutating turn is accepted into the queue");
        assert_eq!(started.load(Ordering::SeqCst), 0);

        handle
            .respond(
                "session",
                "phase0-turn",
                InteractionResponse::new("intent-1", decision.wire_id()),
            )
            .await
            .expect("non-confirm decision resolves the review");
        // Give the worker a moment to (incorrectly) start the queued turn if
        // the fence regresses; then assert it never ran.
        sleep(Duration::from_millis(50)).await;
        assert_eq!(
            started.load(Ordering::SeqCst),
            0,
            "{} must discard queued mutations, not release them",
            decision.wire_id()
        );
        let snapshot = handle
            .snapshot("session")
            .await
            .expect("snapshot after non-confirm");
        assert_eq!(snapshot.interaction, InteractionState::Completed);
        assert_eq!(
            snapshot.queued_turns,
            0,
            "{} must empty the queue of fenced mutations",
            decision.wire_id()
        );
        worker.abort();
    }
}

/// W2-03: Plan Execute must not release mutating work until the network-policy
/// human gate resolves. Revise/Cancel discard queued mutations (same fence
/// contract as IntentSpec review).
#[tokio::test]
async fn plan_review_network_policy_gate() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use libra::internal::ai::runtime::{
        AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, InteractionResponse,
        InteractionState, PrincipalContext, PrincipalRole, RuntimeExecutionContext,
        RuntimeTurnExecution, RuntimeTurnExecutor, RuntimeWorkerError, SecretRedactor,
        ToolBoundaryPolicy, ToolBoundaryRuntime, TurnRequest,
        phase1::{
            NetworkPolicyAckDelivery, NetworkPolicyDecision, PlanReviewAckDelivery,
            PlanReviewDecision,
        },
    };
    use tokio::{
        sync::Notify,
        time::{Duration, timeout},
    };
    use tokio_util::sync::CancellationToken;

    struct MutatingExecutor {
        started: Arc<AtomicUsize>,
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for MutatingExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            _context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_one();
            Ok(RuntimeTurnExecution::Completed {
                summary: "mutation applied".to_string(),
            })
        }
    }

    let started = Arc::new(AtomicUsize::new(0));
    let notify = Arc::new(Notify::new());
    let executor = Arc::new(MutatingExecutor {
        started: Arc::clone(&started),
        notify: Arc::clone(&notify),
    });
    let boundary = ToolBoundaryRuntime::new(
        Uuid::new_v4(),
        PrincipalContext {
            principal_id: "plan-review-network-policy-gate".to_string(),
            role: PrincipalRole::Contributor,
        },
        ToolBoundaryPolicy::default_runtime(),
        SecretRedactor::default_runtime(),
        Arc::new(InMemoryAuditSink::default()),
    );
    let (handle, worker) =
        AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig::new(executor, boundary));

    handle
        .track_external_turn(
            TurnRequest::new("session", "plan-review-turn", "plan review", false),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("plan review turn tracked");
    handle
        .register_interaction_with_delivery(
            "session",
            "plan-review-turn",
            InteractionState::AwaitingPlanReview {
                interaction_id: "plan-1".to_string(),
            },
            Box::new(PlanReviewAckDelivery::new()),
        )
        .await
        .expect("worker owns the Plan review interaction");

    handle
        .submit(TurnRequest::new(
            "session",
            "mutating-turn",
            "apply_patch",
            true,
        ))
        .await
        .expect("mutating turn is accepted into the queue");
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "no mutating tool may run before Plan review + network policy resolve"
    );

    handle
        .respond(
            "session",
            "plan-review-turn",
            InteractionResponse::new("plan-1", PlanReviewDecision::Execute.wire_id()),
        )
        .await
        .expect("Execute advances to network policy");
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "Execute must HoldQueued — network policy still required"
    );

    handle
        .track_external_turn(
            TurnRequest::new("session", "network-policy-turn", "network policy", false),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("network policy turn tracked");
    handle
        .register_interaction_with_delivery(
            "session",
            "network-policy-turn",
            InteractionState::AwaitingNetworkPolicy {
                interaction_id: "plan-1:network-policy".to_string(),
            },
            Box::new(NetworkPolicyAckDelivery::new()),
        )
        .await
        .expect("worker owns the network policy interaction");

    let snapshot = handle
        .snapshot("session")
        .await
        .expect("snapshot while awaiting network policy");
    assert_eq!(
        snapshot.interaction,
        InteractionState::AwaitingNetworkPolicy {
            interaction_id: "plan-1:network-policy".to_string(),
        }
    );
    assert_eq!(started.load(Ordering::SeqCst), 0);

    handle
        .respond(
            "session",
            "network-policy-turn",
            InteractionResponse::new(
                "plan-1:network-policy",
                NetworkPolicyDecision::Allow.wire_id(),
            ),
        )
        .await
        .expect("network allow releases the fence");

    timeout(Duration::from_secs(2), notify.notified())
        .await
        .expect("queued mutation should start after network allow");
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "network allow releases the held mutating queue"
    );
    worker.abort();
}

/// W2-03 end-to-end fence, mirroring `intentspec_review_blocks_mutation` but
/// for the full Phase 1 sequence the TUI drives: a mutating Phase 1 turn
/// terminalizes with `CompletedHoldQueued`, a non-mutating Plan review gate
/// parks in front of the held queue, Execute hands off to a non-mutating
/// network-policy gate, and only `network-allow` releases the mutation.
#[tokio::test]
async fn plan_review_gate_holds_phase1_queue_until_network_policy_resolves() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use libra::internal::ai::runtime::{
        AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, InteractionResponse,
        InteractionState, PrincipalContext, PrincipalRole, RuntimeExecutionContext,
        RuntimeTurnExecution, RuntimeTurnExecutor, RuntimeWorkerError, SecretRedactor,
        ToolBoundaryPolicy, ToolBoundaryRuntime, TurnRequest,
        phase1::{
            NetworkPolicyAckDelivery, NetworkPolicyDecision, PlanReviewAckDelivery,
            PlanReviewDecision, network_policy_interaction_id,
        },
    };
    use tokio::{
        sync::Notify,
        time::{Duration, sleep, timeout},
    };
    use tokio_util::sync::CancellationToken;

    struct MutatingExecutor {
        started: Arc<AtomicUsize>,
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for MutatingExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            _context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_one();
            Ok(RuntimeTurnExecution::Completed {
                summary: "mutation applied".to_string(),
            })
        }
    }

    let started = Arc::new(AtomicUsize::new(0));
    let notify = Arc::new(Notify::new());
    let executor = Arc::new(MutatingExecutor {
        started: Arc::clone(&started),
        notify: Arc::clone(&notify),
    });
    let boundary = ToolBoundaryRuntime::new(
        Uuid::new_v4(),
        PrincipalContext {
            principal_id: "plan-review-phase1-hold-queued".to_string(),
            role: PrincipalRole::Contributor,
        },
        ToolBoundaryPolicy::default_runtime(),
        SecretRedactor::default_runtime(),
        Arc::new(InMemoryAuditSink::default()),
    );
    let (handle, worker) =
        AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig::new(executor, boundary));

    // The mutating Phase 1 turn that wrote the plan draft is still tracked when
    // `PlanWorkflowComplete` lands, exactly as `track_external_turn` leaves it.
    handle
        .track_external_turn(
            TurnRequest::new("session", "phase1-turn", "plan workflow", true),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(true)),
        )
        .await
        .expect("phase1 turn tracked");
    handle
        .submit(TurnRequest::new(
            "session",
            "mutating-turn",
            "apply_patch",
            true,
        ))
        .await
        .expect("mutating turn queues behind phase1");
    assert_eq!(started.load(Ordering::SeqCst), 0);

    handle
        .finish_external_turn(
            "session",
            "phase1-turn",
            Ok(RuntimeTurnExecution::CompletedHoldQueued {
                summary: "Execution plan draft persisted; awaiting review".to_string(),
            }),
        )
        .await
        .expect("phase1 terminalizes without releasing the queue");
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "CompletedHoldQueued must not start queued mutations"
    );

    handle
        .track_external_turn(
            TurnRequest::new("session", "plan-review-turn", "Plan review", false),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("non-mutating plan review gate parks in front of the queue");
    handle
        .register_interaction_with_delivery(
            "session",
            "plan-review-turn",
            InteractionState::AwaitingPlanReview {
                interaction_id: "plan-7".to_string(),
            },
            Box::new(PlanReviewAckDelivery::new()),
        )
        .await
        .expect("review gate owns AwaitingPlanReview");

    sleep(Duration::from_millis(50)).await;
    let snapshot = handle
        .snapshot("session")
        .await
        .expect("snapshot while awaiting plan review");
    assert_eq!(
        snapshot.interaction,
        InteractionState::AwaitingPlanReview {
            interaction_id: "plan-7".to_string(),
        }
    );
    assert_eq!(snapshot.queued_turns, 1);
    assert_eq!(started.load(Ordering::SeqCst), 0);

    handle
        .respond(
            "session",
            "plan-review-turn",
            InteractionResponse::new("plan-7", PlanReviewDecision::Execute.wire_id()),
        )
        .await
        .expect("Execute resolves the plan review gate");
    sleep(Duration::from_millis(50)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "Execute alone must not admit mutating work — network policy is still unanswered"
    );

    let network_interaction_id = network_policy_interaction_id(Some("plan-7"));
    assert_eq!(network_interaction_id, "plan-7:network-policy");
    handle
        .track_external_turn(
            TurnRequest::new(
                "session",
                "network-policy-turn",
                "network policy (default: deny)",
                false,
            ),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("network policy gate parks after Execute");
    handle
        .register_interaction_with_delivery(
            "session",
            "network-policy-turn",
            InteractionState::AwaitingNetworkPolicy {
                interaction_id: network_interaction_id.clone(),
            },
            Box::new(NetworkPolicyAckDelivery::new()),
        )
        .await
        .expect("network policy gate owns AwaitingNetworkPolicy");
    sleep(Duration::from_millis(50)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "a preselected network default must not skip the human gate"
    );

    handle
        .respond(
            "session",
            "network-policy-turn",
            InteractionResponse::new(
                network_interaction_id.as_str(),
                NetworkPolicyDecision::Allow.wire_id(),
            ),
        )
        .await
        .expect("network allow resolves the last gate");
    timeout(Duration::from_secs(2), notify.notified())
        .await
        .expect("queued mutation starts once both Phase 1 gates resolve");
    assert_eq!(started.load(Ordering::SeqCst), 1);
    let snapshot = handle
        .snapshot("session")
        .await
        .expect("snapshot after both gates resolved");
    assert_eq!(snapshot.queued_turns, 0);
    worker.abort();
}

/// W2-03 recovery: after Plan `Execute` durably resolves the review, the
/// `NetworkPolicyRequested` marker keeps the mandatory network human gate
/// recoverable across restart. Markers observed without a prior Execute
/// resolution must not reopen (otherwise an unapproved plan could execute).
#[test]
fn plan_review_execute_leaves_network_policy_gate_recoverable() {
    use libra::internal::ai::{
        runtime::phase1::{
            network_policy_interaction_id, open_network_policy_from_workflow,
            open_plan_review_from_workflow,
        },
        session::{CodeWorkflowEventKind, jsonl::SessionJsonlStore},
    };

    let temp = tempfile::tempdir().expect("temp dir");
    let session_root = temp.path().join("session");
    let store = SessionJsonlStore::new(session_root.clone());
    let network_interaction_id = network_policy_interaction_id(Some("plan-42"));

    store
        .append_code_workflow_durable(CodeWorkflowEventKind::PlanReviewRequested {
            interaction_id: "review-42".to_string(),
            plan_id: "plan-42".to_string(),
            turn_id: "plan-review-turn".to_string(),
            phase1_turn_id: "phase1-turn".to_string(),
        })
        .expect("plan review marker persists");
    // Premature marker (before Execute) must not restore.
    store
        .append_code_workflow_durable(CodeWorkflowEventKind::NetworkPolicyRequested {
            interaction_id: network_interaction_id.clone(),
            plan_id: "plan-42".to_string(),
            turn_id: "network-policy-turn".to_string(),
            default_allow: true,
        })
        .expect("premature network policy marker persists");
    {
        let premature = SessionJsonlStore::new(session_root.clone());
        let events: Vec<_> = premature
            .load_code_workflow_replay()
            .expect("replay")
            .events
            .into_iter()
            .map(|e| e.event)
            .collect();
        assert!(
            open_network_policy_from_workflow(events.iter()).is_none(),
            "network marker before Plan Execute must not be restorable"
        );
    }
    store
        .append_code_workflow_durable(CodeWorkflowEventKind::InteractionResolved {
            interaction_id: "review-42".to_string(),
            resolution: "execute".to_string(),
        })
        .expect("plan review resolution persists");

    // Restart: a fresh store over the same session root.
    let reloaded = SessionJsonlStore::new(session_root);
    let events = |store: &SessionJsonlStore| {
        store
            .load_code_workflow_replay()
            .expect("workflow replay")
            .events
            .into_iter()
            .map(|event| event.event)
            .collect::<Vec<_>>()
    };
    let replayed = events(&reloaded);
    assert_eq!(
        open_plan_review_from_workflow(replayed.iter()),
        None,
        "the plan review is durably resolved, so its restore must no-op"
    );
    assert_eq!(
        open_network_policy_from_workflow(replayed.iter()),
        Some((
            network_interaction_id.clone(),
            "plan-42".to_string(),
            "network-policy-turn".to_string(),
            true,
        )),
        "after Execute, the unanswered network-policy gate must survive restart"
    );

    reloaded
        .append_code_workflow_durable(CodeWorkflowEventKind::InteractionResolved {
            interaction_id: network_interaction_id,
            resolution: "network-allow".to_string(),
        })
        .expect("network policy resolution persists");
    assert!(
        open_network_policy_from_workflow(events(&reloaded).iter()).is_none(),
        "answering the gate must clear the durable marker"
    );
}

/// W2-03 r6: `PlanReviewAckDelivery` Execute returns `CompletedHoldQueued`,
/// which must still append durable `InteractionResolved` so network-policy
/// recovery can prove the plan was approved.
#[tokio::test]
async fn plan_review_execute_hold_queued_persists_interaction_resolved() {
    use std::sync::{Arc, atomic::AtomicBool};

    use libra::internal::ai::{
        runtime::{
            AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, InteractionResponse,
            InteractionState, PrincipalContext, PrincipalRole, RuntimeCommandDurability,
            SecretRedactor, ToolBoundaryPolicy, ToolBoundaryRuntime, TurnRequest,
            phase1::{
                PlanReviewAckDelivery, PlanReviewDecision, network_policy_interaction_id,
                open_network_policy_from_workflow, open_plan_review_from_workflow,
            },
        },
        session::{CodeWorkflowEventKind, SessionJsonlStore},
    };
    use tokio_util::sync::CancellationToken;

    let temp = tempfile::tempdir().expect("temp dir");
    let session_root = temp.path().join("session");
    let store = SessionJsonlStore::new(session_root.clone());
    let durability = RuntimeCommandDurability::new(SessionJsonlStore::new(session_root.clone()));
    let boundary = ToolBoundaryRuntime::new(
        Uuid::new_v4(),
        PrincipalContext {
            principal_id: "plan-review-hold-persist".to_string(),
            role: PrincipalRole::Contributor,
        },
        ToolBoundaryPolicy::default_runtime(),
        SecretRedactor::default_runtime(),
        Arc::new(InMemoryAuditSink::default()),
    );
    let (handle, worker) = AgentRuntimeWorker::spawn(
        AgentRuntimeWorkerConfig::new(
            Arc::new(libra::internal::ai::runtime::ExternalTurnTrackingExecutor),
            boundary,
        )
        .with_durability(durability, "repo", "principal")
        .with_durability_command_kind("tui_local_turn"),
    );

    store
        .append_code_workflow_durable(CodeWorkflowEventKind::PlanReviewRequested {
            interaction_id: "review-hold".to_string(),
            plan_id: "plan-hold".to_string(),
            turn_id: "plan-review-turn".to_string(),
            phase1_turn_id: "phase1-turn".to_string(),
        })
        .expect("plan review marker");
    store
        .append_code_workflow_durable(CodeWorkflowEventKind::NetworkPolicyRequested {
            interaction_id: network_policy_interaction_id(Some("plan-hold")),
            plan_id: "plan-hold".to_string(),
            turn_id: "network-policy-turn".to_string(),
            default_allow: true,
        })
        .expect("network policy marker before Execute");

    handle
        .track_external_turn(
            TurnRequest::new("session", "plan-review-turn", "Plan review", false),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("plan review gate tracked");
    handle
        .register_interaction_with_delivery(
            "session",
            "plan-review-turn",
            InteractionState::AwaitingPlanReview {
                interaction_id: "review-hold".to_string(),
            },
            Box::new(PlanReviewAckDelivery::new()),
        )
        .await
        .expect("plan review delivery registered");
    handle
        .respond(
            "session",
            "plan-review-turn",
            InteractionResponse::new("review-hold", PlanReviewDecision::Execute.wire_id()),
        )
        .await
        .expect("Execute settles via CompletedHoldQueued");

    let events: Vec<_> = store
        .load_code_workflow_replay()
        .expect("replay")
        .events
        .into_iter()
        .map(|event| event.event)
        .collect();
    assert!(
        events.iter().any(|event| match event {
            CodeWorkflowEventKind::InteractionResolved {
                interaction_id,
                resolution,
            }
            | CodeWorkflowEventKind::CommandTerminalSuccessWithInteractionResolved {
                interaction_id,
                resolution,
                ..
            } => {
                interaction_id == "review-hold" && resolution.eq_ignore_ascii_case("execute")
            }
            _ => false,
        }),
        "CompletedHoldQueued Execute must append a durable execute resolution: {events:?}"
    );
    assert_eq!(
        open_plan_review_from_workflow(events.iter()),
        None,
        "plan review must be closed after durable Execute"
    );
    assert_eq!(
        open_network_policy_from_workflow(events.iter()),
        Some((
            network_policy_interaction_id(Some("plan-hold")),
            "plan-hold".to_string(),
            "network-policy-turn".to_string(),
            true,
        )),
        "network gate must become restorable once Execute is durably resolved"
    );
    worker.abort();
}

/// W2-03 r9: after Plan `Execute` returns `CompletedHoldQueued`, a concurrent
/// `submit` must not drain the held queue before the network-policy gate is
/// tracked — otherwise a mutation can start without an explicit network choice.
#[tokio::test]
async fn plan_review_execute_hold_blocks_submit_until_network_gate_parks() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use libra::internal::ai::runtime::{
        AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, InteractionResponse,
        InteractionState, PrincipalContext, PrincipalRole, RuntimeExecutionContext,
        RuntimeTurnExecution, RuntimeTurnExecutor, RuntimeWorkerError, SecretRedactor,
        ToolBoundaryPolicy, ToolBoundaryRuntime, TurnRequest,
        phase1::{
            NetworkPolicyAckDelivery, NetworkPolicyDecision, PlanReviewAckDelivery,
            PlanReviewDecision, network_policy_interaction_id,
        },
    };
    use tokio::{
        sync::Notify,
        time::{Duration, sleep, timeout},
    };
    use tokio_util::sync::CancellationToken;

    struct MutatingExecutor {
        started: Arc<AtomicUsize>,
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for MutatingExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            _context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_one();
            Ok(RuntimeTurnExecution::Completed {
                summary: "mutation applied".to_string(),
            })
        }
    }

    let started = Arc::new(AtomicUsize::new(0));
    let notify = Arc::new(Notify::new());
    let executor = Arc::new(MutatingExecutor {
        started: Arc::clone(&started),
        notify: Arc::clone(&notify),
    });
    let boundary = ToolBoundaryRuntime::new(
        Uuid::new_v4(),
        PrincipalContext {
            principal_id: "plan-review-hold-submit-race".to_string(),
            role: PrincipalRole::Contributor,
        },
        ToolBoundaryPolicy::default_runtime(),
        SecretRedactor::default_runtime(),
        Arc::new(InMemoryAuditSink::default()),
    );
    let (handle, worker) =
        AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig::new(executor, boundary));

    handle
        .track_external_turn(
            TurnRequest::new("session", "phase1-turn", "plan workflow", true),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(true)),
        )
        .await
        .expect("phase1 turn tracked");
    handle
        .submit(TurnRequest::new(
            "session",
            "mutating-before-execute",
            "apply_patch",
            true,
        ))
        .await
        .expect("mutation queues behind phase1");
    handle
        .finish_external_turn(
            "session",
            "phase1-turn",
            Ok(RuntimeTurnExecution::CompletedHoldQueued {
                summary: "plan draft ready".to_string(),
            }),
        )
        .await
        .expect("phase1 hold");
    handle
        .track_external_turn(
            TurnRequest::new("session", "plan-review-turn", "Plan review", false),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("plan review parks");
    handle
        .register_interaction_with_delivery(
            "session",
            "plan-review-turn",
            InteractionState::AwaitingPlanReview {
                interaction_id: "review-race".to_string(),
            },
            Box::new(PlanReviewAckDelivery::new()),
        )
        .await
        .expect("plan review registered");
    handle
        .respond(
            "session",
            "plan-review-turn",
            InteractionResponse::new("review-race", PlanReviewDecision::Execute.wire_id()),
        )
        .await
        .expect("Execute holds for network gate");

    // Race window: a late submit after Execute must queue without starting the
    // held mutation or this new one before the network gate is parked.
    handle
        .submit(TurnRequest::new(
            "session",
            "mutating-after-execute",
            "apply_patch",
            true,
        ))
        .await
        .expect("post-Execute submit is accepted into the held queue");
    sleep(Duration::from_millis(50)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "submit after Execute must not start mutations before the network gate parks"
    );

    let network_id = network_policy_interaction_id(Some("plan-race"));
    handle
        .track_external_turn(
            TurnRequest::new("session", "network-policy-turn", "network policy", false),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("network gate parks");
    handle
        .register_interaction_with_delivery(
            "session",
            "network-policy-turn",
            InteractionState::AwaitingNetworkPolicy {
                interaction_id: network_id.clone(),
            },
            Box::new(NetworkPolicyAckDelivery::new()),
        )
        .await
        .expect("network delivery registered");
    sleep(Duration::from_millis(50)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "network gate must keep mutations fenced"
    );

    handle
        .respond(
            "session",
            "network-policy-turn",
            InteractionResponse::new(&network_id, NetworkPolicyDecision::Allow.wire_id()),
        )
        .await
        .expect("network allow");
    timeout(Duration::from_secs(2), notify.notified())
        .await
        .expect("held mutations eventually start");
    assert!(started.load(Ordering::SeqCst) >= 1);
    worker.abort();
}

/// W2-03 r12: end-to-end crash/restart for the App-owned Execute → network
/// marker ordering. Simulates the durable rows `App` writes, drops the live
/// worker before `register_local_runtime_network_policy`, then recovers the
/// gate from the session store and proves mutations stay fenced until Allow.
#[tokio::test]
async fn plan_review_network_gate_survives_worker_restart_after_execute() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use libra::internal::ai::{
        runtime::{
            AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, InteractionResponse,
            InteractionState, PrincipalContext, PrincipalRole, RuntimeCommandDurability,
            RuntimeExecutionContext, RuntimeTurnExecution, RuntimeTurnExecutor, RuntimeWorkerError,
            SecretRedactor, ToolBoundaryPolicy, ToolBoundaryRuntime, TurnRequest,
            phase1::{
                NetworkPolicyAckDelivery, NetworkPolicyDecision, PlanReviewAckDelivery,
                PlanReviewDecision, network_policy_interaction_id,
                open_network_policy_from_workflow, open_plan_review_from_workflow,
            },
        },
        session::{CodeWorkflowEventKind, SessionJsonlStore},
    };
    use tokio::{
        sync::Notify,
        time::{Duration, sleep, timeout},
    };
    use tokio_util::sync::CancellationToken;

    struct MutatingExecutor {
        started: Arc<AtomicUsize>,
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for MutatingExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            _context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_one();
            Ok(RuntimeTurnExecution::Completed {
                summary: "mutation applied".to_string(),
            })
        }
    }

    let temp = tempfile::tempdir().expect("temp dir");
    let session_root = temp.path().join("session");
    let store = SessionJsonlStore::new(session_root.clone());
    let started = Arc::new(AtomicUsize::new(0));
    let notify = Arc::new(Notify::new());
    let boundary = || {
        ToolBoundaryRuntime::new(
            Uuid::new_v4(),
            PrincipalContext {
                principal_id: "plan-review-restart".to_string(),
                role: PrincipalRole::Contributor,
            },
            ToolBoundaryPolicy::default_runtime(),
            SecretRedactor::default_runtime(),
            Arc::new(InMemoryAuditSink::default()),
        )
    };

    // Process 1: Phase 1 hold → Plan review → App-ordered network marker → Execute.
    let durability = RuntimeCommandDurability::new(SessionJsonlStore::new(session_root.clone()));
    let (handle, worker) = AgentRuntimeWorker::spawn(
        AgentRuntimeWorkerConfig::new(
            Arc::new(MutatingExecutor {
                started: Arc::clone(&started),
                notify: Arc::clone(&notify),
            }),
            boundary(),
        )
        .with_durability(durability, "repo", "principal")
        .with_durability_command_kind("tui_local_turn"),
    );
    handle
        .track_external_turn(
            TurnRequest::new("session", "phase1-turn", "plan workflow", true),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(true)),
        )
        .await
        .expect("phase1 admitted");
    handle
        .submit(TurnRequest::new(
            "session",
            "mutating-turn",
            "apply_patch",
            true,
        ))
        .await
        .expect("mutation queued");
    handle
        .finish_external_turn(
            "session",
            "phase1-turn",
            Ok(RuntimeTurnExecution::CompletedHoldQueued {
                summary: "plan draft ready".to_string(),
            }),
        )
        .await
        .expect("phase1 hold");
    store
        .append_code_workflow_durable(CodeWorkflowEventKind::PlanReviewRequested {
            interaction_id: "review-restart".to_string(),
            plan_id: "plan-restart".to_string(),
            turn_id: "plan-review-turn".to_string(),
            phase1_turn_id: "phase1-turn".to_string(),
        })
        .expect("plan review marker");
    handle
        .track_external_turn(
            TurnRequest::new("session", "plan-review-turn", "Plan review", false),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("plan review parks");
    handle
        .register_interaction_with_delivery(
            "session",
            "plan-review-turn",
            InteractionState::AwaitingPlanReview {
                interaction_id: "review-restart".to_string(),
            },
            Box::new(PlanReviewAckDelivery::new()),
        )
        .await
        .expect("plan review registered");

    let network_id = network_policy_interaction_id(Some("plan-restart"));
    // App writes the network marker *before* Execute settles.
    store
        .append_code_workflow_durable(CodeWorkflowEventKind::NetworkPolicyRequested {
            interaction_id: network_id.clone(),
            plan_id: "plan-restart".to_string(),
            turn_id: "network-policy-turn".to_string(),
            default_allow: false,
        })
        .expect("network marker before Execute");
    handle
        .respond(
            "session",
            "plan-review-turn",
            InteractionResponse::new("review-restart", PlanReviewDecision::Execute.wire_id()),
        )
        .await
        .expect("Execute settles");
    // Crash before register_local_runtime_network_policy.
    worker.abort();
    sleep(Duration::from_millis(20)).await;
    assert_eq!(started.load(Ordering::SeqCst), 0);

    let events = |root: &std::path::Path| {
        SessionJsonlStore::new(root.to_path_buf())
            .load_code_workflow_replay()
            .expect("replay")
            .events
            .into_iter()
            .map(|event| event.event)
            .collect::<Vec<_>>()
    };
    let replayed = events(&session_root);
    assert_eq!(
        open_plan_review_from_workflow(replayed.iter()),
        None,
        "Execute must have closed the plan review on disk"
    );
    assert_eq!(
        open_network_policy_from_workflow(replayed.iter()),
        Some((
            network_id.clone(),
            "plan-restart".to_string(),
            "network-policy-turn".to_string(),
            false,
        )),
        "network gate must survive restart after Execute"
    );

    // Process 2: restore gate and keep mutations fenced until Allow.
    let durability = RuntimeCommandDurability::new(SessionJsonlStore::new(session_root.clone()));
    let (handle, worker) = AgentRuntimeWorker::spawn(
        AgentRuntimeWorkerConfig::new(
            Arc::new(MutatingExecutor {
                started: Arc::clone(&started),
                notify: Arc::clone(&notify),
            }),
            boundary(),
        )
        .with_durability(durability, "repo", "principal")
        .with_durability_command_kind("tui_local_turn"),
    );
    handle
        .track_external_turn(
            TurnRequest::new("session", "network-policy-turn", "network policy", false),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .expect("restored network gate parks");
    handle
        .register_interaction_with_delivery(
            "session",
            "network-policy-turn",
            InteractionState::AwaitingNetworkPolicy {
                interaction_id: network_id.clone(),
            },
            Box::new(NetworkPolicyAckDelivery::new()),
        )
        .await
        .expect("network delivery registered");
    handle
        .submit(TurnRequest::new(
            "session",
            "mutating-after-restore",
            "apply_patch",
            true,
        ))
        .await
        .expect("post-restore mutation queues");
    sleep(Duration::from_millis(50)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "restored network gate must fence execution"
    );
    handle
        .respond(
            "session",
            "network-policy-turn",
            InteractionResponse::new(&network_id, NetworkPolicyDecision::Allow.wire_id()),
        )
        .await
        .expect("network allow after restore");
    timeout(Duration::from_secs(2), notify.notified())
        .await
        .expect("mutations start after restored Allow");
    assert!(started.load(Ordering::SeqCst) >= 1);
    worker.abort();
}

/// W2-03: Modify/Cancel on the Plan review must discard the mutations queued
/// under the review fence rather than release them — the developer never
/// approved the plan those turns would execute.
#[tokio::test]
async fn plan_review_non_execute_discards_queued_mutations() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use libra::internal::ai::runtime::{
        AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, InteractionResponse,
        InteractionState, PrincipalContext, PrincipalRole, RuntimeExecutionContext,
        RuntimeTurnExecution, RuntimeTurnExecutor, RuntimeWorkerError, SecretRedactor,
        ToolBoundaryPolicy, ToolBoundaryRuntime, TurnRequest,
        phase1::{PlanReviewAckDelivery, PlanReviewDecision},
    };
    use tokio::time::{Duration, sleep};
    use tokio_util::sync::CancellationToken;

    struct MutatingExecutor {
        started: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for MutatingExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            _context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            Ok(RuntimeTurnExecution::Completed {
                summary: "mutation applied".to_string(),
            })
        }
    }

    for decision in [PlanReviewDecision::Revise, PlanReviewDecision::Cancel] {
        let started = Arc::new(AtomicUsize::new(0));
        let executor = Arc::new(MutatingExecutor {
            started: Arc::clone(&started),
        });
        let boundary = ToolBoundaryRuntime::new(
            Uuid::new_v4(),
            PrincipalContext {
                principal_id: format!("plan-review-{}-fence", decision.wire_id()),
                role: PrincipalRole::Contributor,
            },
            ToolBoundaryPolicy::default_runtime(),
            SecretRedactor::default_runtime(),
            Arc::new(InMemoryAuditSink::default()),
        );
        let (handle, worker) =
            AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig::new(executor, boundary));

        handle
            .track_external_turn(
                TurnRequest::new("session", "plan-review-turn", "Plan review", false),
                CancellationToken::new(),
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .expect("plan review gate tracked");
        handle
            .register_interaction_with_delivery(
                "session",
                "plan-review-turn",
                InteractionState::AwaitingPlanReview {
                    interaction_id: "plan-7".to_string(),
                },
                Box::new(PlanReviewAckDelivery::new()),
            )
            .await
            .expect("worker owns the Plan review interaction");
        handle
            .submit(TurnRequest::new(
                "session",
                "mutating-turn",
                "apply_patch",
                true,
            ))
            .await
            .expect("mutating turn is accepted into the queue");
        assert_eq!(started.load(Ordering::SeqCst), 0);

        handle
            .respond(
                "session",
                "plan-review-turn",
                InteractionResponse::new("plan-7", decision.wire_id()),
            )
            .await
            .expect("non-execute decision resolves the review");
        sleep(Duration::from_millis(50)).await;
        assert_eq!(
            started.load(Ordering::SeqCst),
            0,
            "{} must discard queued mutations, not release them",
            decision.wire_id()
        );
        let snapshot = handle
            .snapshot("session")
            .await
            .expect("snapshot after non-execute decision");
        assert_eq!(snapshot.interaction, InteractionState::Completed);
        assert_eq!(
            snapshot.queued_turns,
            0,
            "{} must empty the queue of fenced mutations",
            decision.wire_id()
        );
        worker.abort();
    }
}

/// W1-04: the legacy TUI tool loop is externally executed, but its cancel
/// request must still enter the runtime before the adapter signals its local
/// cooperative token. A mutation marker shared with the runtime turns an
/// ambiguous local cancellation into the durable reconciliation fence.
#[tokio::test]
async fn external_tui_turn_cancel_uses_runtime_reconciliation() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use libra::internal::ai::{
        runtime::{
            AgentRuntimeWorker, AgentRuntimeWorkerConfig, ExternalTurnTrackingExecutor,
            InMemoryAuditSink, InteractionState, PrincipalContext, PrincipalRole,
            RuntimeCommandDurability, RuntimeWorkerError, SecretRedactor, ToolBoundaryPolicy,
            ToolBoundaryRuntime, TurnRequest,
        },
        session::{CodeCommandIdentity, CodeCommandRecovery, CodeCommandStatus, SessionJsonlStore},
    };
    use tokio_util::sync::CancellationToken;

    let boundary = ToolBoundaryRuntime::new(
        Uuid::new_v4(),
        PrincipalContext {
            principal_id: "tui-runtime-contract-test".to_string(),
            role: PrincipalRole::Contributor,
        },
        ToolBoundaryPolicy::default_runtime(),
        SecretRedactor::default_runtime(),
        Arc::new(InMemoryAuditSink::default()),
    );
    let temp = tempfile::TempDir::new().expect("temporary TUI session JSONL root");
    let durability =
        RuntimeCommandDurability::new(SessionJsonlStore::new(temp.path().join("session")));
    let (handle, worker) = AgentRuntimeWorker::spawn(
        AgentRuntimeWorkerConfig::new(Arc::new(ExternalTurnTrackingExecutor), boundary)
            .with_durability(durability.clone(), "tui-contract-repo", "tui-local")
            .with_durability_command_kind("tui_local_turn"),
    );
    let cancellation = CancellationToken::new();
    let mutation_started = Arc::new(AtomicBool::new(true));
    handle
        .track_external_turn(
            TurnRequest::new("session", "turn-1", "apply patch", true),
            cancellation.clone(),
            Arc::clone(&mutation_started),
        )
        .await
        .expect("external TUI turn admitted");

    handle
        .cancel("session", "turn-1")
        .await
        .expect("runtime accepted TUI cancellation");
    assert!(
        !cancellation.is_cancelled(),
        "a started mutation must not be locally aborted by runtime cancellation"
    );
    assert_eq!(
        handle
            .snapshot("session")
            .await
            .expect("runtime snapshot")
            .interaction,
        InteractionState::Cancelling
    );

    mutation_started.store(false, Ordering::Release);
    let finalization = handle
        .finish_external_turn("session", "turn-1", Err(RuntimeWorkerError::Cancelled))
        .await;
    assert!(
        matches!(
            finalization,
            Err(RuntimeWorkerError::ReconciliationRequired { .. })
        ),
        "the adapter must observe the worker's durable indeterminate fence"
    );
    assert!(matches!(
        handle
            .snapshot("session")
            .await
            .expect("reconciled snapshot")
            .interaction,
        InteractionState::IndeterminateSideEffect { .. }
    ));
    assert!(matches!(
        durability
            .session_store()
            .recover_code_command(&CodeCommandIdentity::new(
                "tui-contract-repo",
                "session",
                "tui-local",
                "turn-1",
            ))
            .expect("durable TUI cancellation result"),
        CodeCommandRecovery::Existing {
            status: CodeCommandStatus::Indeterminate { .. }
        }
    ));
    worker.abort();
}

/// W1-08: runtime shutdown is a structured, idempotent lifecycle operation.
/// It stops admission before cancelling a read-only turn, drains queued work,
/// and makes concurrent callers observe one terminal outcome. A non-cooperative
/// executor must return an actionable timeout rather than leaving shutdown
/// indefinitely pending.
#[tokio::test]
async fn runtime_shutdown_releases_resources() {
    use std::sync::Arc;

    use libra::internal::ai::runtime::{
        AgentRuntimeWorker, AgentRuntimeWorkerConfig, InMemoryAuditSink, PrincipalContext,
        PrincipalRole, RuntimeExecutionContext, RuntimeShutdownError, RuntimeTurnExecution,
        RuntimeTurnExecutor, RuntimeWorkerError, SecretRedactor, ToolBoundaryPolicy,
        ToolBoundaryRuntime, TurnRequest,
    };
    use tokio::{
        sync::Notify,
        time::{Duration, timeout},
    };

    fn boundary() -> ToolBoundaryRuntime {
        ToolBoundaryRuntime::new(
            Uuid::new_v4(),
            PrincipalContext {
                principal_id: "runtime-shutdown-test".to_string(),
                role: PrincipalRole::Contributor,
            },
            ToolBoundaryPolicy::default_runtime(),
            SecretRedactor::default_runtime(),
            Arc::new(InMemoryAuditSink::default()),
        )
    }

    struct CooperativeExecutor {
        started: Arc<Notify>,
        cancellation_seen: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for CooperativeExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            self.started.notify_one();
            context.cancellation().cancelled().await;
            self.cancellation_seen.notify_one();
            self.release.notified().await;
            Err(RuntimeWorkerError::Cancelled)
        }
    }

    let started = Arc::new(Notify::new());
    let cancellation_seen = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let executor = Arc::new(CooperativeExecutor {
        started: Arc::clone(&started),
        cancellation_seen: Arc::clone(&cancellation_seen),
        release: Arc::clone(&release),
    });
    let mut config = AgentRuntimeWorkerConfig::new(executor, boundary());
    config.shutdown_timeout = Duration::from_secs(1);
    let (handle, worker) = AgentRuntimeWorker::spawn(config);

    handle
        .submit(TurnRequest::new("session", "active", "read", false))
        .await
        .expect("active turn accepted");
    handle
        .submit(TurnRequest::new("session", "queued", "later", false))
        .await
        .expect("queued turn accepted");
    timeout(Duration::from_secs(1), started.notified())
        .await
        .expect("active turn started");

    let first_shutdown = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.shutdown().await })
    };
    timeout(Duration::from_secs(1), cancellation_seen.notified())
        .await
        .expect("shutdown cooperatively cancels the active turn");
    assert!(matches!(
        handle
            .submit(TurnRequest::new(
                "session",
                "rejected",
                "after shutdown",
                false
            ))
            .await,
        Err(RuntimeWorkerError::ShuttingDown)
    ));

    let second_shutdown = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.shutdown().await })
    };
    tokio::task::yield_now().await;
    release.notify_one();
    assert!(first_shutdown.await.expect("first shutdown task").is_ok());
    assert!(second_shutdown.await.expect("second shutdown task").is_ok());
    timeout(Duration::from_secs(1), worker)
        .await
        .expect("worker exits after shutdown")
        .expect("worker task does not panic");
    assert!(
        handle.shutdown().await.is_ok(),
        "a completed shutdown must stay idempotent after the worker task exits"
    );

    let drop_started = Arc::new(Notify::new());
    let drop_cancellation_seen = Arc::new(Notify::new());
    let drop_release = Arc::new(Notify::new());
    let drop_executor = Arc::new(CooperativeExecutor {
        started: Arc::clone(&drop_started),
        cancellation_seen: Arc::clone(&drop_cancellation_seen),
        release: Arc::clone(&drop_release),
    });
    let (drop_handle, drop_worker) =
        AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig::new(drop_executor, boundary()));
    drop_handle
        .submit(TurnRequest::new("dropped", "active", "read", false))
        .await
        .expect("last-handle-drop turn accepted");
    timeout(Duration::from_secs(1), drop_started.notified())
        .await
        .expect("last-handle-drop turn started");
    drop(drop_handle);
    timeout(Duration::from_secs(1), drop_cancellation_seen.notified())
        .await
        .expect("dropping the last runtime handle starts the same shutdown path");
    drop_release.notify_one();
    timeout(Duration::from_secs(1), drop_worker)
        .await
        .expect("worker exits after last handle drop")
        .expect("last-handle-drop worker task does not panic");

    struct MutatingExecutor {
        started: Arc<Notify>,
        cancellation_seen: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for MutatingExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            context.mark_mutation_started();
            self.started.notify_one();
            let cancellation = context.cancellation();
            tokio::select! {
                _ = cancellation.cancelled() => {
                    self.cancellation_seen.notify_one();
                    Err(RuntimeWorkerError::Cancelled)
                }
                _ = self.release.notified() => Ok(RuntimeTurnExecution::Completed {
                    summary: "mutation completed determinately".to_string(),
                }),
            }
        }
    }

    let mutation_started = Arc::new(Notify::new());
    let mutation_cancellation_seen = Arc::new(Notify::new());
    let mutation_release = Arc::new(Notify::new());
    let mutating_executor = Arc::new(MutatingExecutor {
        started: Arc::clone(&mutation_started),
        cancellation_seen: Arc::clone(&mutation_cancellation_seen),
        release: Arc::clone(&mutation_release),
    });
    let (mutating_handle, mutating_worker) =
        AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig::new(mutating_executor, boundary()));
    mutating_handle
        .submit(TurnRequest::new("mutating", "active", "write", true))
        .await
        .expect("mutating turn accepted");
    timeout(Duration::from_secs(1), mutation_started.notified())
        .await
        .expect("mutating turn started");
    let mutating_shutdown = {
        let handle = mutating_handle.clone();
        tokio::spawn(async move { handle.shutdown().await })
    };
    assert!(
        timeout(
            Duration::from_millis(100),
            mutation_cancellation_seen.notified()
        )
        .await
        .is_err(),
        "shutdown must not cancel a mutation after its side effect begins"
    );
    mutation_release.notify_one();
    assert!(
        mutating_shutdown
            .await
            .expect("mutating shutdown task")
            .is_ok(),
        "a determinate mutation completion releases shutdown"
    );
    timeout(Duration::from_secs(1), mutating_worker)
        .await
        .expect("worker exits after determinate mutation")
        .expect("mutating worker task does not panic");

    struct StuckExecutor;

    #[async_trait]
    impl RuntimeTurnExecutor for StuckExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            _context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            std::future::pending().await
        }
    }

    let mut stuck_config = AgentRuntimeWorkerConfig::new(Arc::new(StuckExecutor), boundary());
    stuck_config.shutdown_timeout = Duration::from_millis(20);
    let (stuck_handle, stuck_worker) = AgentRuntimeWorker::spawn(stuck_config);
    stuck_handle
        .submit(TurnRequest::new("stuck", "active", "read", false))
        .await
        .expect("stuck turn accepted");
    let timeout_error = stuck_handle
        .shutdown()
        .await
        .expect_err("non-cooperative executor must hit the shutdown deadline");
    assert!(matches!(
        &timeout_error,
        RuntimeShutdownError::TimedOut { unreleased_resources }
            if unreleased_resources == &vec!["runtime_turn".to_string()]
    ));
    timeout(Duration::from_secs(1), stuck_worker)
        .await
        .expect("timed-out worker exits")
        .expect("timed-out worker task does not panic");
    assert_eq!(
        stuck_handle.shutdown().await,
        Err(timeout_error),
        "a completed failed shutdown must preserve its diagnostic for repeat callers"
    );

    struct StuckMutatingExecutor {
        started: Arc<Notify>,
    }

    #[async_trait]
    impl RuntimeTurnExecutor for StuckMutatingExecutor {
        async fn execute(
            &self,
            _request: TurnRequest,
            context: RuntimeExecutionContext,
        ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
            context.mark_mutation_started();
            self.started.notify_one();
            std::future::pending().await
        }
    }

    let stuck_mutation_started = Arc::new(Notify::new());
    let mutating_stuck_executor = Arc::new(StuckMutatingExecutor {
        started: Arc::clone(&stuck_mutation_started),
    });
    let mutating_stuck_config = AgentRuntimeWorkerConfig::new(mutating_stuck_executor, boundary());
    let (mutating_stuck_handle, mutating_stuck_worker) =
        AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig {
            shutdown_timeout: Duration::from_millis(20),
            ..mutating_stuck_config
        });
    mutating_stuck_handle
        .submit(TurnRequest::new("stuck-mutation", "active", "write", true))
        .await
        .expect("stuck mutating turn accepted");
    timeout(Duration::from_secs(1), stuck_mutation_started.notified())
        .await
        .expect("stuck mutation reached dispatch boundary");
    assert!(matches!(
        mutating_stuck_handle.shutdown().await,
        Err(RuntimeShutdownError::TimedOut { unreleased_resources })
            if unreleased_resources == vec!["mutating_runtime_turn_reconciliation".to_string()]
    ));
    timeout(Duration::from_secs(1), mutating_stuck_worker)
        .await
        .expect("timed-out mutating worker exits")
        .expect("timed-out mutating worker task does not panic");
}

/// W1-08: SIGINT/SIGTERM-shaped process shutdown and half-initialized startup
/// failure share one [`LifecycleShutdownOwner`] deadline/result contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_shutdown_on_signal_and_startup_failure() {
    use std::sync::Arc;

    use libra::internal::ai::runtime::{
        LifecycleShutdownError, LifecycleShutdownOwner, LifecycleStepError, lifecycle_resource,
    };
    use tokio::{
        sync::Notify,
        time::{Duration, timeout},
    };

    // Signal path: runtime finishes, but a stuck listener reports its category
    // under the shared owner deadline (same contract as SIGINT/SIGTERM cleanup).
    let signal_owner = LifecycleShutdownOwner::with_timeout(Duration::from_millis(40));
    signal_owner
        .push_step(lifecycle_resource::RUNTIME_TURN, async { Ok(()) })
        .await;
    signal_owner
        .push_step(lifecycle_resource::CONTROLLER_LEASE, async { Ok(()) })
        .await;
    signal_owner
        .push_step(lifecycle_resource::MCP_SERVER, async { Ok(()) })
        .await;
    signal_owner
        .push_step(lifecycle_resource::CONTROL_LOCK, async { Ok(()) })
        .await;
    signal_owner
        .push_step(lifecycle_resource::WEB_SERVER, async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok(())
        })
        .await;

    let signal_first = {
        let owner = signal_owner.clone();
        tokio::spawn(async move { owner.shutdown().await })
    };
    let signal_second = {
        let owner = signal_owner.clone();
        tokio::spawn(async move { owner.shutdown().await })
    };
    let signal_a = signal_first.await.expect("join signal shutdown");
    let signal_b = signal_second.await.expect("join repeated signal shutdown");
    assert_eq!(signal_a, signal_b);
    assert!(matches!(
        signal_a,
        Err(LifecycleShutdownError::TimedOut {
            unreleased_resources
        }) if unreleased_resources == vec![lifecycle_resource::WEB_SERVER.to_string()]
    ));

    // Startup-failure path: only the resources that were actually started are
    // registered; a stuck managed child still surfaces under the same owner.
    let started = Arc::new(Notify::new());
    let startup_owner = LifecycleShutdownOwner::with_timeout(Duration::from_millis(30));
    startup_owner
        .push_step(lifecycle_resource::RUNTIME_TURN, async { Ok(()) })
        .await;
    startup_owner
        .push_step(lifecycle_resource::TEMP_FILE, async { Ok(()) })
        .await;
    {
        let started = Arc::clone(&started);
        startup_owner
            .push_step(lifecycle_resource::MANAGED_CODEX_CHILD, async move {
                started.notify_one();
                tokio::time::sleep(Duration::from_secs(2)).await;
                Ok(())
            })
            .await;
    }

    let startup = {
        let owner = startup_owner.clone();
        tokio::spawn(async move { owner.shutdown().await })
    };
    timeout(Duration::from_secs(1), started.notified())
        .await
        .expect("startup-failure cleanup reached the managed child step");
    let startup_result = startup.await.expect("join startup shutdown");
    assert!(matches!(
        &startup_result,
        Err(LifecycleShutdownError::TimedOut {
            unreleased_resources
        }) if unreleased_resources.as_slice()
            == [lifecycle_resource::MANAGED_CODEX_CHILD]
    ));
    assert_eq!(
        startup_owner.shutdown().await,
        startup_result,
        "startup-failure shutdown must stay idempotent"
    );

    // Nested runtime timeout categories propagate through a lifecycle step.
    let nested = LifecycleShutdownOwner::with_timeout(Duration::from_millis(50));
    nested
        .push_step(lifecycle_resource::RUNTIME_TURN, async {
            Err(LifecycleStepError::timed_out_with([
                lifecycle_resource::MUTATING_RUNTIME_TURN_RECONCILIATION,
            ]))
        })
        .await;
    nested
        .push_step(lifecycle_resource::WEB_SERVER, async { Ok(()) })
        .await;
    assert!(matches!(
        nested.shutdown().await,
        Err(LifecycleShutdownError::TimedOut {
            unreleased_resources
        }) if unreleased_resources
            == vec![lifecycle_resource::MUTATING_RUNTIME_TURN_RECONCILIATION.to_string()]
    ));
}

/// W2-04: confirmed plan execution enters the serialized worker queue, and a
/// mutating tool is refused when the shared hardening boundary denies it.
#[tokio::test]
async fn plan_execution_enters_runtime_queue() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use libra::internal::ai::runtime::{
        AgentRuntimeWorker, AgentRuntimeWorkerConfig, DeferredPlanExecutionExecutor,
        InMemoryAuditSink, InteractionState, PLAN_EXECUTION_TURN_INPUT, PrincipalContext,
        PrincipalRole, RuntimeTurnExecution, SecretRedactor, ToolBoundaryPolicy,
        ToolBoundaryRuntime, is_plan_execution_turn, plan_execution_turn_request,
        submit_confirmed_plan_execution,
    };
    use tokio::{
        sync::Notify,
        time::{Duration, timeout},
    };

    let starts = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let first_started = Arc::new(Notify::new());
    let release_first = Arc::new(Notify::new());
    let executor = Arc::new(DeferredPlanExecutionExecutor::new());

    let contributor_boundary = ToolBoundaryRuntime::new(
        Uuid::new_v4(),
        PrincipalContext {
            principal_id: "plan-execution-contributor".to_string(),
            role: PrincipalRole::Contributor,
        },
        ToolBoundaryPolicy::default_runtime(),
        SecretRedactor::default_runtime(),
        Arc::new(InMemoryAuditSink::default()),
    );
    let (handle, worker) = AgentRuntimeWorker::spawn(AgentRuntimeWorkerConfig::new(
        executor.clone(),
        contributor_boundary,
    ));

    let starts_for_first = starts.clone();
    let first_started_notify = first_started.clone();
    let release_first_for_runner = release_first.clone();
    submit_confirmed_plan_execution(
        &handle,
        executor.as_ref(),
        "session",
        "plan-exec-1",
        Box::new(move |_context| {
            Box::pin(async move {
                starts_for_first
                    .lock()
                    .await
                    .push("plan-exec-1".to_string());
                first_started_notify.notify_one();
                release_first_for_runner.notified().await;
                Ok(RuntimeTurnExecution::Completed {
                    summary: "first plan executed".to_string(),
                })
            })
        }),
    )
    .await
    .expect("first plan execution admitted");

    timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("first plan execution started on the worker");
    assert_eq!(
        starts.lock().await.as_slice(),
        ["plan-exec-1"],
        "only the dequeued plan-execution turn may start"
    );
    let snapshot = handle.snapshot("session").await.expect("snapshot");
    assert_eq!(snapshot.active_turn_id.as_deref(), Some("plan-exec-1"));
    assert_eq!(snapshot.queued_turns, 0);
    assert!(is_plan_execution_turn(&plan_execution_turn_request(
        "session",
        "plan-exec-1"
    )));
    assert_eq!(
        plan_execution_turn_request("session", "x").input,
        PLAN_EXECUTION_TURN_INPUT
    );

    // While the first plan turn is active, stage+submit a second plan turn —
    // it must remain queued until cancelled or the first completes.
    let starts_for_second = starts.clone();
    submit_confirmed_plan_execution(
        &handle,
        executor.as_ref(),
        "session",
        "plan-exec-2",
        Box::new(move |_context| {
            Box::pin(async move {
                starts_for_second
                    .lock()
                    .await
                    .push("plan-exec-2".to_string());
                Ok(RuntimeTurnExecution::Completed {
                    summary: "second plan executed".to_string(),
                })
            })
        }),
    )
    .await
    .expect("second plan execution queued");

    let snapshot = handle
        .snapshot("session")
        .await
        .expect("snapshot while held");
    assert_eq!(snapshot.active_turn_id.as_deref(), Some("plan-exec-1"));
    assert_eq!(snapshot.queued_turns, 1);
    assert_eq!(
        starts.lock().await.as_slice(),
        ["plan-exec-1"],
        "queued plan must not start while the prior plan-execution turn is active"
    );

    // Cancel the queued second plan before it dequeues — on_admission_discarded
    // must release the staged runner so a later plan can stage again.
    handle
        .cancel("session", "plan-exec-2")
        .await
        .expect("cancel queued plan-execution turn");
    let snapshot = handle
        .snapshot("session")
        .await
        .expect("snapshot after queued cancel");
    assert_eq!(snapshot.active_turn_id.as_deref(), Some("plan-exec-1"));
    assert_eq!(snapshot.queued_turns, 0);
    assert_eq!(
        starts.lock().await.as_slice(),
        ["plan-exec-1"],
        "cancelled queued plan must never execute"
    );

    let third_started = Arc::new(Notify::new());
    let starts_for_third = starts.clone();
    let third_started_notify = third_started.clone();
    submit_confirmed_plan_execution(
        &handle,
        executor.as_ref(),
        "session",
        "plan-exec-3",
        Box::new(move |_context| {
            Box::pin(async move {
                starts_for_third
                    .lock()
                    .await
                    .push("plan-exec-3".to_string());
                third_started_notify.notify_one();
                Ok(RuntimeTurnExecution::Completed {
                    summary: "third plan executed".to_string(),
                })
            })
        }),
    )
    .await
    .expect("third plan stages after queued cancel discarded the second runner");

    release_first.notify_one();
    timeout(Duration::from_secs(2), third_started.notified())
        .await
        .expect("third plan starts after the first completes");
    assert_eq!(
        starts.lock().await.as_slice(),
        ["plan-exec-1", "plan-exec-3"],
        "cancelled queued plan must stay out of the execution order"
    );
    timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = handle.snapshot("session").await.expect("snapshot");
            if snapshot.active_turn_id.is_none()
                && matches!(snapshot.interaction, InteractionState::Completed)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("third plan reaches a terminal idle snapshot");

    // Shutdown must also discard any remaining queued staged runner.
    let shutdown_started = Arc::new(Notify::new());
    let shutdown_release = Arc::new(Notify::new());
    let shutdown_body = Arc::new(AtomicUsize::new(0));
    let shutdown_started_notify = shutdown_started.clone();
    let shutdown_release_wait = shutdown_release.clone();
    submit_confirmed_plan_execution(
        &handle,
        executor.as_ref(),
        "session",
        "plan-exec-shutdown-active",
        Box::new(move |_context| {
            Box::pin(async move {
                shutdown_started_notify.notify_one();
                shutdown_release_wait.notified().await;
                Ok(RuntimeTurnExecution::Completed {
                    summary: "shutdown active".to_string(),
                })
            })
        }),
    )
    .await
    .expect("shutdown-active plan admitted");
    timeout(Duration::from_secs(2), shutdown_started.notified())
        .await
        .expect("shutdown-active plan started");
    let shutdown_body_queued = shutdown_body.clone();
    submit_confirmed_plan_execution(
        &handle,
        executor.as_ref(),
        "session",
        "plan-exec-shutdown-queued",
        Box::new(move |_context| {
            Box::pin(async move {
                shutdown_body_queued.fetch_add(1, Ordering::SeqCst);
                Ok(RuntimeTurnExecution::Completed {
                    summary: "should be discarded on shutdown".to_string(),
                })
            })
        }),
    )
    .await
    .expect("shutdown-queued plan staged");
    let shutdown = handle.shutdown();
    // Release the active turn so shutdown can finish cooperatively.
    shutdown_release.notify_one();
    timeout(Duration::from_secs(5), shutdown)
        .await
        .expect("shutdown timeout")
        .expect("worker shutdown discards queued plan admissions");
    assert_eq!(
        shutdown_body.load(Ordering::SeqCst),
        0,
        "shutdown must discard queued plan runners without executing them"
    );
    worker.abort();

    // Observer principal must deny confirmed plan execution before the body runs.
    let observer_executor = Arc::new(DeferredPlanExecutionExecutor::new());
    let observer_boundary = ToolBoundaryRuntime::new(
        Uuid::new_v4(),
        PrincipalContext {
            principal_id: "plan-execution-observer".to_string(),
            role: PrincipalRole::Observer,
        },
        ToolBoundaryPolicy::default_runtime(),
        SecretRedactor::default_runtime(),
        Arc::new(InMemoryAuditSink::default()),
    );
    let (observer_handle, observer_worker) = AgentRuntimeWorker::spawn(
        AgentRuntimeWorkerConfig::new(observer_executor.clone(), observer_boundary),
    );
    let body_ran = Arc::new(AtomicUsize::new(0));
    let body_ran_runner = body_ran.clone();
    submit_confirmed_plan_execution(
        &observer_handle,
        observer_executor.as_ref(),
        "session",
        "denied-plan",
        Box::new(move |_context| {
            Box::pin(async move {
                body_ran_runner.fetch_add(1, Ordering::SeqCst);
                Ok(RuntimeTurnExecution::Completed {
                    summary: "should not run".to_string(),
                })
            })
        }),
    )
    .await
    .expect("denied plan still admits onto the queue");

    timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = observer_handle.snapshot("session").await.expect("snapshot");
            if matches!(
                snapshot.interaction,
                InteractionState::Failed { .. } | InteractionState::Completed
            ) && snapshot.active_turn_id.is_none()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("observer-denied plan reaches a terminal state");
    assert_eq!(
        body_ran.load(Ordering::SeqCst),
        0,
        "mutating plan body must not run when the tool boundary denies apply_patch"
    );
    let denied = observer_handle
        .snapshot("session")
        .await
        .expect("final snapshot");
    assert!(
        matches!(denied.interaction, InteractionState::Failed { .. }),
        "observer deny must fail the plan-execution turn: {denied:?}"
    );
    observer_worker.abort();
}

// ---------------------------------------------------------------------------
// CEX-00.5: top-level Event / Snapshot trait contract
// ---------------------------------------------------------------------------

mod cex_00_5 {
    use chrono::Utc;
    use libra::internal::ai::{
        hooks::lifecycle::{LifecycleEvent, LifecycleEventKind},
        runtime::{
            Event, Snapshot, audit_action_for,
            contracts::{MaterializedProjection, ProjectionFreshness, ProjectionVersions},
        },
    };
    use uuid::Uuid;

    fn lifecycle(kind: LifecycleEventKind) -> LifecycleEvent {
        LifecycleEvent {
            kind,
            session_id: "test-session".to_string(),
            session_ref: None,
            prompt: None,
            model: None,
            source: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            assistant_message: None,
            timestamp: Utc::now(),
        }
    }

    fn projection(thread_id: Uuid) -> MaterializedProjection {
        MaterializedProjection {
            thread_id,
            versions: ProjectionVersions::default(),
            freshness: ProjectionFreshness::Fresh,
            summary: serde_json::Value::Null,
        }
    }

    #[test]
    fn lifecycle_event_kinds_match_display_strings() {
        // The Event::event_kind impl must mirror the existing Display impl
        // verbatim; drift between them would fork audit / wire / log readers.
        let cases = [
            (LifecycleEventKind::SessionStart, "session_start"),
            (LifecycleEventKind::TurnStart, "turn_start"),
            (LifecycleEventKind::ToolUse, "tool_use"),
            (LifecycleEventKind::ModelUpdate, "model_update"),
            (LifecycleEventKind::Compaction, "compaction"),
            (LifecycleEventKind::TurnEnd, "turn_end"),
            (LifecycleEventKind::SessionEnd, "session_end"),
        ];
        for (kind, expected) in cases {
            let event = lifecycle(kind);
            assert_eq!(event.event_kind(), expected);
            assert_eq!(format!("{}", event.kind), expected);
        }
    }

    /// CEX-00.5 P2 fix (round 3 — byte-for-byte golden): pin the actual
    /// `Uuid::new_v5` output for a fixed `(session_id, timestamp_nanos,
    /// kind)` tuple so that **any** change to either the namespace UUID
    /// (`LIFECYCLE_EVENT_NAMESPACE` in `lifecycle.rs`) or the name-bytes
    /// layout will fail this test. Audit logs may persist `event_id`, so a
    /// silent change to the derivation would break dedupe / correlation
    /// across upgrades.
    ///
    /// To regenerate the golden value (only on a deliberate, versioned
    /// migration to a new namespace / layout):
    /// ```text
    /// $ cargo test --test ai_runtime_contract_test \
    ///     lifecycle_event_id_v5_golden -- --nocapture
    /// ```
    /// then copy the printed value into `EXPECTED_GOLDEN` and document
    /// the migration in the audit closure.
    #[test]
    fn lifecycle_event_id_v5_golden_value_is_stable() {
        const EXPECTED_GOLDEN: &str = "69eaa838-b433-55f6-8068-d943a56cfcb8";

        let event = LifecycleEvent {
            kind: LifecycleEventKind::TurnStart,
            session_id: "golden".to_string(),
            session_ref: None,
            prompt: None,
            model: None,
            source: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            assistant_message: None,
            timestamp: chrono::DateTime::<Utc>::from_timestamp_nanos(1_700_000_000_000_000_000),
        };

        let id = event.event_id();
        let expected = Uuid::parse_str(EXPECTED_GOLDEN).expect("parseable golden UUID");
        assert_eq!(
            id, expected,
            "lifecycle event_id derivation drifted — see test docs for migration steps"
        );

        // Structural sanity (catches the case where someone updates both
        // the golden and the derivation but breaks the version/variant).
        assert_eq!(id.get_version_num(), 5, "must be UUIDv5 (SHA-1 namespaced)");
        assert_eq!(
            id.get_variant(),
            uuid::Variant::RFC4122,
            "must be RFC 4122 variant"
        );
    }

    /// CEX-00.5 P1 (R-A3) test coverage: the `Event` trait does not
    /// enforce envelope-with-typed-payload at compile time, but the
    /// canonical pattern (`tag = "kind", content = "payload"` plus an
    /// `untagged` Known/Unknown wrapper) MUST stay reachable so concrete
    /// implementors keep using it. This test exercises the pattern with a
    /// tiny in-test event hierarchy and proves an unknown future variant
    /// falls through to `Unknown(Value)` instead of erroring.
    #[test]
    fn r_a3_envelope_pattern_round_trips_and_survives_unknown_kinds() {
        use serde::{Deserialize, Serialize};
        use serde_json::json;

        #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
        #[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
        enum DemoEvent {
            Started { id: u64 },
            Stopped { id: u64, reason: String },
        }

        #[derive(Debug, Serialize, Deserialize)]
        #[serde(untagged)]
        enum DemoEventEnvelope {
            Known(Box<DemoEvent>),
            Unknown(serde_json::Value),
        }

        // 1. A known variant round-trips through the envelope.
        let started = DemoEvent::Started { id: 7 };
        let wire = serde_json::to_value(&started).unwrap();
        assert_eq!(wire["kind"], "started");
        let envelope: DemoEventEnvelope = serde_json::from_value(wire).unwrap();
        match envelope {
            DemoEventEnvelope::Known(event) => assert_eq!(*event, started),
            DemoEventEnvelope::Unknown(_) => panic!("known variant must not fall through"),
        }

        // 2. An unknown future variant falls through to Unknown(Value)
        // and the raw payload is preserved verbatim.
        let future = json!({
            "kind": "future_variant",
            "payload": { "anything": [1, 2, 3] }
        });
        let envelope: DemoEventEnvelope = serde_json::from_value(future.clone())
            .expect("unknown kind must not error — R-A3 / S2-INV-10");
        match envelope {
            DemoEventEnvelope::Known(_) => panic!("unknown kind must not parse as Known"),
            DemoEventEnvelope::Unknown(raw) => assert_eq!(raw, future),
        }
    }

    #[test]
    fn lifecycle_event_id_is_deterministic_and_collision_safe() {
        // CEX-00.5 P2 fix: derive `event_id()` deterministically from
        // (session_id, timestamp_nanos, kind) so the id is stable for an
        // occurrence and distinct events do not silently collide.
        let mut a = lifecycle(LifecycleEventKind::TurnStart);
        a.session_id = "alpha".to_string();
        a.timestamp = chrono::DateTime::<Utc>::from_timestamp_nanos(1_700_000_000_000_000_000);
        let mut b = a.clone();

        // Same input -> same id.
        assert_eq!(a.event_id(), b.event_id());
        // Stable across clones / impl Trait coercion.
        let dyn_ref: &dyn Event = &a;
        assert_eq!(dyn_ref.event_id(), a.event_id());

        // Different session -> different id.
        b.session_id = "beta".to_string();
        assert_ne!(a.event_id(), b.event_id());

        // Different timestamp -> different id.
        let mut c = a.clone();
        c.timestamp = a.timestamp + chrono::Duration::nanoseconds(1);
        assert_ne!(a.event_id(), c.event_id());

        // Different kind -> different id.
        let mut d = a.clone();
        d.kind = LifecycleEventKind::ToolUse;
        assert_ne!(a.event_id(), d.event_id());

        // Never the nil UUID for a real event.
        assert_ne!(a.event_id(), Uuid::nil());
    }

    #[test]
    fn lifecycle_event_summary_includes_kind_and_session() {
        let event = lifecycle(LifecycleEventKind::ToolUse);
        let summary = event.event_summary();
        assert!(summary.contains("kind=tool_use"));
        assert!(summary.contains("session=test-session"));
    }

    #[test]
    fn lifecycle_event_summary_carries_tool_when_present() {
        let mut event = lifecycle(LifecycleEventKind::ToolUse);
        event.tool_name = Some("apply_patch".to_string());
        let summary = event.event_summary();
        assert!(summary.contains("tool=apply_patch"));
    }

    #[test]
    fn audit_action_for_lifecycle_event_produces_event_prefixed_kind() {
        let event = lifecycle(LifecycleEventKind::SessionEnd);
        let dyn_ref: &dyn Event = &event;
        assert_eq!(audit_action_for(dyn_ref), "event/session_end");
    }

    #[test]
    fn event_trait_is_dyn_compatible() {
        // CEX-00.5 contract: Event must be dyn-compatible so callers can
        // pass `&dyn Event` (e.g. into `AuditSink::record_event`).
        let event = lifecycle(LifecycleEventKind::Compaction);
        let dyn_ref: &dyn Event = &event;
        let _kind = dyn_ref.event_kind();
    }

    #[test]
    fn lifecycle_event_kind_strings_are_stable_snake_case() {
        for kind in [
            LifecycleEventKind::SessionStart,
            LifecycleEventKind::TurnStart,
            LifecycleEventKind::ToolUse,
            LifecycleEventKind::ModelUpdate,
            LifecycleEventKind::Compaction,
            LifecycleEventKind::TurnEnd,
            LifecycleEventKind::SessionEnd,
        ] {
            let event = lifecycle(kind);
            let kind_str = event.event_kind();
            assert!(
                kind_str.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "event_kind '{kind_str}' must be snake_case ascii"
            );
            assert!(!kind_str.is_empty());
        }
    }

    #[test]
    fn materialized_projection_snapshot_id_is_thread_id() {
        let id = Uuid::new_v4();
        let snap = projection(id);
        assert_eq!(snap.snapshot_kind(), "materialized_projection");
        assert_eq!(snap.snapshot_id(), id);
    }

    #[test]
    fn snapshot_trait_is_dyn_compatible() {
        let snap = projection(Uuid::nil());
        let dyn_ref: &dyn Snapshot = &snap;
        assert_eq!(dyn_ref.snapshot_kind(), "materialized_projection");
    }

    #[test]
    fn snapshot_id_is_stable_under_clone() {
        let id = Uuid::new_v4();
        let snap = projection(id);
        let cloned = snap.clone();
        assert_eq!(snap.snapshot_id(), cloned.snapshot_id());
        assert_eq!(snap.snapshot_kind(), cloned.snapshot_kind());
    }
}
