use crfty_core::{
    AnalysisIntent, ClaimId, DurableDelta, ExecutionSettings, ItemOutcome, JobAction, JobSpec,
    Operation, OutputTarget, OverwriteDecision, QueueItem, QueueItemId, QueueItemState,
    ReservedJob, RunId, UnixMillis,
};
use std::path::PathBuf;

pub(crate) fn prepared() -> Vec<DurableDelta> {
    let item = QueueItem {
        id: QueueItemId(1),
        input: PathBuf::from("synthetic.mkv"),
        operation: Operation::Convert,
        intent: AnalysisIntent::ReuseIfFresh,
        output_target: OutputTarget::Replace,
        overwrite: OverwriteDecision::FollowSettings,
        state: QueueItemState::Queued,
    };
    let reserved = ReservedJob {
        item_id: item.id,
        claim_id: ClaimId(1),
        run_id: RunId(2),
        input: item.input.clone(),
        operation: item.operation,
        intent: item.intent,
        output_target: item.output_target.clone(),
    };
    let mut execution = ExecutionSettings::default();
    execution.profile.ab_av1_revision = "synthetic-ab-av1".to_owned();
    execution.profile.ffmpeg_revision = "synthetic-ffmpeg".to_owned();
    execution.profile.encoder_revision = "synthetic-encoder".to_owned();
    let spec = JobSpec {
        item_id: item.id,
        claim_id: reserved.claim_id,
        run_id: reserved.run_id,
        input: item.input.clone(),
        source: None,
        operation: item.operation,
        intent: item.intent,
        output_target: item.output_target.clone(),
        execution,
        action: JobAction::Encode {
            selected_analysis: None,
        },
    };
    vec![
        DurableDelta::QueueAdded { item },
        DurableDelta::ItemReserved {
            job: Box::new(reserved),
        },
        DurableDelta::ItemPrepared {
            spec: Box::new(spec),
        },
    ]
}

pub(crate) fn stopped() -> DurableDelta {
    DurableDelta::ItemFinished {
        item_id: QueueItemId(1),
        claim_id: ClaimId(1),
        run_id: RunId(2),
        outcome: ItemOutcome::Stopped,
        at: UnixMillis(123),
        phase_spans: Vec::new(),
    }
}
